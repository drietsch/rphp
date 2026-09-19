//! Lower `rphp_ast::v2` to `rphp-bytecode` (the v2 contract: `Init*`/`Send*`/
//! `DoCall` calls, late-bound names, references and fetch-for-write chains,
//! control flow, operators).
//!
//! The pipeline is `parse_v2 → rphp_hir::lower → compile` (plan F4): the
//! parsed program first goes through the HIR pass — name resolution (every
//! class/function/constant name carries its [`rphp_hir::Resolved`] entry
//! and declarations are renamed to their FQN), magic-constant folding,
//! php's compile-time validation, and desugaring into the canonical subset
//! (`Let`/`Temp`/`Seq` temporaries). An HIR *error* is php's compile-time
//! fatal and fails the compile before anything is lowered.
//!
//! The lowering is a straightforward tree-walk into three-address register
//! bytecode (`rphp-bytecode`). Function `0` is the synthetic `{main}`; every
//! other function (hoisted or conditional declarations, methods, closures,
//! default-value thunks) is appended through a sink as it is compiled, so
//! [`FuncId`]s are known before bodies finish. Classes are numbered by a
//! pre-pass over the whole unit ([`class::collect_class_ids`]) so `extends`
//! can name a class declared later or conditionally. Every name a program
//! *uses* — functions, classes, constants — stays a name in the bytecode
//! (`Const::Name`, spelled as the resolver resolved it: `App\Foo`) and is
//! resolved by the runtime (plan D5): the compiler no longer depends on the
//! native registry. Unqualified function and constant names inside a
//! namespace carry both candidates of php's two-step rule
//! (`InitFCall{name: N\f, ns_fallback: f}`).
//!
//! Each function pre-scans its body to give every variable a permanent
//! register; intermediate results use a stack of temporaries allocated above
//! the variable region. `$this` is a frame slot read with `LoadThis`.
//!
//! What the compiler lowers is described in [`stmt`], [`expr`] and [`class`];
//! every other node, variant or flag is reported as `RPHP_E0300 unsupported
//! construct: <what> (not lowered yet)` with the node's span, through the
//! single [`unsupported`] helper, so a program outside the slice fails with a
//! precise list rather than mis-compiling. Types on parameters, properties
//! and returns, attributes and doc comments are metadata the slice ignores.
//!
//! Errors (duplicate declaration, invalid `break` level, `goto` into a loop,
//! unsupported construct, …) are collected as [`Diagnostic`]s; the compile
//! fails iff any of them is an error.
//!
//! | module   | contents                                                  |
//! |----------|-----------------------------------------------------------|
//! | [`func`] | `FnCompiler`, function/closure/thunk drivers, emit helpers|
//! | [`stmt`] | statement lowering                                        |
//! | [`expr`] | expression lowering (calls, write targets, operators)     |
//! | [`class`]| class pre-pass and `Class` assembly                       |
//! | [`regs`] | register pre-scan and arrow-function capture visitors     |
#![forbid(unsafe_code)]

mod class;
mod expr;
mod func;
mod regs;
mod stmt;
mod tryfin;

#[cfg(test)]
mod tests;

use std::path::PathBuf;
use std::rc::Rc;

use std::collections::HashSet;

use rphp_ast::v2::{ClassLike, FuncDecl, Program, Stmt};
use rphp_bytecode::{Const, FuncId, Module, Op};
use rphp_diagnostics::{codes, Diagnostic};
use rphp_hir::{Hir, LowerOptions};
use rphp_intern::Interner;
use rphp_span::Span;

use func::{compile_function, ClosureBody, FnSpec, ModuleCtx};

/// Reading `$a[]` (the empty-subscript append form) is not a valid expression.
pub const INVALID_APPEND_READ: &str = "RPHP_E0104";
/// A cyclic `extends`.
pub const REDECLARED_CLASS: &str = "RPHP_E0106";
/// `extends Foo` where `Foo` is not declared in the unit.
pub const UNDEFINED_CLASS: &str = "RPHP_E0107";
/// A property default that is not a constant expression.
pub const NON_CONST_PROP_DEFAULT: &str = "RPHP_E0108";
/// `self::`/`parent::`/`static::` used outside a class.
pub const INVALID_SCOPE: &str = "RPHP_E0110";
/// `break`/`continue` outside a loop or with too many levels.
pub const INVALID_BREAK: &str = "RPHP_E0111";
/// `goto` to an undefined label, or a label defined twice.
pub const UNDEFINED_LABEL: &str = "RPHP_E0112";
/// `goto` into a loop or `switch`.
pub const GOTO_INTO_LOOP: &str = "RPHP_E0113";
/// An expression that cannot be written to (`f() = 1`, `$this = …`).
pub const INVALID_WRITE_TARGET: &str = "RPHP_E0114";
/// A syntactically valid construct the compiler does not lower yet
/// (`codes::UNSUPPORTED_CONSTRUCT`).
pub const UNSUPPORTED_CONSTRUCT: &str = codes::UNSUPPORTED_CONSTRUCT;

/// Report a construct outside the lowered slice:
/// `RPHP_E0300 unsupported construct: <what> (not lowered yet)` at `span`.
pub(crate) fn unsupported(diags: &mut Vec<Diagnostic>, span: Span, what: &str) {
    diags.push(
        Diagnostic::error(
            UNSUPPORTED_CONSTRUCT,
            format!("unsupported construct: {what} (not lowered yet)"),
        )
        .with_primary(span, "not lowered yet"),
    );
}

/// What [`compile`] needs besides the program.
#[derive(Default)]
pub struct CompileOptions<'a> {
    /// Maps a byte offset in the source to its 1-based line, for
    /// `Function::lines` and `__LINE__`; `None` leaves the line tables empty
    /// (and folds `__LINE__` to 0).
    pub line_of: Option<&'a dyn Fn(u32) -> u32>,
    /// The script's path as php reports it (`__FILE__`; `__DIR__` is its
    /// parent). `None` for `-r`/eval code (`Command line code`, cwd).
    pub file: Option<PathBuf>,
}

impl CompileOptions<'_> {
    /// The HIR naming options these compile options imply: the file (or
    /// php's `Command line code` unit with the working directory as
    /// `__DIR__`) and the span → line mapping.
    fn lower_options<'l>(&self, line_of: &'l dyn Fn(Span) -> u32) -> LowerOptions<'l> {
        let mut lo = LowerOptions::new(line_of);
        match &self.file {
            Some(p) => lo.file = Some(p.clone()),
            None => {
                lo.eval_name = Some("Command line code".to_string());
                lo.dir = Some(std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
            }
        }
        lo
    }
}

impl<'a> CompileOptions<'a> {
    /// Options without line tables or a file.
    pub fn new() -> CompileOptions<'a> {
        CompileOptions::default()
    }
}

/// The declarations PHP hoists ahead of `{main}`, from [`Hir::hoisted`]:
/// every top-level function, and the top-level classes php binds early —
/// those without parent/interfaces/traits ([`rphp_hir::ClassHoist::Early`])
/// and, in the same source order, those with only a parent
/// ([`rphp_hir::ClassHoist::TryEarly`]: php binds them iff the parent is
/// known when the declaration is compiled, and this unit's parents are all
/// numbered by the class pre-pass). Everything else (`implements`, traits,
/// enums, nested or conditional declarations) is declared when its
/// statement executes.
fn hoisted<'a>(hir: &'a Hir, interner: &Interner) -> (Vec<&'a FuncDecl>, Vec<&'a ClassLike>) {
    let h = hir.hoisted();
    // php early-binds a `class B extends A` only when `A` is already known
    // when the file is compiled. A parent declared *in this unit* is known
    // only if it was itself early-bound and appears earlier in the file — the
    // rule is transitive, because an `InOrder` class (one with `implements`,
    // `use` or an enum) is not declared until its statement runs. Hoisting a
    // child of one would leave `load_unit`'s pending loop unable to progress
    // and fail with `Class "A" not found` at load time.
    let key = |name: &[u8]| -> Box<[u8]> { name.to_ascii_lowercase().into() };
    let in_unit: HashSet<Box<[u8]>> = top_level_class_likes(hir.program())
        .filter_map(|c| c.name.map(|n| key(interner.resolve(n))))
        .collect();

    let mut early: HashSet<Box<[u8]>> = HashSet::new();
    let mut try_early_ok: Vec<&ClassLike> = Vec::new();
    for c in top_level_class_likes(hir.program()) {
        let own = c.name.map(|n| key(interner.resolve(n)));
        match rphp_hir::class_hoist(c) {
            rphp_hir::ClassHoist::Early => {
                if let Some(k) = own {
                    early.insert(k);
                }
            }
            rphp_hir::ClassHoist::TryEarly => {
                // `extends` holds exactly one name for a class.
                let parent = c.extends.first().map(|n| key(&class::class_fqn(n, interner)));
                let known = match &parent {
                    // Not declared here: an already-loaded class (`Exception`)
                    // that `load_unit` resolves against the class table.
                    Some(p) => !in_unit.contains(p) || early.contains(p),
                    None => true,
                };
                if known {
                    try_early_ok.push(c);
                    if let Some(k) = own {
                        early.insert(k);
                    }
                }
            }
            rphp_hir::ClassHoist::InOrder => {}
        }
    }

    let mut classes: Vec<&ClassLike> = h.classes.iter().copied().chain(try_early_ok).collect();
    classes.sort_by_key(|c| c.span.lo);
    (h.funcs, classes)
}

/// Every named class-like declared at the top level of the unit, in source
/// order (the only ones `hoisted` may consider).
fn top_level_class_likes(program: &Program) -> impl Iterator<Item = &ClassLike> {
    program.items.iter().filter_map(|s| match s {
        Stmt::ClassLike(c) => Some(c),
        _ => None,
    })
}

/// Whether a top-level class-like is declared in statement order rather
/// than hoisted (see [`hoisted`]).
pub(crate) fn class_is_hoisted(mx: &func::ModuleCtx<'_>, c: &ClassLike) -> bool {
    mx.hoisted_classes.contains(&(c as *const ClassLike))
}

/// Lower a parsed program to its HIR and compile that into a bytecode
/// module. Function `0` is the synthetic `{main}` entry containing the
/// top-level statements; hoisted functions and classes are declared by the
/// runtime before `{main}` runs (`Module::hoist_funcs` / `hoist_classes`),
/// conditional ones by their `DeclareFunction` / `DeclareClass` ops.
///
/// The interner is mutable because resolution interns the names it
/// synthesizes (FQNs, folded magic constants). An HIR error (php's
/// compile-time fatal: `Cannot use X as Y because the name is already in
/// use`, an invalid `break` level, …) is returned as the diagnostics of
/// that pass, before lowering; HIR warnings are carried along with the
/// compiler's own diagnostics.
pub fn compile(
    program: Program,
    interner: &mut Interner,
    opts: &CompileOptions<'_>,
) -> Result<Module, Vec<Diagnostic>> {
    let line_of_span = |s: Span| opts.line_of.map_or(0, |f| f(s.lo));
    let lower_opts = opts.lower_options(&line_of_span);
    let (hir, mut diags) = rphp_hir::lower(program, interner, &lower_opts);
    if diags.iter().any(Diagnostic::is_error) {
        return Err(diags);
    }
    let program = hir.program();

    // The class pre-pass interns the names of the anonymous classes, so it
    // runs while the interner is still mutable.
    let (unit_file, _) = crate::func::unit_file(opts);
    let (class_map, class_ids, anon_names) =
        class::collect_class_ids(program, interner, &unit_file, opts.line_of);

    let interner: &Interner = interner;
    let (func_decls, class_decls) = hoisted(&hir, interner);

    let mut mx = ModuleCtx::new(
        interner,
        &class_map,
        &class_ids,
        &anon_names,
        class_ids.len(),
        program.strict_types,
        opts,
    );
    mx.hoisted_classes = class_decls.iter().map(|c| *c as *const ClassLike).collect();
    let mx = mx;

    // Hoisted functions first, so their ids come right after `{main}`. A
    // duplicate declaration is php's runtime fatal (`Cannot redeclare …`),
    // raised when the runtime declares the second one.
    let mut hoist_funcs: Vec<FuncId> = Vec::new();
    for f in &func_decls {
        hoist_funcs.push(compile_nested_function(&mx, &mut diags, f));
    }
    let mut hoist_classes = Vec::new();
    for c in &class_decls {
        if let Some(id) = class::compile_class(&mx, &mut diags, c) {
            hoist_classes.push(id);
        }
    }

    // Function 0: synthetic `{main}`: the top-level statements (hoisted
    // declarations emit no code there).
    let main = compile_main(&mx, &mut diags, program);

    if diags.iter().any(Diagnostic::is_error) {
        return Err(diags);
    }
    let sink = mx.sink.into_inner();
    let mut funcs = sink.funcs;
    funcs[0] = main;
    let classes = sink
        .classes
        .into_iter()
        .map(|c| c.unwrap_or_else(|| rphp_bytecode::Class::new_minimal(rphp_intern::IdentId(0), b"")))
        .collect();
    let file: Rc<str> = Rc::from(String::from_utf8_lossy(&mx.file).as_ref());
    Ok(Module {
        funcs,
        classes,
        main: 0,
        hoist_funcs,
        hoist_classes,
        file,
    })
}

/// Compile a named function declaration (hoisted or conditional) into the
/// sink, returning its id.
pub(crate) fn compile_nested_function(
    mx: &ModuleCtx<'_>,
    diags: &mut Vec<Diagnostic>,
    f: &FuncDecl,
) -> FuncId {
    compile_function(
        mx,
        diags,
        FnSpec {
            name: f.name,
            params: &f.params,
            body: &f.body,
            span: f.span,
            by_ref: f.by_ref,
            cur_class: None,
            is_static: false,
            ret: f.ret.as_ref(),
        },
    )
}

/// `{main}`: the program's statement list compiled with top-level
/// declarations recognised as already hoisted. It is a `NEEDS_SYMTAB`
/// frame (its variables are the globals) and returns `1` when it falls off
/// the end, the value an `include` of the file evaluates to.
fn compile_main(
    mx: &ModuleCtx<'_>,
    diags: &mut Vec<Diagnostic>,
    program: &Program,
) -> rphp_bytecode::Function {
    let mut fc = func::FnCompiler::new(
        mx,
        diags,
        &[],
        &[],
        ClosureBody::Stmts(&program.items),
        None,
        Box::from(&b""[..]),
    );
    fc.at_top_level = true;
    fc.is_main = true;
    fc.flags |= rphp_bytecode::FnFlags::NEEDS_SYMTAB;
    fc.emit(Op::BindSymtab);
    fc.bind_auto_globals();
    fc.compile_stmts(&program.items);
    let k = fc.push_const(Const::Int(1));
    let one = fc.alloc_temp();
    fc.emit(Op::LoadConst { dst: one, k });
    fc.emit(Op::Ret { src: Some(one) });
    fc.finish(Box::from(&b""[..]), Vec::new(), Span::dummy())
}
