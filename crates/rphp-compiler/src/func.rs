//! Per-function lowering state ([`FnCompiler`]), the function/closure drivers
//! and the shared low-level emit helpers.
//!
//! Every function body — `{main}`, a named function, a method, a closure —
//! is compiled by one [`FnCompiler`]. Variables get permanent registers up
//! front (params, then captured `use` variables, then every other variable in
//! first-use order, via [`crate::regs`]); temporaries are a stack above them.

use std::collections::HashMap;

use rphp_ast::v2::{Closure, Expr, Param, Stmt};
use rphp_bytecode::{ClassId, ClosureProto, CodeAddr, Const, FuncId, Function, Op, Reg};
use rphp_diagnostics::{codes, Diagnostic};
use rphp_intern::{IdentId, Interner};
use rphp_span::Span;
use rphp_value::Str;

use crate::class::ClassCtx;
use crate::{regs, unsupported, CompileOptions, KnownFunctions, NativeSig};

/// What a closure body is: a statement list (`function () { ... }`) or the
/// single returned expression of an arrow function (`fn () => e`).
#[derive(Clone, Copy)]
pub(crate) enum ClosureBody<'a> {
    /// A block body.
    Stmts(&'a [Stmt]),
    /// An arrow function's expression, compiled as `return e;`.
    ReturnExpr(&'a Expr),
}

/// Everything shared by the function compilers of one module.
pub(crate) struct ModuleCtx<'a> {
    pub(crate) interner: &'a Interner,
    /// Top-level user functions by name.
    pub(crate) func_map: &'a HashMap<IdentId, FuncId>,
    pub(crate) class_ctx: &'a ClassCtx<'a>,
    /// Declared parameter counts indexed by [`FuncId`].
    pub(crate) arities: &'a [u16],
    /// The natives call sites may bind to.
    pub(crate) natives: &'a dyn KnownFunctions,
    /// Byte offset → 1-based line, when line tables are wanted.
    pub(crate) line_of: Option<&'a dyn Fn(u32) -> u32>,
    /// Ids below this are `{main}`, user functions and methods; closures are
    /// appended after them (`top_level_count + sink index`).
    pub(crate) top_level_count: FuncId,
}

impl<'a> ModuleCtx<'a> {
    /// Build the context from the compile options and the pre-pass tables.
    pub(crate) fn new(
        interner: &'a Interner,
        func_map: &'a HashMap<IdentId, FuncId>,
        class_ctx: &'a ClassCtx<'a>,
        arities: &'a [u16],
        top_level_count: FuncId,
        opts: &CompileOptions<'a>,
    ) -> Self {
        ModuleCtx {
            interner,
            func_map,
            class_ctx,
            arities,
            natives: opts.natives,
            line_of: opts.line_of,
            top_level_count,
        }
    }
}

/// A named body to compile: a user function or a method.
pub(crate) struct FnSpec<'a> {
    pub(crate) name: IdentId,
    pub(crate) params: &'a [Param],
    pub(crate) body: &'a [Stmt],
    pub(crate) span: Span,
    /// The declaring class for a method (`$this` in register 0).
    pub(crate) cur_class: Option<ClassId>,
}

/// Compile a user function or a method to a [`Function`]. A method
/// (`cur_class` set) reserves register 0 for `$this`.
pub(crate) fn compile_function(
    mx: &ModuleCtx<'_>,
    diags: &mut Vec<Diagnostic>,
    closure_sink: &mut Vec<Function>,
    spec: FnSpec<'_>,
) -> Function {
    let FnSpec {
        name,
        params,
        body,
        span,
        cur_class,
    } = spec;
    check_params(params, diags);
    let mut fc = FnCompiler::new(
        mx,
        diags,
        closure_sink,
        params,
        &[],
        ClosureBody::Stmts(body),
        cur_class,
    );
    fc.compile_stmts(body);
    // Always terminate with a fall-through return so every code path (and every
    // branch target that lands at the textual end) has a valid `Ret`.
    fc.emit(Op::Ret { src: None });
    Function {
        name,
        name_bytes: mx.interner.resolve(name).into(),
        // A method's register 0 holds the implicit `$this`, so its declared
        // parameters occupy registers `1 ..= n` and the frame takes `n + 1`.
        num_params: params.len() as u16 + u16::from(cur_class.is_some()),
        num_regs: fc.num_regs,
        code: fc.code,
        consts: fc.consts,
        capture_regs: fc.capture_regs,
        closures: fc.closures,
        span,
        lines: fc.lines,
        ..Function::default()
    }
}

/// Reject the parameter features the slice does not lower yet (defaults,
/// by-reference, variadics, promotion, hooks). Types and attributes are
/// metadata and are ignored.
pub(crate) fn check_params(params: &[Param], diags: &mut Vec<Diagnostic>) {
    for p in params {
        if p.default.is_some() {
            unsupported(diags, p.span, "default parameter value");
        }
        if p.by_ref {
            unsupported(diags, p.span, "by-reference parameter");
        }
        if p.variadic {
            unsupported(diags, p.span, "variadic parameter");
        }
        if p.promote.is_some() {
            unsupported(diags, p.span, "constructor property promotion");
        }
        if !p.hooks.is_empty() {
            unsupported(diags, p.span, "property hooks on a promoted parameter");
        }
    }
}

/// What a call site resolves to during lowering.
pub(crate) enum CallTarget {
    /// A user-defined function, by [`FuncId`].
    User(FuncId),
    /// A native, by the runtime's id (see [`NativeSig`]) with its by-ref mask.
    Native { id: u32, by_ref: u32 },
}

/// The lowering state of one function body.
pub(crate) struct FnCompiler<'a> {
    pub(crate) mx: &'a ModuleCtx<'a>,
    pub(crate) diags: &'a mut Vec<Diagnostic>,
    /// Where nested closures register their compiled `Function`s. Their `FuncId`
    /// is `top_level_count + index` (the sink is appended after the top-level
    /// functions), so ids stay stable as the sink grows.
    pub(crate) closure_sink: &'a mut Vec<Function>,
    /// The class whose method is being compiled (lexical context for `$this`,
    /// `self::`/`parent::`, and visibility); `None` outside a method.
    pub(crate) cur_class: Option<ClassId>,
    /// `true` while compiling statements PHP hoists declarations from (the
    /// `{main}` statement list, plain blocks and global namespace bodies at
    /// that level); a declaration met anywhere else is conditional and is
    /// reported as not lowered.
    pub(crate) at_top_level: bool,

    /// Variable -> permanent register. Variables occupy the low registers
    /// (params first, then captured `use` vars, then locals); temporaries live
    /// above them.
    pub(crate) vars: HashMap<IdentId, Reg>,
    /// Current top of the temporary stack (next free temp register).
    pub(crate) temp_top: Reg,
    /// High-water mark: total registers the frame needs.
    pub(crate) num_regs: Reg,
    /// Registers a closure body binds its captures to, in capture order (empty
    /// for an ordinary function).
    pub(crate) capture_regs: Vec<Reg>,
    /// Closure templates this function emits via `Op::MakeClosure`.
    pub(crate) closures: Vec<ClosureProto>,

    pub(crate) code: Vec<Op>,
    pub(crate) consts: Vec<Const>,
    /// Source line per op, parallel to `code` (empty without `line_of`).
    pub(crate) lines: Vec<u32>,
    /// The line the ops being emitted belong to.
    pub(crate) cur_line: u32,
}

impl<'a> FnCompiler<'a> {
    pub(crate) fn new(
        mx: &'a ModuleCtx<'a>,
        diags: &'a mut Vec<Diagnostic>,
        closure_sink: &'a mut Vec<Function>,
        params: &[Param],
        captures: &[IdentId],
        body: ClosureBody<'_>,
        cur_class: Option<ClassId>,
    ) -> Self {
        // A method reserves register 0 for the implicit `$this`; parameters then
        // start at register 1. (If the body names `$this`, the parser has
        // interned it, so we can bind that id to register 0.)
        let is_method = cur_class.is_some();
        let mut vars: HashMap<IdentId, Reg> = HashMap::new();
        let mut base = 0;
        if is_method {
            if let Some(this_id) = mx.interner.get(b"this") {
                vars.insert(this_id, 0);
            }
            base = 1;
        }
        // Params take registers `base .. base+np`; captured `use` vars follow.
        for (i, p) in params.iter().enumerate() {
            vars.insert(p.name, base + i as Reg);
        }
        let mut var_count = base + params.len() as Reg;
        let mut capture_regs = Vec::with_capacity(captures.len());
        for &c in captures {
            let reg = var_count;
            // A capture may shadow nothing here; if it repeats a param, keep the
            // param's slot (degenerate, but avoids a duplicate register).
            vars.entry(c).or_insert(reg);
            let reg = vars[&c];
            capture_regs.push(reg);
            if reg == var_count {
                var_count += 1;
            }
        }
        // Pre-scan the body so every variable has a permanent register before
        // any temporary is allocated.
        regs::collect_body(body, &mut vars, &mut var_count);

        FnCompiler {
            mx,
            diags,
            closure_sink,
            cur_class,
            at_top_level: false,
            vars,
            temp_top: var_count,
            num_regs: var_count,
            capture_regs,
            closures: Vec::new(),
            code: Vec::new(),
            consts: Vec::new(),
            lines: Vec::new(),
            cur_line: 0,
        }
    }

    // ---- low-level helpers --------------------------------------------------

    pub(crate) fn interner(&self) -> &'a Interner {
        self.mx.interner
    }

    pub(crate) fn emit(&mut self, op: Op) -> usize {
        self.code.push(op);
        if self.mx.line_of.is_some() {
            self.lines.push(self.cur_line);
        }
        self.code.len() - 1
    }

    /// Record the source line the following ops belong to.
    pub(crate) fn mark_line(&mut self, span: Span) {
        if let Some(line_of) = self.mx.line_of {
            self.cur_line = line_of(span.lo);
        }
    }

    pub(crate) fn here(&self) -> CodeAddr {
        self.code.len() as CodeAddr
    }

    pub(crate) fn patch(&mut self, idx: usize, target: CodeAddr) {
        match &mut self.code[idx] {
            Op::Jmp { target: t }
            | Op::JmpIfTrue { target: t, .. }
            | Op::JmpIfFalse { target: t, .. }
            | Op::ForeachNext { target: t, .. } => *t = target,
            _ => unreachable!("patch on a non-branch op"),
        }
    }

    pub(crate) fn set_top(&mut self, n: Reg) {
        self.temp_top = n;
        if n > self.num_regs {
            self.num_regs = n;
        }
    }

    /// Allocate a fresh temporary register.
    pub(crate) fn alloc_temp(&mut self) -> Reg {
        let r = self.temp_top;
        self.set_top(self.temp_top + 1);
        r
    }

    /// Release temporaries down to `mark` (does not lower the high-water mark).
    pub(crate) fn free_to(&mut self, mark: Reg) {
        self.temp_top = mark;
    }

    pub(crate) fn push_const(&mut self, c: Const) -> u32 {
        let k = self.consts.len() as u32;
        self.consts.push(c);
        k
    }

    /// Intern a member name (property / method) as a string constant in the
    /// pool, returning its index — the form `PropGet`/`PropSet`/`MethodCall` use.
    pub(crate) fn name_const(&mut self, name: IdentId) -> u32 {
        self.push_const(Const::Str(Str::new(self.mx.interner.resolve(name))))
    }

    pub(crate) fn var_reg(&self, id: IdentId) -> Reg {
        *self
            .vars
            .get(&id)
            .expect("every variable is assigned a register during the pre-scan")
    }

    /// Report an unsupported construct and yield a `null` temporary so
    /// lowering can go on collecting diagnostics.
    pub(crate) fn unsupported_expr(&mut self, span: Span, what: &str) -> Reg {
        unsupported(self.diags, span, what);
        self.null_temp()
    }

    /// A fresh temporary holding `null` (the value of every error path).
    pub(crate) fn null_temp(&mut self) -> Reg {
        let dst = self.alloc_temp();
        self.emit(Op::LoadNull { dst });
        dst
    }

    // ---- arity checks -------------------------------------------------------

    /// A user function takes a fixed parameter count (defaults/variadics are not
    /// modelled yet), so the arg count must match exactly.
    pub(crate) fn check_user_arity(&mut self, name: IdentId, id: FuncId, argc: u16, span: Span) {
        let expected = self.mx.arities[id as usize];
        if argc != expected {
            self.diags.push(
                Diagnostic::error(
                    codes::WRONG_ARG_COUNT,
                    format!(
                        "function {}() expects {} argument(s), {} given",
                        self.mx.interner.resolve_lossy(name),
                        expected,
                        argc
                    ),
                )
                .with_primary(span, "wrong number of arguments"),
            );
        }
    }

    /// A builtin declares an arity range (`min_args ..= max_args`, `None` upper
    /// bound meaning variadic); range-check the call site against it.
    pub(crate) fn check_native_arity(
        &mut self,
        name: IdentId,
        desc: &NativeSig,
        argc: u16,
        span: Span,
    ) {
        let argc = argc as usize;
        let too_few = argc < desc.min_args;
        let too_many = desc.max_args.is_some_and(|max| argc > max);
        if too_few || too_many {
            let want = match desc.max_args {
                Some(max) if max == desc.min_args => format!("exactly {}", desc.min_args),
                Some(max) => format!("{} to {}", desc.min_args, max),
                None => format!("at least {}", desc.min_args),
            };
            self.diags.push(
                Diagnostic::error(
                    codes::WRONG_ARG_COUNT,
                    format!(
                        "function {}() expects {want} argument(s), {argc} given",
                        self.mx.interner.resolve_lossy(name),
                    ),
                )
                .with_primary(span, "wrong number of arguments"),
            );
        }
    }

    // ---- closures -------------------------------------------------------------

    /// Lower `function (...) use (...) { ... }`: capture the current values of
    /// its `use` variables from this frame, compile its body as its own
    /// function, and emit a `MakeClosure` that binds them together at runtime.
    pub(crate) fn compile_closure_expr(&mut self, c: &Closure) -> Reg {
        if c.static_ {
            unsupported(self.diags, c.span, "static closure");
        }
        if c.by_ref {
            unsupported(self.diags, c.span, "closure returning by reference");
        }
        for u in &c.uses {
            if u.by_ref {
                unsupported(self.diags, u.span, "by-reference closure capture");
            }
        }
        let uses: Vec<IdentId> = c.uses.iter().map(|u| u.name).collect();
        self.compile_closure(&c.params, &uses, ClosureBody::Stmts(&c.body), c.span)
    }

    /// Lower `fn (...) => e`: the free variables of `e` are captured by value
    /// (auto-capture) and the body is `return e;`.
    pub(crate) fn compile_arrow_fn(&mut self, f: &rphp_ast::v2::ArrowFn) -> Reg {
        if f.static_ {
            unsupported(self.diags, f.span, "static arrow function");
        }
        if f.by_ref {
            unsupported(self.diags, f.span, "arrow function returning by reference");
        }
        let uses = regs::arrow_free_vars(f);
        self.compile_closure(&f.params, &uses, ClosureBody::ReturnExpr(&f.body), f.span)
    }

    fn compile_closure(
        &mut self,
        params: &[Param],
        uses: &[IdentId],
        body: ClosureBody<'_>,
        span: Span,
    ) -> Reg {
        // Registers in *this* frame holding the captured variables' current
        // values (snapshotted by value when the closure is built).
        let src_regs: Vec<Reg> = uses.iter().map(|u| self.var_reg(*u)).collect();
        let closure_fn = self.compile_closure_fn(params, uses, body, span);
        // FuncId = top-level count + position in the sink (nested closures of
        // this body were already pushed during compilation).
        let func = self.mx.top_level_count + self.closure_sink.len() as FuncId;
        self.closure_sink.push(closure_fn);
        let proto = self.closures.len() as u32;
        self.closures.push(ClosureProto { func, src_regs });
        let dst = self.alloc_temp();
        self.emit(Op::MakeClosure { dst, proto });
        dst
    }

    /// Compile a closure body to a `Function` (a sub-compiler sharing the sink).
    fn compile_closure_fn(
        &mut self,
        params: &[Param],
        uses: &[IdentId],
        body: ClosureBody<'_>,
        span: Span,
    ) -> Function {
        check_params(params, self.diags);
        let mut fc = FnCompiler::new(
            self.mx,
            &mut *self.diags,
            &mut *self.closure_sink,
            params,
            uses,
            body,
            None,
        );
        match body {
            ClosureBody::Stmts(stmts) => fc.compile_stmts(stmts),
            ClosureBody::ReturnExpr(e) => {
                fc.mark_line(span);
                let mark = fc.temp_top;
                let r = fc.compile_expr(e);
                fc.emit(Op::Ret { src: Some(r) });
                fc.free_to(mark);
            }
        }
        fc.emit(Op::Ret { src: None });
        Function {
            name: IdentId(0),
            name_bytes: Box::from(&b""[..]),
            num_params: params.len() as u16,
            num_regs: fc.num_regs,
            code: fc.code,
            consts: fc.consts,
            capture_regs: fc.capture_regs,
            closures: fc.closures,
            span,
            lines: fc.lines,
            ..Function::default()
        }
    }
}
