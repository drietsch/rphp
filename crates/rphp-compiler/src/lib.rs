//! Lower `rphp_ast::v2` to `rphp-bytecode` for the current language slice.
//!
//! The lowering is a straightforward tree-walk into three-address register
//! bytecode (`rphp-bytecode`). A pre-pass assigns every top-level function a
//! [`FuncId`] (the synthetic `{main}` is `0`, user functions follow in
//! declaration order, then the methods of every class, then the closures
//! discovered while compiling bodies) so calls resolve regardless of source
//! order. Each function then pre-scans its body to give every variable a
//! permanent register; intermediate results use a stack of temporaries
//! allocated above the variable region.
//!
//! The input is the full PHP 8.5 tree (roadmap F1/F2). What the compiler
//! lowers today is the M0 slice described in [`stmt`], [`expr`] and
//! [`class`]; every other node, variant or flag is reported as
//! `RPHP_E0300 unsupported construct: <what> (not lowered yet)` with the
//! node's span, through the single [`unsupported`] helper, so a program
//! outside the slice fails with a precise list rather than mis-compiling.
//! Types on parameters, properties and returns, attributes and doc comments
//! are metadata the slice ignores (they neither fail nor change lowering).
//!
//! Errors (undefined function, wrong argument count, duplicate declaration,
//! unsupported construct) are collected as [`Diagnostic`]s; the compile fails
//! iff any of them is an error.
//!
//! | module   | contents                                                  |
//! |----------|-----------------------------------------------------------|
//! | [`func`] | `FnCompiler`, function/closure drivers, emit helpers      |
//! | [`stmt`] | statement lowering                                        |
//! | [`expr`] | expression lowering                                       |
//! | [`class`]| class pre-pass, class tables, `Class` assembly            |
//! | [`regs`] | register pre-scan and arrow-function capture visitors     |
#![forbid(unsafe_code)]

mod class;
mod expr;
mod func;
mod regs;
mod stmt;

#[cfg(test)]
mod tests;

use std::collections::HashMap;

use rphp_ast::v2::{ClassLike, FuncDecl, Program, Stmt};
use rphp_bytecode::{ClassId, FuncId, Function, Module};
use rphp_diagnostics::{codes, Diagnostic};
use rphp_intern::{IdentId, Interner};
use rphp_span::Span;

use class::ClassCtx;
use func::{compile_function, FnSpec, ModuleCtx};

/// Diagnostic code for a duplicate function declaration. `rphp-diagnostics`
/// does not (yet) expose a shared constant for this, so the compiler owns it.
pub const REDECLARED_FUNCTION: &str = "RPHP_E0102";
/// Writing through a nested subscript (`$a[i][j] = v`) is not lowered yet.
pub const NESTED_ARRAY_WRITE: &str = "RPHP_E0103";
/// Reading `$a[]` (the empty-subscript append form) is not a valid expression.
pub const INVALID_APPEND_READ: &str = "RPHP_E0104";
/// A by-reference parameter (e.g. `sort($a)`) was passed a non-variable.
pub const BY_REF_NOT_VARIABLE: &str = "RPHP_E0105";
/// A duplicate class declaration.
pub const REDECLARED_CLASS: &str = "RPHP_E0106";
/// `new Foo(...)` where `Foo` is not a declared class.
pub const UNDEFINED_CLASS: &str = "RPHP_E0107";
/// A property default that is not a constant expression.
pub const NON_CONST_PROP_DEFAULT: &str = "RPHP_E0108";
/// A scoped call (`self::m()` / `parent::m()` / `Class::m()`) to a method that
/// does not exist.
pub const UNDEFINED_METHOD: &str = "RPHP_E0109";
/// `self::`/`parent::` used outside a class, or `parent::` with no parent.
pub const INVALID_SCOPE: &str = "RPHP_E0110";
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

/// The arity/by-ref shape of a native the runtime provides, as the compiler
/// needs it: `id` is the runtime's process-local `NativeId` (baked into
/// `Op::CallNative` — valid only for the interpreter it was resolved against,
/// ADR-014), the rest drives the call-site range check and the by-reference
/// write-back lowering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeSig {
    /// The runtime's native id.
    pub id: u32,
    /// The minimum argument count.
    pub min_args: usize,
    /// The maximum argument count; `None` = variadic.
    pub max_args: Option<usize>,
    /// Bitmask of by-reference parameter positions.
    pub by_ref: u32,
}

/// The set of native functions a call site may bind to — supplied by the
/// embedder (rphp-embed builds it from the interpreter's registry) so the
/// compiler has no dependency on any extension crate.
pub trait KnownFunctions {
    /// Look a (case-insensitive) function name up.
    fn native(&self, name: &[u8]) -> Option<NativeSig>;
}

/// A [`KnownFunctions`] that knows no natives (every builtin call is
/// "undefined function").
pub struct NoNatives;

impl KnownFunctions for NoNatives {
    fn native(&self, _name: &[u8]) -> Option<NativeSig> {
        None
    }
}

/// What [`compile`] needs besides the program.
pub struct CompileOptions<'a> {
    /// The natives call sites may bind to.
    pub natives: &'a dyn KnownFunctions,
    /// Maps a byte offset in the source to its 1-based line, for
    /// `Function::lines`; `None` leaves the line tables empty.
    pub line_of: Option<&'a dyn Fn(u32) -> u32>,
}

impl<'a> CompileOptions<'a> {
    /// Options binding natives through `natives`, without line tables.
    pub fn new(natives: &'a dyn KnownFunctions) -> CompileOptions<'a> {
        CompileOptions {
            natives,
            line_of: None,
        }
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
/// synthetic `{main}` entry containing the top-level statements; each hoisted
/// function becomes its own [`Function`] appended afterwards, then the
/// methods of every class, then the closures.
pub fn compile(
    program: &Program,
    interner: &Interner,
    opts: &CompileOptions<'_>,
) -> Result<Module, Vec<Diagnostic>> {
    let mut diags: Vec<Diagnostic> = Vec::new();

    let mut func_decls: Vec<&FuncDecl> = Vec::new();
    let mut class_decls: Vec<&ClassLike> = Vec::new();
    hoisted(&program.items, &mut func_decls, &mut class_decls);

    // ---- pre-pass A: function ids (main = 0, user funcs 1..=U) ----
    let mut func_map: HashMap<IdentId, FuncId> = HashMap::new();
    let mut user_funcs: Vec<&FuncDecl> = Vec::new();
    // Argument counts indexed by FuncId; index 0 is `{main}` (never called).
    let mut arities: Vec<u16> = vec![0];
    let mut next_id: FuncId = 1;
    for f in func_decls {
        if f.by_ref {
            unsupported(&mut diags, f.span, "function returning by reference");
        }
        if func_map.contains_key(&f.name) {
            diags.push(
                Diagnostic::error(
                    REDECLARED_FUNCTION,
                    format!(
                        "cannot redeclare function {}()",
                        interner.resolve_lossy(f.name)
                    ),
                )
                .with_primary(f.span, "duplicate declaration"),
            );
            continue;
        }
        func_map.insert(f.name, next_id);
        user_funcs.push(f);
        arities.push(f.params.len() as u16);
        next_id += 1;
    }

    // ---- pre-pass B: class ids + method func ids (U+1 ..= U+M) ----
    // Methods compile to ordinary functions appended right after the user
    // functions. Their ids are fixed *before* any body is compiled, so closure
    // ids (`top_level_count + sink index`) stay stable as bodies are lowered.
    let (classes, method_ids, class_map) =
        class::collect_classes(&class_decls, interner, next_id, &mut diags);
    let parent_id = class::resolve_parents(&classes, &class_map, interner, &mut diags);
    let class_has_ctor = class::constructor_chain(&classes, &parent_id, interner);
    let methods_ct = class::method_tables(&classes, &method_ids, interner);
    let class_ctx = ClassCtx {
        map: &class_map,
        has_ctor: &class_has_ctor,
        parent: &parent_id,
        methods: &methods_ct,
    };
    let total_methods: usize = method_ids.iter().map(Vec::len).sum();
    // Closures discovered while compiling bodies are appended after main, the
    // user functions, and the methods; their FuncId is `top_level_count + sink`.
    let top_level_count = (user_funcs.len() + 1 + total_methods) as FuncId;
    let mx = ModuleCtx::new(
        interner,
        &func_map,
        &class_ctx,
        &arities,
        top_level_count,
        opts,
    );
    let mut funcs: Vec<Function> = Vec::with_capacity(top_level_count as usize);
    let mut closure_sink: Vec<Function> = Vec::new();

    // Function 0: synthetic `{main}`: the top-level statements (declarations
    // emit no code there).
    funcs.push(compile_main(&mx, &mut diags, &mut closure_sink, program));
    for f in &user_funcs {
        funcs.push(compile_function(
            &mx,
            &mut diags,
            &mut closure_sink,
            FnSpec {
                name: f.name,
                params: &f.params,
                body: &f.body,
                span: f.span,
                cur_class: None,
            },
        ));
    }
    // Methods, in the same class-then-declaration order as pre-pass B so each
    // lands at exactly the FuncId reserved for it. Each carries its class as the
    // lexical context (`cur_class`) for `$this`, `self::`/`parent::`, visibility.
    for (ci, c) in classes.iter().enumerate() {
        for m in &c.methods {
            funcs.push(compile_function(
                &mx,
                &mut diags,
                &mut closure_sink,
                FnSpec {
                    name: m.decl.name,
                    params: &m.decl.params,
                    body: m.body,
                    span: m.decl.span,
                    cur_class: Some(ci as ClassId),
                },
            ));
        }
    }

    let bc_classes = class::lower_classes(&classes, &method_ids, &parent_id, interner, &mut diags);

    if diags.iter().any(Diagnostic::is_error) {
        return Err(diags);
    }
    // Append the compiled closures so `FuncId`s line up with their indices.
    funcs.extend(closure_sink);
    Ok(Module {
        funcs,
        classes: bc_classes,
        main: 0,
    })
}

/// `{main}`: the program's statement list compiled with top-level
/// declarations recognised as already hoisted.
fn compile_main(
    mx: &ModuleCtx<'_>,
    diags: &mut Vec<Diagnostic>,
    closure_sink: &mut Vec<Function>,
    program: &Program,
) -> Function {
    let mut fc = func::FnCompiler::new(
        mx,
        diags,
        closure_sink,
        &[],
        &[],
        func::ClosureBody::Stmts(&program.items),
        None,
    );
    fc.at_top_level = true;
    fc.compile_stmts(&program.items);
    fc.emit(rphp_bytecode::Op::Ret { src: None });
    Function {
        name: IdentId(0),
        name_bytes: Box::from(&b""[..]),
        num_params: 0,
        num_regs: fc.num_regs,
        code: fc.code,
        consts: fc.consts,
        capture_regs: fc.capture_regs,
        closures: fc.closures,
        span: Span::dummy(),
        lines: fc.lines,
        ..Function::default()
    }
}
