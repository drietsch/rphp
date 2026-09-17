//! Lower `rphp_ast::v2` to `rphp-bytecode` (the v2 contract: `Init*`/`Send*`/
//! `DoCall` calls, late-bound names, references and fetch-for-write chains,
//! control flow, operators).
//!
//! The lowering is a straightforward tree-walk into three-address register
//! bytecode (`rphp-bytecode`). Function `0` is the synthetic `{main}`; every
//! other function (hoisted or conditional declarations, methods, closures,
//! default-value thunks) is appended through a sink as it is compiled, so
//! [`FuncId`]s are known before bodies finish. Classes are numbered by a
//! pre-pass over the whole unit ([`class::collect_class_ids`]) so `extends`
//! can name a class declared later or conditionally. Every name a program
//! *uses* — functions, classes, constants — stays a name in the bytecode
//! (`Const::Name`) and is resolved by the runtime (plan D5): the compiler no
//! longer depends on the native registry.
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

#[cfg(test)]
mod tests;

use std::path::PathBuf;
use std::rc::Rc;

use rphp_ast::v2::{ClassLike, FuncDecl, Program, Stmt};
use rphp_bytecode::{Const, FuncId, Module, Op};
use rphp_diagnostics::{codes, Diagnostic};
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

/// Whether a written name resolves as spelled: every unit compiled today is
/// in the global namespace (namespace declarations are not lowered), so an
/// unqualified, qualified (`Foo\Bar`), fully qualified (`\Foo\Bar`) or
/// `namespace\`-relative name all denote the global symbol with that
/// spelling. Namespaced *resolution* (imports, the two-step fallback) is F4.
pub(crate) fn name_is_global(name: &rphp_ast::v2::Name) -> bool {
    matches!(
        name.kind,
        rphp_ast::v2::NameKind::Unqualified
            | rphp_ast::v2::NameKind::Qualified
            | rphp_ast::v2::NameKind::FullyQualified
            | rphp_ast::v2::NameKind::Relative
    )
}

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
    /// `Function::lines` and `__LINE__`; `None` leaves the line tables empty.
    pub line_of: Option<&'a dyn Fn(u32) -> u32>,
    /// The script's path as php reports it (`__FILE__`; `__DIR__` is its
    /// parent). `None` for `-r`/eval code (`Command line code`, cwd).
    pub file: Option<PathBuf>,
}

impl<'a> CompileOptions<'a> {
    /// Options without line tables or a file.
    pub fn new() -> CompileOptions<'a> {
        CompileOptions::default()
    }
}

/// The declarations PHP hoists: functions and classes directly in the
/// top-level statement list, in plain `{ }` blocks at that level, and in the
/// global `namespace { }` body — the statements `zend_compile_top_stmt`
/// reaches. Named namespaces are left alone (reported as `RPHP_E0300` when
/// `{main}` is compiled).
fn hoisted<'a>(stmts: &'a [Stmt], funcs: &mut Vec<&'a FuncDecl>, classes: &mut Vec<&'a ClassLike>) {
    for s in stmts {
        match s {
            Stmt::Func(f) => funcs.push(f),
            Stmt::ClassLike(c) => classes.push(c),
            Stmt::Block { body, .. }
            | Stmt::Namespace {
                name: None, body, ..
            } => hoisted(body, funcs, classes),
            _ => {}
        }
    }
}

/// Compile a parsed program into a bytecode module. Function `0` is the
/// synthetic `{main}` entry containing the top-level statements; hoisted
/// functions and classes are declared by the runtime before `{main}` runs
/// (`Module::hoist_funcs` / `hoist_classes`), conditional ones by their
/// `DeclareFunction` / `DeclareClass` ops.
pub fn compile(
    program: &Program,
    interner: &Interner,
    opts: &CompileOptions<'_>,
) -> Result<Module, Vec<Diagnostic>> {
    let mut diags: Vec<Diagnostic> = Vec::new();

    let mut func_decls: Vec<&FuncDecl> = Vec::new();
    let mut class_decls: Vec<&ClassLike> = Vec::new();
    hoisted(&program.items, &mut func_decls, &mut class_decls);

    let (class_map, class_ids) = class::collect_class_ids(program, interner);
    let mx = ModuleCtx::new(
        interner,
        &class_map,
        &class_ids,
        class_ids.len(),
        program.strict_types,
        opts,
    );

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
        .map(|c| c.unwrap_or_else(|| rphp_bytecode::Class {
            name: rphp_intern::IdentId(0),
            name_bytes: Box::from(&b""[..]),
            parent: None,
            props: Vec::new(),
            methods: Vec::new(),
            line: 0,
        }))
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
    fc.compile_stmts(&program.items);
    let k = fc.push_const(Const::Int(1));
    let one = fc.alloc_temp();
    fc.emit(Op::LoadConst { dst: one, k });
    fc.emit(Op::Ret { src: Some(one) });
    fc.finish(Box::from(&b""[..]), Vec::new(), Span::dummy())
}
