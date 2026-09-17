//! Per-function lowering state ([`FnCompiler`]), the function / closure /
//! thunk drivers and the shared low-level emit helpers.
//!
//! Every function body — `{main}`, a named function, a method, a closure, a
//! default-value thunk — is compiled by one [`FnCompiler`]. Variables get
//! permanent registers up front (params in `0 .. n`, then captured `use`
//! variables, then every other variable in first-use order, via
//! [`crate::regs`]); temporaries are a stack above them. `$this` is a frame
//! slot read with `LoadThis` into its own register at every use.
//!
//! Compiled functions are appended to the module through a *sink*
//! ([`FnSink`]) so their [`FuncId`]s are known before their bodies are done
//! (a `DeclareFunction`/`MakeClosure` op can name them).

use std::cell::RefCell;
use std::collections::HashMap;

use rphp_ast::v2::{Closure, Expr, Param, Stmt};
use rphp_bytecode::{
    CaptureDesc, Class as BcClass, ClassId, CodeAddr, Const, FnFlags, FuncId, Function, InitRef,
    NameConst, Op, ParamDef, Reg, StaticVar,
};
use rphp_diagnostics::Diagnostic;
use rphp_intern::{IdentId, Interner};
use rphp_span::Span;
use rphp_value::Str;

use crate::{regs, unsupported, CompileOptions};

/// What a closure body is: a statement list (`function () { ... }`) or the
/// single returned expression of an arrow function (`fn () => e`).
#[derive(Clone, Copy)]
pub(crate) enum ClosureBody<'a> {
    /// A block body.
    Stmts(&'a [Stmt]),
    /// An arrow function's expression, compiled as `return e;`.
    ReturnExpr(&'a Expr),
}

/// The module-wide function and class sinks. Functions are reserved (an id is
/// handed out and a placeholder pushed) and filled once compiled, so a body can
/// reference its own or a nested function's id before it is complete.
pub(crate) struct FnSink {
    /// `funcs[0]` is `{main}`; everything else is appended here.
    pub(crate) funcs: Vec<Function>,
    /// Classes by pre-assigned [`ClassId`] (`None` until compiled).
    pub(crate) classes: Vec<Option<BcClass>>,
}

impl FnSink {
    /// Reserve the next [`FuncId`] with a placeholder.
    pub(crate) fn reserve(&mut self) -> FuncId {
        let id = self.funcs.len() as FuncId;
        self.funcs.push(Function::default());
        id
    }

    /// Fill a reserved slot.
    pub(crate) fn fill(&mut self, id: FuncId, f: Function) {
        self.funcs[id as usize] = f;
    }
}

/// Everything shared by the function compilers of one module.
pub(crate) struct ModuleCtx<'a> {
    pub(crate) interner: &'a Interner,
    /// Class name (lowercased bytes) → pre-assigned id, for `extends`
    /// resolution and `DeclareClass`.
    pub(crate) class_map: &'a HashMap<Box<[u8]>, ClassId>,
    /// Every class-like declaration of the unit by pointer identity → its id.
    pub(crate) class_ids: &'a HashMap<*const rphp_ast::v2::ClassLike, ClassId>,
    /// Byte offset → 1-based line, when line tables are wanted.
    pub(crate) line_of: Option<&'a dyn Fn(u32) -> u32>,
    /// `__FILE__`.
    pub(crate) file: Box<[u8]>,
    /// `__DIR__`.
    pub(crate) dir: Box<[u8]>,
    /// `declare(strict_types=1)` at the top of the unit.
    pub(crate) strict_types: bool,
    /// The function/class sinks.
    pub(crate) sink: RefCell<FnSink>,
}

impl<'a> ModuleCtx<'a> {
    /// Build the context from the compile options and the class pre-pass.
    pub(crate) fn new(
        interner: &'a Interner,
        class_map: &'a HashMap<Box<[u8]>, ClassId>,
        class_ids: &'a HashMap<*const rphp_ast::v2::ClassLike, ClassId>,
        n_classes: usize,
        strict_types: bool,
        opts: &CompileOptions<'a>,
    ) -> Self {
        let (file, dir): (Box<[u8]>, Box<[u8]>) = match &opts.file {
            Some(p) => {
                let file = p.to_string_lossy().into_owned();
                let dir = p
                    .parent()
                    .map(|d| d.to_string_lossy().into_owned())
                    .filter(|d| !d.is_empty())
                    .unwrap_or_else(|| ".".to_string());
                (file.into_bytes().into(), dir.into_bytes().into())
            }
            None => {
                // php -r: `__FILE__` is the unit name and `__DIR__` the cwd.
                let dir = std::env::current_dir()
                    .map(|d| d.to_string_lossy().into_owned())
                    .unwrap_or_else(|_| ".".to_string());
                (
                    Box::from(&b"Command line code"[..]),
                    dir.into_bytes().into(),
                )
            }
        };
        ModuleCtx {
            interner,
            class_map,
            class_ids,
            line_of: opts.line_of,
            file,
            dir,
            strict_types,
            sink: RefCell::new(FnSink {
                funcs: vec![Function::default()],
                classes: vec![None; n_classes],
            }),
        }
    }

    /// The 1-based line of a byte offset (0 without line information).
    pub(crate) fn line(&self, offset: u32) -> u32 {
        self.line_of.map_or(0, |f| f(offset))
    }
}

/// A named body to compile: a user function or a method.
pub(crate) struct FnSpec<'a> {
    pub(crate) name: IdentId,
    pub(crate) params: &'a [Param],
    pub(crate) body: &'a [Stmt],
    pub(crate) span: Span,
    pub(crate) by_ref: bool,
    /// The declaring class for a method, with its name.
    pub(crate) cur_class: Option<(ClassId, IdentId)>,
    pub(crate) is_static: bool,
}

/// An enclosing loop or `switch`, for `break N` / `continue N`.
pub(crate) struct LoopCtx {
    /// Unique per function (for the `goto`-into-loop check).
    pub(crate) id: u32,
    /// `break` jumps to patch.
    pub(crate) breaks: Vec<usize>,
    /// `continue` jumps to patch (`None` target until the loop closes).
    pub(crate) continues: Vec<usize>,
    /// A `switch` (where `continue` acts like `break`).
    pub(crate) is_switch: bool,
}

/// A nullsafe chain in progress: jumps that must land on "result = null".
pub(crate) struct NullsafeCtx {
    pub(crate) jumps: Vec<usize>,
}

/// The lowering state of one function body.
pub(crate) struct FnCompiler<'a> {
    pub(crate) mx: &'a ModuleCtx<'a>,
    pub(crate) diags: &'a mut Vec<Diagnostic>,
    /// The class whose method is being compiled (lexical context for
    /// `self::`/`parent::`, `__CLASS__`); `None` outside a method.
    pub(crate) cur_class: Option<(ClassId, IdentId)>,
    /// `__FUNCTION__`.
    pub(crate) func_name: Box<[u8]>,
    /// `true` while compiling statements PHP hoists declarations from (the
    /// `{main}` statement list, plain blocks and global namespace bodies at
    /// that level); a declaration met anywhere else is conditional and is
    /// lowered to a `DeclareFunction`/`DeclareClass` op in place.
    pub(crate) at_top_level: bool,
    /// Compiling `{main}`.
    pub(crate) is_main: bool,

    /// Variable -> permanent register. Variables occupy the low registers
    /// (params first, then captured `use` vars, then locals); temporaries live
    /// above them.
    pub(crate) vars: HashMap<IdentId, Reg>,
    /// The register `$this` is loaded into, when the body reads it.
    pub(crate) this_reg: Option<Reg>,
    /// Current top of the temporary stack (next free temp register).
    pub(crate) temp_top: Reg,
    /// High-water mark: total registers the frame needs.
    pub(crate) num_regs: Reg,
    /// Closure captures (`Function::captures`).
    pub(crate) captures: Vec<CaptureDesc>,
    /// `static $x` cells (`Function::statics`).
    pub(crate) statics: Vec<StaticVar>,
    /// Function flags accumulated while lowering.
    pub(crate) flags: FnFlags,
    /// Inline-cache slots handed out so far.
    pub(crate) ic_count: u16,

    pub(crate) code: Vec<Op>,
    pub(crate) consts: Vec<Const>,
    /// Source line per op, parallel to `code` (empty without `line_of`).
    pub(crate) lines: Vec<u32>,
    /// The line the ops being emitted belong to.
    pub(crate) cur_line: u32,

    /// Enclosing loops/switches, innermost last.
    pub(crate) loops: Vec<LoopCtx>,
    pub(crate) next_loop_id: u32,
    /// `label:` → (address, enclosing loop ids at the label).
    pub(crate) labels: HashMap<IdentId, (CodeAddr, Vec<u32>)>,
    /// `goto` sites to patch: (jump op index, label, enclosing loop ids, span).
    pub(crate) gotos: Vec<(usize, IdentId, Vec<u32>, Span)>,
    /// Nullsafe chains being compiled, innermost last.
    pub(crate) nullsafe: Vec<NullsafeCtx>,
    /// Compiling the object/base link of a nullsafe chain (a nested `?->`
    /// joins the enclosing chain instead of starting its own).
    pub(crate) in_nullsafe: bool,
}

impl<'a> FnCompiler<'a> {
    pub(crate) fn new(
        mx: &'a ModuleCtx<'a>,
        diags: &'a mut Vec<Diagnostic>,
        params: &[Param],
        captures: &[(IdentId, bool)],
        body: ClosureBody<'_>,
        cur_class: Option<(ClassId, IdentId)>,
        func_name: Box<[u8]>,
    ) -> Self {
        let mut vars: HashMap<IdentId, Reg> = HashMap::new();
        // Params take registers `0 .. np`; captured `use` vars follow.
        for (i, p) in params.iter().enumerate() {
            vars.insert(p.name, i as Reg);
        }
        let mut var_count = params.len() as Reg;
        let mut capture_descs = Vec::with_capacity(captures.len());
        for &(c, by_ref) in captures {
            let reg = *vars.entry(c).or_insert_with(|| {
                let r = var_count;
                var_count += 1;
                r
            });
            capture_descs.push(CaptureDesc {
                src: 0, // filled by the enclosing frame when it builds the closure
                dst: reg,
                by_ref,
            });
        }
        // Pre-scan the body so every variable has a permanent register before
        // any temporary is allocated.
        let facts = regs::collect_body(body, mx.interner, &mut vars, &mut var_count);
        let this_id = mx.interner.get(b"this");
        let this_reg = this_id.and_then(|id| vars.get(&id).copied());
        let mut flags = FnFlags::NONE;
        if facts.uses_this || this_reg.is_some() {
            flags |= FnFlags::USES_THIS;
        }
        if facts.needs_symtab {
            flags |= FnFlags::NEEDS_SYMTAB;
        }
        if mx.strict_types {
            flags |= FnFlags::STRICT_TYPES;
        }

        FnCompiler {
            mx,
            diags,
            cur_class,
            func_name,
            at_top_level: false,
            is_main: false,
            vars,
            this_reg,
            temp_top: var_count,
            num_regs: var_count,
            captures: capture_descs,
            statics: Vec::new(),
            flags,
            ic_count: 0,
            code: Vec::new(),
            consts: Vec::new(),
            lines: Vec::new(),
            cur_line: 0,
            loops: Vec::new(),
            next_loop_id: 0,
            labels: HashMap::new(),
            gotos: Vec::new(),
            nullsafe: Vec::new(),
            in_nullsafe: false,
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
            | Op::IterNext { target: t, .. }
            | Op::BindStaticOrJmp { target: t, .. }
            | Op::Switch { default: t, .. } => *t = target,
            _ => unreachable!("patch on a non-branch op"),
        }
    }

    /// Emit a forward `Jmp` to be patched.
    pub(crate) fn jmp_fwd(&mut self) -> usize {
        self.emit(Op::Jmp { target: 0 })
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

    /// A fresh inline-cache slot.
    pub(crate) fn ic(&mut self) -> u16 {
        let ic = self.ic_count;
        self.ic_count += 1;
        ic
    }

    /// A member name (property / method) as a `Const::Str` in the pool.
    pub(crate) fn str_const(&mut self, bytes: &[u8]) -> u32 {
        self.push_const(Const::Str(Str::new(bytes)))
    }

    /// A member name as a `Const::Str`, by interned id.
    pub(crate) fn name_const(&mut self, name: IdentId) -> u32 {
        self.str_const(self.mx.interner.resolve(name))
    }

    /// A function/class/constant name as a `Const::Name` (prelowercased twin).
    pub(crate) fn sym_const(&mut self, bytes: &[u8]) -> u32 {
        self.push_const(Const::Name(NameConst::new(bytes)))
    }

    pub(crate) fn var_reg(&self, id: IdentId) -> Reg {
        *self
            .vars
            .get(&id)
            .expect("every variable is assigned a register during the pre-scan")
    }

    /// Whether `id` is the variable `$this`.
    pub(crate) fn is_this(&self, id: IdentId) -> bool {
        self.mx.interner.resolve(id) == b"this"
    }

    /// Whether `id` is the superglobal `$GLOBALS`.
    pub(crate) fn is_globals(&self, id: IdentId) -> bool {
        self.mx.interner.resolve(id) == b"GLOBALS"
    }

    /// The register holding `$this` for a read: emits `LoadThis` into the
    /// dedicated register.
    pub(crate) fn load_this(&mut self) -> Reg {
        let dst = match self.this_reg {
            Some(r) => r,
            None => {
                // `$this` was not seen by the pre-scan (it came through a path
                // the scan skips); give it a register now.
                let r = self.alloc_temp();
                self.this_reg = Some(r);
                r
            }
        };
        self.flags |= FnFlags::USES_THIS;
        self.emit(Op::LoadThis { dst });
        dst
    }

    /// Store `src` into the named-variable register `dst`: always through a
    /// possible reference binding (`AssignThroughRef`), which also
    /// dereferences the source. A plain register behaves like `Move`.
    pub(crate) fn store_var(&mut self, dst: Reg, src: Reg) {
        if dst != src {
            self.emit(Op::AssignThroughRef { dst, src });
        }
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

    /// The `var_names` table: every named variable (excluding `$this`).
    fn var_names(&self) -> Vec<(Box<[u8]>, Reg)> {
        let mut names: Vec<(Box<[u8]>, Reg)> = self
            .vars
            .iter()
            .filter(|(id, _)| !self.is_this(**id))
            .map(|(id, r)| (self.mx.interner.resolve(*id).into(), *r))
            .collect();
        names.sort_by_key(|(_, r)| *r);
        names
    }

    /// Resolve `goto`s against the labels of this body.
    fn resolve_gotos(&mut self) {
        let gotos = std::mem::take(&mut self.gotos);
        for (idx, label, loops, span) in gotos {
            match self.labels.get(&label) {
                None => self.diags.push(
                    Diagnostic::error(
                        crate::UNDEFINED_LABEL,
                        format!(
                            "'goto' to undefined label '{}'",
                            self.mx.interner.resolve_lossy(label)
                        ),
                    )
                    .with_primary(span, "no such label"),
                ),
                Some((addr, label_loops)) => {
                    // Jumping *into* a loop or switch is disallowed: the
                    // label's enclosing loops must all enclose the goto too.
                    if !loops.starts_with(label_loops) {
                        self.diags.push(
                            Diagnostic::error(
                                crate::GOTO_INTO_LOOP,
                                "'goto' into loop or switch statement is disallowed",
                            )
                            .with_primary(span, "jumps into a loop or switch"),
                        );
                    }
                    let addr = *addr;
                    self.patch(idx, addr);
                }
            }
        }
    }

    /// Assemble the compiled body into a [`Function`].
    pub(crate) fn finish(mut self, name_bytes: Box<[u8]>, params: Vec<ParamDef>, span: Span) -> Function {
        self.resolve_gotos();
        let var_names = self.var_names();
        let num_params = params.len() as u16;
        let decl_line = self.mx.line(span.lo);
        let end_line = self.mx.line(span.hi);
        if params.last().is_some_and(|p| p.variadic) {
            self.flags |= FnFlags::VARIADIC;
        }
        Function {
            name: IdentId(0),
            name_bytes,
            num_params,
            num_regs: self.num_regs,
            code: self.code,
            consts: self.consts,
            capture_regs: Vec::new(),
            closures: Vec::new(),
            span,
            params,
            ret_ty: None,
            flags: self.flags,
            ex_regions: Vec::new(),
            captures: self.captures,
            statics: self.statics,
            var_names,
            lines: self.lines,
            ic_count: self.ic_count,
            doc: None,
            attrs: Vec::new(),
            decl_line,
            end_line,
            in_class: self.cur_class.map(|(c, _)| c),
        }
    }

    // ---- parameters -----------------------------------------------------------

    /// Emit the prologue: `BindSymtab` (symtab frames), `RecvInit` for every
    /// defaulted parameter, `RecvVariadic` for `...$rest`; and build the
    /// [`ParamDef`]s (default thunks are compiled into the sink).
    pub(crate) fn compile_params(&mut self, params: &[Param]) -> Vec<ParamDef> {
        if self.flags.contains(FnFlags::NEEDS_SYMTAB) {
            self.emit(Op::BindSymtab);
        }
        let mut defs = Vec::with_capacity(params.len());
        for (i, p) in params.iter().enumerate() {
            if p.promote.is_some() {
                unsupported(self.diags, p.span, "constructor property promotion");
            }
            if !p.hooks.is_empty() {
                unsupported(self.diags, p.span, "property hooks on a promoted parameter");
            }
            let reg = self.var_reg(p.name);
            let default = match &p.default {
                None => None,
                Some(e) => {
                    let init = self.compile_init(e);
                    self.mark_line(p.span);
                    self.emit(Op::RecvInit {
                        param: i as u16,
                        init,
                    });
                    Some(init)
                }
            };
            if p.variadic {
                self.emit(Op::RecvVariadic { reg });
            }
            defs.push(ParamDef {
                name: self.mx.interner.resolve(p.name).into(),
                reg,
                by_ref: p.by_ref,
                variadic: p.variadic,
                default,
                ty: None,
                promoted: None,
                attrs: Vec::new(),
            });
        }
        defs
    }

    /// A constant-expression initializer: a pool constant when the expression
    /// is a literal, otherwise a zero-argument thunk in the sink evaluated in
    /// this function's class scope.
    pub(crate) fn compile_init(&mut self, e: &Expr) -> InitRef {
        if let Some(v) = crate::class::const_default(e, self.mx.interner) {
            let k = self.push_const(match v {
                rphp_value::Value::Null => Const::Null,
                rphp_value::Value::Bool(b) => Const::Bool(b),
                rphp_value::Value::Int(i) => Const::Int(i),
                rphp_value::Value::Float(f) => Const::Float(f),
                rphp_value::Value::Str(s) => Const::Str(s),
                _ => return InitRef::Thunk(self.compile_thunk(e)),
            });
            return InitRef::Const(k);
        }
        InitRef::Thunk(self.compile_thunk(e))
    }

    /// Compile `e` as a zero-argument function returning its value.
    pub(crate) fn compile_thunk(&mut self, e: &Expr) -> FuncId {
        let id = self.mx.sink.borrow_mut().reserve();
        let mut fc = FnCompiler::new(
            self.mx,
            &mut *self.diags,
            &[],
            &[],
            ClosureBody::ReturnExpr(e),
            self.cur_class,
            Box::from(&b""[..]),
        );
        fc.mark_line(e.span());
        let r = fc.compile_expr(e);
        fc.emit(Op::Ret { src: Some(r) });
        let f = fc.finish(Box::from(&b""[..]), Vec::new(), e.span());
        self.mx.sink.borrow_mut().fill(id, f);
        id
    }

    // ---- closures -------------------------------------------------------------

    /// Lower `function (...) use (...) { ... }`: compile its body as its own
    /// function (captures bound to the closure's registers), then emit a
    /// `MakeClosure` that snapshots the captured variables (or binds the
    /// by-reference ones) and the current `$this`/scope at runtime.
    pub(crate) fn compile_closure_expr(&mut self, c: &Closure) -> Reg {
        let uses: Vec<(IdentId, bool)> = c.uses.iter().map(|u| (u.name, u.by_ref)).collect();
        self.compile_closure(&c.params, &uses, ClosureBody::Stmts(&c.body), c.span, c.static_)
    }

    /// Lower `fn (...) => e`: the free variables of `e` are captured by value
    /// (auto-capture) and the body is `return e;`.
    pub(crate) fn compile_arrow_fn(&mut self, f: &rphp_ast::v2::ArrowFn) -> Reg {
        let uses: Vec<(IdentId, bool)> = regs::arrow_free_vars(f, self.mx.interner)
            .into_iter()
            .map(|id| (id, false))
            .collect();
        self.compile_closure(&f.params, &uses, ClosureBody::ReturnExpr(&f.body), f.span, f.static_)
    }

    fn compile_closure(
        &mut self,
        params: &[Param],
        uses: &[(IdentId, bool)],
        body: ClosureBody<'_>,
        span: Span,
        is_static: bool,
    ) -> Reg {
        let line = self.mx.line(span.lo);
        // php 8.4+: `{closure:<enclosing>:<line>}` where the enclosing scope is
        // the file for top-level code, `f()` / `A::m()` for functions and
        // methods, and the enclosing closure's own name when nested.
        let name: Vec<u8> = {
            let mut n = b"{closure:".to_vec();
            if self.is_main {
                n.extend_from_slice(&self.mx.file);
            } else if self.flags.contains(FnFlags::CLOSURE) {
                n.extend_from_slice(&self.func_name);
            } else {
                if let Some((_, class)) = self.cur_class {
                    n.extend_from_slice(self.mx.interner.resolve(class));
                    n.extend_from_slice(b"::");
                }
                n.extend_from_slice(&self.func_name);
                n.extend_from_slice(b"()");
            }
            n.push(b':');
            n.extend_from_slice(line.to_string().as_bytes());
            n.push(b'}');
            n
        };
        let id = self.mx.sink.borrow_mut().reserve();
        // The enclosing frame's registers each capture comes from.
        let srcs: Vec<Reg> = uses.iter().map(|&(u, _)| self.var_reg(u)).collect();
        let mut fc = FnCompiler::new(
            self.mx,
            &mut *self.diags,
            params,
            uses,
            body,
            self.cur_class,
            name.clone().into(),
        );
        fc.flags |= FnFlags::CLOSURE;
        if is_static {
            fc.flags |= FnFlags::STATIC;
        }
        for (i, src) in srcs.into_iter().enumerate() {
            fc.captures[i].src = src;
        }
        fc.mark_line(span);
        let defs = fc.compile_params(params);
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
        let f = fc.finish(name.into(), defs, span);
        self.mx.sink.borrow_mut().fill(id, f);
        let dst = self.alloc_temp();
        self.emit(Op::MakeClosure { dst, proto: id });
        dst
    }
}

/// Compile a user function or a method to a [`Function`] in the sink,
/// returning its id.
pub(crate) fn compile_function(
    mx: &ModuleCtx<'_>,
    diags: &mut Vec<Diagnostic>,
    spec: FnSpec<'_>,
) -> FuncId {
    let FnSpec {
        name,
        params,
        body,
        span,
        by_ref,
        cur_class,
        is_static,
    } = spec;
    let id = mx.sink.borrow_mut().reserve();
    let name_bytes: Box<[u8]> = mx.interner.resolve(name).into();
    let mut fc = FnCompiler::new(
        mx,
        diags,
        params,
        &[],
        ClosureBody::Stmts(body),
        cur_class,
        name_bytes.clone(),
    );
    if is_static {
        fc.flags |= FnFlags::STATIC;
    }
    if by_ref {
        // `function &f()`: the flag is metadata; the value is returned by
        // value until reference returns land (`$x = &f()` binds a copy).
        fc.flags |= FnFlags::RETURNS_REF;
    }
    // The prologue (and an empty body's `Ret`) belongs to the declaration line.
    fc.mark_line(span);
    let defs = fc.compile_params(params);
    fc.compile_stmts(body);
    // Always terminate with a fall-through return so every code path (and every
    // branch target that lands at the textual end) has a valid `Ret`.
    fc.emit(Op::Ret { src: None });
    let f = fc.finish(name_bytes, defs, span);
    mx.sink.borrow_mut().fill(id, f);
    id
}
