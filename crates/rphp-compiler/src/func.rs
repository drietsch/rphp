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

use rphp_ast::v2::{
    Builtin, ClassRef as AstClassRef, Closure, ConstSel, Expr, Hook, HookBody, HookKind, Param,
    Resolved, Stmt, TempId, Type, TypeKind, Visibility as AstVis,
};
use rphp_bytecode::{
    AttrDef, AttrTarget, BuiltinType, CaptureDesc, Class as BcClass, ClassId, CmpKind, CodeAddr,
    Const, FnFlags, FuncId, Function, InitRef, NameConst, NameRef, Op, ParamDef, PromotedProp, Reg,
    StaticVar, TypeDecl, CONST_OPERAND,
};
use rphp_diagnostics::Diagnostic;
use rphp_intern::{IdentId, Interner};
use rphp_span::Span;
use rphp_value::{Str, Value};

use crate::class::{bc_vis, class_fqn};
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
    /// The name the pre-pass synthesized for each anonymous class.
    pub(crate) anon_names: &'a HashMap<*const rphp_ast::v2::ClassLike, rphp_intern::IdentId>,
    /// Byte offset → 1-based line, when line tables are wanted.
    pub(crate) line_of: Option<&'a dyn Fn(u32) -> u32>,
    /// `__FILE__`.
    pub(crate) file: Box<[u8]>,
    /// `__DIR__`.
    pub(crate) dir: Box<[u8]>,
    /// `declare(strict_types=1)` at the top of the unit.
    pub(crate) strict_types: bool,
    /// `zend.assertions` as it was when this unit started compiling.
    pub(crate) assertions: i8,
    /// The unit's source, for `assert()`'s reconstructed text.
    pub(crate) source: Option<&'a [u8]>,
    /// The function/class sinks.
    pub(crate) sink: RefCell<FnSink>,
    /// The top-level class-likes the unit hoists (declared before `{main}`
    /// runs). Everything else is declared by an `Op::DeclareClass` at its
    /// statement, so `hoisted` and `stmt.rs` must agree — see
    /// [`crate::class_is_hoisted`].
    pub(crate) hoisted_classes: std::collections::HashSet<*const rphp_ast::v2::ClassLike>,
}

/// `__FILE__` and `__DIR__` for the unit. The class pre-pass needs the file
/// before there is a [`ModuleCtx`], because an anonymous class is named after
/// where it was declared.
pub(crate) fn unit_file(opts: &CompileOptions<'_>) -> (Box<[u8]>, Box<[u8]>) {
    match &opts.file {
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
            (Box::from(&b"Command line code"[..]), dir.into_bytes().into())
        }
    }
}

impl<'a> ModuleCtx<'a> {
    /// Build the context from the compile options and the class pre-pass.
    pub(crate) fn new(
        interner: &'a Interner,
        class_map: &'a HashMap<Box<[u8]>, ClassId>,
        class_ids: &'a HashMap<*const rphp_ast::v2::ClassLike, ClassId>,
        anon_names: &'a HashMap<*const rphp_ast::v2::ClassLike, rphp_intern::IdentId>,
        n_classes: usize,
        strict_types: bool,
        opts: &CompileOptions<'a>,
    ) -> Self {
        let (file, dir) = unit_file(opts);
        ModuleCtx {
            assertions: opts.assertions,
            source: opts.source,
            hoisted_classes: std::collections::HashSet::new(),
            interner,
            class_map,
            class_ids,
            anon_names,
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
    /// The line a declaration *starts* on for Reflection: php counts from
    /// the first keyword, so the attributes (and the docblock) before it do
    /// not move it. `after_attrs` is the end of the last attribute group,
    /// or the declaration's start when there is none.
    pub(crate) fn decl_line(&self, after_attrs: u32) -> u32 {
        let mut at = after_attrs as usize;
        if let Some(src) = self.source {
            while at < src.len() && src[at].is_ascii_whitespace() {
                at += 1;
            }
        }
        self.line(at as u32)
    }

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
    /// The declared return type.
    pub(crate) ret: Option<&'a rphp_ast::v2::Type>,
    /// The `/** … */` immediately before the declaration, which
    /// `ReflectionFunctionAbstract::getDocComment()` answers with.
    pub(crate) doc: Option<IdentId>,
    /// The `#[...]` groups before the declaration.
    pub(crate) attrs: &'a [rphp_ast::v2::AttrGroup],
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
    /// Number of `finally` regions open when the loop was entered: a
    /// `break`/`continue` to it crosses the regions opened after that.
    pub(crate) finally_depth: usize,
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
    /// Variables known to hold a value at the point being compiled, so a read
    /// of one needs no `Op::CheckVar`. Assignments add to it. What a branch
    /// assigns is only conditionally assigned after it, so a branching
    /// construct restores the set it started with at every alternative entry
    /// (`else`, a `case`, a `catch`) and at its end — and what was assigned
    /// *before* the construct stays assigned throughout it, since nothing in
    /// a body without `unset()` can take a local's value away
    /// ([`Self::structured_assign`]). Erring is only ever an extra check.
    pub(crate) assigned: std::collections::HashSet<IdentId>,
    /// Whether the set above survives a branch (see there): false at the
    /// top level, where other files and `$GLOBALS` reach the variables, and
    /// in a body that can lose a local (`BodyFacts::may_lose_vars`). When
    /// false a branch clears the set, the conservative answer.
    pub(crate) structured_assign: bool,
    /// For each enclosing branching statement, the set assigned before it
    /// (what an alternative entry comes back to; `stmt.rs`).
    pub(crate) branch_bases: Vec<std::collections::HashSet<IdentId>>,
    /// The register `$this` is loaded into, when the body reads it.
    pub(crate) this_reg: Option<Reg>,
    /// Current top of the temporary stack (next free temp register).
    pub(crate) temp_top: Reg,
    /// The highest `temp_top` since the current statement began (what
    /// [`release_temps`](Self::release_temps) clears: sub-expressions free
    /// their temporaries early, but the values linger until then).
    pub(crate) temp_peak: Reg,
    /// The temporaries (a register range, `lo..hi`) that received a value
    /// which may be a container since the current statement began; empty
    /// when `dirty_lo >= dirty_hi`.
    pub(crate) dirty_lo: Reg,
    pub(crate) dirty_hi: Reg,
    /// Registers below this are named variables; the rest are temporaries.
    pub(crate) var_count: Reg,
    /// The highest code address a jump can land on (every label taken with
    /// [`Self::here`]): an op after it is reached only through the op before
    /// it, which is what lets [`Self::store_var`] fold the store into the
    /// producer.
    pub(crate) max_label: usize,
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
    /// HIR temporaries in scope (`Let` bindings and `foreach` value temps),
    /// innermost last: `Temp(t)` reads the register of the latest binding
    /// of `t`.
    pub(crate) temps: Vec<(TempId, Reg)>,
    /// Open `try` regions with a `finally`, innermost last (`tryfin.rs`).
    pub(crate) finallys: Vec<crate::tryfin::FinallyCtx>,
    /// Closed exception regions, innermost-first (`Function::ex_regions`).
    pub(crate) ex_regions: Vec<rphp_bytecode::ExRegion>,
    /// The declared return type (`Function::ret_ty`).
    pub(crate) ret_ty: Option<rphp_bytecode::TypeDecl>,
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
        if facts.is_generator {
            flags |= FnFlags::GENERATOR;
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
            // A `use` capture holds its value before the body runs (php
            // warned at the closure's creation and bound null for an
            // undefined one); an arrow function's implicit capture of an
            // undefined variable is simply not made, so its reads warn.
            assigned: match body {
                ClosureBody::Stmts(_) => captures.iter().map(|&(c, _)| c).collect(),
                ClosureBody::ReturnExpr(_) => std::collections::HashSet::new(),
            },
            structured_assign: !facts.needs_symtab && !facts.may_lose_vars,
            branch_bases: Vec::new(),
            this_reg,
            temp_top: var_count,
            temp_peak: var_count,
            dirty_lo: Reg::MAX,
            dirty_hi: 0,
            var_count,
            max_label: 0,
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
            temps: Vec::new(),
            finallys: Vec::new(),
            ex_regions: Vec::new(),
            ret_ty: None,
        }
    }

    // ---- low-level helpers --------------------------------------------------

    pub(crate) fn interner(&self) -> &'a Interner {
        self.mx.interner
    }

    pub(crate) fn emit(&mut self, op: Op) -> usize {
        // A temporary that receives a value which may be a container (a
        // property, a static, an element, a call's result, a variable's
        // copy) is what `release_temps` must clear; one holding an
        // arithmetic result or a literal need not be.
        let holder = match op {
            Op::FetchProp { dst, .. }
            | Op::FetchStaticProp { dst, .. }
            | Op::ArrayGet { dst, .. }
            | Op::ArrayGetQuiet { dst, .. }
            | Op::DoCall { dst }
            | Op::Move { dst, .. }
            | Op::Deref { dst, .. }
            | Op::FetchConst { dst, .. } => Some(dst),
            _ => None,
        };
        if let Some(r) = holder {
            if r >= self.var_count {
                self.dirty_lo = self.dirty_lo.min(r);
                self.dirty_hi = self.dirty_hi.max(r + 1);
            }
        }
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

    /// The address of the next op — taken as a label a jump will land on.
    pub(crate) fn here(&mut self) -> CodeAddr {
        self.max_label = self.code.len();
        self.code.len() as CodeAddr
    }

    pub(crate) fn patch(&mut self, idx: usize, target: CodeAddr) {
        match &mut self.code[idx] {
            Op::Jmp { target: t }
            | Op::JmpIfTrue { target: t, .. }
            | Op::JmpIfFalse { target: t, .. }
            | Op::JmpUnless { target: t, .. }
            | Op::JmpUnlessArgByRef { target: t, .. }
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
        if n > self.temp_peak {
            self.temp_peak = n;
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

    /// [`free_to`](Self::free_to) at a point where the temporaries above
    /// `mark` are dead for good (a statement's end): the emitted
    /// [`Op::FreeTemps`] drops their values too, so nothing lingers in a
    /// register — a container left there would make the next write to it
    /// copy. `keep` is a register the caller still needs (a condition
    /// about to be tested).
    pub(crate) fn release_temps(&mut self, mark: Reg, keep: Option<Reg>) {
        let top = self.temp_peak.max(self.temp_top);
        self.temp_peak = mark;
        // Only the range that may hold a container is cleared.
        let (lo, hi) = (self.dirty_lo.max(mark), self.dirty_hi.min(top));
        self.dirty_lo = Reg::MAX;
        self.dirty_hi = 0;
        if lo < hi {
            match keep {
                Some(k) if k >= lo && k < hi => {
                    if k > lo {
                        self.emit(Op::FreeTemps { from: lo, to: k });
                    }
                    if k + 1 < hi {
                        self.emit(Op::FreeTemps { from: k + 1, to: hi });
                    }
                }
                _ => {
                    self.emit(Op::FreeTemps { from: lo, to: hi });
                }
            }
        }
        self.free_to(mark);
    }

    pub(crate) fn push_const(&mut self, c: Const) -> u32 {
        let k = self.consts.len() as u32;
        self.consts.push(c);
        k
    }

    /// An operand for an arithmetic/comparison op: a literal goes into the
    /// pool and is named as a constant operand ([`CONST_OPERAND`]), anything
    /// else is compiled into a register.
    pub(crate) fn operand(&mut self, e: &Expr) -> Reg {
        if let Some(v) = crate::stmt::literal_value(e, self.mx.interner) {
            let c = match v {
                Value::Null => Const::Null,
                Value::Bool(b) => Const::Bool(b),
                Value::Int(i) => Const::Int(i),
                Value::Float(f) => Const::Float(f),
                Value::Str(s) => Const::Str(s),
                _ => return self.compile_expr(e),
            };
            let k = self.push_const(c);
            if k < u32::from(CONST_OPERAND) {
                return CONST_OPERAND | k as Reg;
            }
        }
        self.compile_expr(e)
    }

    /// `JmpIfFalse` on `cond`, fused with the comparison that just computed
    /// it into a temporary ([`Op::JmpUnless`]) when nothing else can reach
    /// this point. Returns the jump's index, to be patched.
    pub(crate) fn jump_if_false(&mut self, cond: Reg) -> usize {
        if cond >= self.var_count && cond < CONST_OPERAND && self.max_label < self.code.len() {
            let fused = match self.code.last() {
                Some(Op::CmpEq { dst, a, b }) if *dst == cond => Some((CmpKind::Eq, *a, *b)),
                Some(Op::CmpNe { dst, a, b }) if *dst == cond => Some((CmpKind::Ne, *a, *b)),
                Some(Op::CmpIdentical { dst, a, b }) if *dst == cond => Some((CmpKind::Identical, *a, *b)),
                Some(Op::CmpNotIdentical { dst, a, b }) if *dst == cond => {
                    Some((CmpKind::NotIdentical, *a, *b))
                }
                Some(Op::CmpLt { dst, a, b }) if *dst == cond => Some((CmpKind::Lt, *a, *b)),
                Some(Op::CmpLe { dst, a, b }) if *dst == cond => Some((CmpKind::Le, *a, *b)),
                Some(Op::CmpGt { dst, a, b }) if *dst == cond => Some((CmpKind::Gt, *a, *b)),
                Some(Op::CmpGe { dst, a, b }) if *dst == cond => Some((CmpKind::Ge, *a, *b)),
                _ => None,
            };
            if let Some((kind, a, b)) = fused {
                self.code.pop();
                if self.mx.line_of.is_some() {
                    self.lines.pop();
                }
                return self.emit(Op::JmpUnless { kind, a, b, target: 0 });
            }
        }
        self.emit(Op::JmpIfFalse { cond, target: 0 })
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

    /// The register of a variable being **read**, with php's
    /// `Warning: Undefined variable $x` when the compiler cannot prove it has
    /// a value by now. The check is a runtime one — the warning depends on
    /// what actually ran — so eliding it is only ever an optimization.
    pub(crate) fn read_var(&mut self, id: IdentId) -> Reg {
        let reg = self.var_reg(id);
        if !self.assigned.contains(&id) {
            let name = self.name_const(id);
            self.emit(Op::CheckVar { reg, name });
        }
        reg
    }

    /// Record that `id` now holds a value.
    pub(crate) fn mark_assigned(&mut self, id: IdentId) {
        self.assigned.insert(id);
    }

    /// Forget what is assigned: called around anything that branches, where
    /// a linear walk of the source says nothing about what ran.
    pub(crate) fn forget_assigned(&mut self) {
        self.assigned.clear();
    }

    /// The set to come back to at a branch's alternative entries and end:
    /// what is assigned now when the body is structured, nothing otherwise.
    pub(crate) fn assigned_snapshot(&self) -> std::collections::HashSet<IdentId> {
        if self.structured_assign {
            self.assigned.clone()
        } else {
            std::collections::HashSet::new()
        }
    }

    /// Come back to a snapshot (see [`Self::assigned_snapshot`]).
    pub(crate) fn restore_assigned(&mut self, snapshot: &std::collections::HashSet<IdentId>) {
        if self.structured_assign {
            self.assigned.clone_from(snapshot);
        } else {
            self.assigned.clear();
        }
    }

    /// Whether `id` is the superglobal `$GLOBALS`.
    pub(crate) fn is_globals(&self, id: IdentId) -> bool {
        self.mx.interner.resolve(id) == b"GLOBALS"
    }

    /// php's auto-globals are one variable in every scope: `$_SERVER` inside a
    /// function is the *global* `$_SERVER`, with no `global` statement and no
    /// capture. So a body that mentions one binds its register to the global
    /// cell in the prologue — which is what `global $_SERVER;` would do, and
    /// creates the entry the way php's auto-global handler does (writing
    /// `$_SESSION` inside a function makes it a global).
    ///
    /// `$GLOBALS` is not in the list: it is not a variable but a view of the
    /// table, lowered on its own ([`FnCompiler::is_globals`]).
    pub(crate) fn bind_auto_globals(&mut self) {
        let mut names: Vec<(Box<[u8]>, IdentId, Reg)> = self
            .vars
            .iter()
            .filter_map(|(&id, &reg)| {
                let name = self.mx.interner.resolve(id);
                (name != b"GLOBALS" && rphp_hir::scope::is_auto_global(name))
                    .then(|| (Box::from(name), id, reg))
            })
            .collect();
        // `vars` is a hash map; the bytecode has to come out the same every
        // time, so bind in name order.
        names.sort_by(|a, b| a.0.cmp(&b.0));
        for (_, id, reg) in names {
            let name = self.name_const(id);
            self.emit(Op::BindGlobal { reg, name });
        }
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
        if dst == src {
            return;
        }
        // The value was just computed into a temporary by the last op, and
        // nothing jumps past that op to here: compute straight into the
        // variable instead (the runtime stores through a reference cell for
        // a variable register, `Function::var_count`).
        if src >= self.var_count && self.max_label < self.code.len() {
            if let Some(op) = self.code.last_mut() {
                if op.result_reg() == Some(src) {
                    op.set_result_reg(dst);
                    return;
                }
            }
        }
        self.emit(Op::AssignThroughRef { dst, src });
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

    /// The `var_names` table: every named variable (excluding `$this` and
    /// `$GLOBALS`, which is a view of the table rather than an entry in it —
    /// php's `array_keys($GLOBALS)` never contains `GLOBALS`).
    fn var_names(&self) -> Vec<(Box<[u8]>, Reg)> {
        let mut names: Vec<(Box<[u8]>, Reg)> = self
            .vars
            .iter()
            .filter(|(id, _)| !self.is_this(**id) && !self.is_globals(**id))
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
            var_count: self.var_count,
            num_regs: self.num_regs,
            code: self.code,
            consts: self.consts,
            capture_regs: Vec::new(),
            closures: Vec::new(),
            span,
            params,
            ret_ty: self.ret_ty,
            flags: self.flags,
            ex_regions: self.ex_regions,
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
    ///
    /// A **promoted** parameter (`__construct(private int $x)`) is an ordinary
    /// parameter that is additionally assigned to `$this->x` once the whole
    /// prologue has run, which is where php assigns it: before the
    /// constructor body, after every argument has been received (so
    /// `func_get_args()` still sees only the arguments actually passed). The
    /// matching [`PropDef`](rphp_bytecode::PropDef) is added to the class by
    /// [`crate::class::compile_class`], which reads the same parameter list.
    pub(crate) fn compile_params(&mut self, params: &[Param]) -> Vec<ParamDef> {
        // A parameter always holds a value by the time the body runs, and so
        // does a captured variable.
        for p in params {
            self.mark_assigned(p.name);
        }

        if self.flags.contains(FnFlags::NEEDS_SYMTAB) {
            self.emit(Op::BindSymtab);
        }
        self.bind_auto_globals();
        let mut defs = Vec::with_capacity(params.len());
        // (register, property name) of every promoted parameter, in order.
        let mut promoted: Vec<(Reg, Box<[u8]>)> = Vec::new();
        for (i, p) in params.iter().enumerate() {
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
            let promotion = p.promote.as_ref().map(|m| PromotedProp {
                vis: bc_vis(m.vis.unwrap_or(AstVis::Public)),
                set_vis: m.set_vis.map(bc_vis),
                readonly: m.readonly,
            });
            if promotion.is_some() {
                promoted.push((reg, self.mx.interner.resolve(p.name).into()));
            }
            defs.push(ParamDef {
                name: self.mx.interner.resolve(p.name).into(),
                reg,
                by_ref: p.by_ref,
                variadic: p.variadic,
                default,
                default_const: p.default.as_ref().and_then(|e| self.default_const_name(e)),
                ty: p.ty.as_ref().map(|t| self.lower_type(t)),
                promoted: promotion,
                attrs: self.compile_attrs(&p.attrs, AttrTarget::Parameter),
            });
        }
        // Promotion outside a class is a compile-time fatal the front end
        // already reported (`Cannot declare promoted property outside a
        // constructor`); emit nothing rather than an unbound `$this`.
        if !promoted.is_empty() && self.cur_class.is_some() {
            let this = self.load_this();
            for (src, name) in promoted {
                let k = self.str_const(&name);
                let ic = self.ic();
                self.emit(Op::AssignProp {
                    obj: this,
                    name: NameRef::constant(k),
                    src,
                    ic,
                });
            }
        }
        defs
    }

    /// A constant-expression initializer: a pool constant when the expression
    /// is a literal, otherwise a zero-argument thunk in the sink evaluated in
    /// Lower `#[Foo(1, x: 2)]` groups into the compiled form Reflection
    /// reads. Each argument becomes an [`InitRef`] against *this* function's
    /// constant pool (or a thunk in its unit), which is what
    /// `ReflectionAttribute::getArguments()` resolves it through. php never
    /// evaluates an attribute's arguments until then, and neither does this.
    pub(crate) fn compile_attrs(
        &mut self,
        groups: &[rphp_ast::v2::AttrGroup],
        target: AttrTarget,
    ) -> Vec<AttrDef> {
        let mut out = Vec::new();
        for g in groups {
            for a in &g.attrs {
                let name: Box<[u8]> = {
                    let id = match a.name.resolved {
                        Some(rphp_ast::v2::Resolved::Class { fqn, .. }) => fqn,
                        _ => a.name.text,
                    };
                    self.interner().resolve(id).into()
                };
                let mut args = Vec::new();
                for arg in &a.args {
                    let init = self.compile_init(&arg.value);
                    let named = arg
                        .name
                        .map(|n| Box::from(self.interner().resolve(n)));
                    args.push((named, init));
                }
                out.push(AttrDef {
                    name,
                    args,
                    target,
                    line: self.mx.line(a.span.lo),
                });
            }
        }
        out
    }

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

    /// The name php reports for a default that is one constant fetch
    /// (`ReflectionParameter::getDefaultValueConstantName()`); `None` for
    /// any other expression.
    fn default_const_name(&self, e: &Expr) -> Option<Box<[u8]>> {
        let interner = self.mx.interner;
        match e {
            Expr::Const(name) => {
                let id = match name.resolved {
                    Some(Resolved::Const { ns_key, global_key }) => ns_key.unwrap_or(global_key),
                    _ => name.text,
                };
                Some(interner.resolve(id).into())
            }
            Expr::ClassConst {
                class,
                name: ConstSel::Ident(member, _),
                ..
            } => {
                let mut out: Vec<u8> = match class {
                    AstClassRef::Named(n) => match n.resolved {
                        Some(Resolved::Class { fqn, .. }) => interner.resolve(fqn).to_vec(),
                        _ => interner.resolve(n.text).strip_prefix(b"\\").unwrap_or(interner.resolve(n.text)).to_vec(),
                    },
                    AstClassRef::SelfKw(_) => b"self".to_vec(),
                    AstClassRef::Parent(_) => b"parent".to_vec(),
                    AstClassRef::Static(_) | AstClassRef::Expr(_) => return None,
                };
                out.extend_from_slice(b"::");
                out.extend_from_slice(interner.resolve(*member));
                Some(out.into())
            }
            _ => None,
        }
    }

    /// Compile `e` as a zero-argument function returning its value.
    pub(crate) fn compile_thunk(&mut self, e: &Expr) -> FuncId {
        compile_thunk_in(self.mx, self.diags, e, self.cur_class)
    }

    // ---- closures -------------------------------------------------------------

    /// Lower `function (...) use (...) { ... }`: compile its body as its own
    /// function (captures bound to the closure's registers), then emit a
    /// `MakeClosure` that snapshots the captured variables (or binds the
    /// by-reference ones) and the current `$this`/scope at runtime.
    pub(crate) fn compile_closure_expr(&mut self, c: &Closure) -> Reg {
        let uses: Vec<(IdentId, bool)> = c.uses.iter().map(|u| (u.name, u.by_ref)).collect();
        self.compile_closure(&c.params, &uses, ClosureBody::Stmts(&c.body), c.span, c.static_, c.ret.as_ref(), c.doc, &c.attrs)
    }

    /// Lower `fn (...) => e`: the free variables of `e` are captured by value
    /// (auto-capture) and the body is `return e;`.
    pub(crate) fn compile_arrow_fn(&mut self, f: &rphp_ast::v2::ArrowFn) -> Reg {
        let uses: Vec<(IdentId, bool)> = regs::arrow_free_vars(f, self.mx.interner)
            .into_iter()
            .map(|id| (id, false))
            .collect();
        self.compile_closure(&f.params, &uses, ClosureBody::ReturnExpr(&f.body), f.span, f.static_, f.ret.as_ref(), f.doc, &f.attrs)
    }

    fn compile_closure(
        &mut self,
        params: &[Param],
        uses: &[(IdentId, bool)],
        body: ClosureBody<'_>,
        span: Span,
        is_static: bool,
        ret: Option<&rphp_ast::v2::Type>,
        doc: Option<IdentId>,
        attrs: &[rphp_ast::v2::AttrGroup],
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
        fc.ret_ty = ret.map(|t| fc.lower_type(t));
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
        let lowered_attrs = fc.compile_attrs(attrs, AttrTarget::Function);
        let mut f = fc.finish(name.into(), defs, span);
        f.decl_line = self.mx.decl_line(attrs.last().map_or(span.lo, |g| g.span.hi));
        f.attrs = lowered_attrs;
        f.doc = doc.map(|d| Box::from(self.interner().resolve(d)));
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
        ret,
        doc,
        attrs,
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
    fc.ret_ty = ret.map(|t| fc.lower_type(t));
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
    let lowered_attrs = fc.compile_attrs(
        attrs,
        if cur_class.is_some() {
            AttrTarget::Method
        } else {
            AttrTarget::Function
        },
    );
    let mut f = fc.finish(name_bytes, defs, span);
    f.decl_line = mx.decl_line(attrs.last().map_or(span.lo, |g| g.span.hi));
    f.attrs = lowered_attrs;
    f.doc = doc.map(|d| Box::from(mx.interner.resolve(d)));
    mx.sink.borrow_mut().fill(id, f);
    id
}

/// Compile `e` as a zero-argument function returning its value, in `cur_class`'s
/// scope (so `self::X` and `new static` resolve). This is the form a property
/// default, a class constant and a parameter default take when the initializer
/// is not a literal: the runtime keeps the thunk and runs it on first use, so
/// linking a class never executes user code.
pub(crate) fn compile_thunk_in(
    mx: &ModuleCtx<'_>,
    diags: &mut Vec<Diagnostic>,
    e: &Expr,
    cur_class: Option<(ClassId, IdentId)>,
) -> FuncId {
    let id = mx.sink.borrow_mut().reserve();
    let mut fc = FnCompiler::new(
        mx,
        diags,
        &[],
        &[],
        ClosureBody::ReturnExpr(e),
        cur_class,
        Box::from(&b""[..]),
    );
    fc.mark_line(e.span());
    let r = fc.compile_expr(e);
    fc.emit(Op::Ret { src: Some(r) });
    let f = fc.finish(Box::from(&b""[..]), Vec::new(), e.span());
    mx.sink.borrow_mut().fill(id, f);
    id
}

/// Compile one property hook (PHP 8.4) as a method-like function of `cur_class`.
///
/// php names a hook `$prop::get` / `$prop::set` (that is what a backtrace and
/// `ReflectionProperty::getHooks()` show) and types it like the property: a
/// `get` hook takes nothing and returns the property's type, a `set` hook takes
/// the incoming value and returns `void`. The HIR pass has already given a
/// parameterless `set` its implicit `$value` parameter and rewritten the
/// `=> expr` shorthand into a block, so what arrives here is an ordinary body.
///
/// An abstract hook (`get;` in an interface or on an `abstract` property) has
/// no body and therefore no function: `None`.
pub(crate) fn compile_hook(
    mx: &ModuleCtx<'_>,
    diags: &mut Vec<Diagnostic>,
    prop: &[u8],
    prop_ty: Option<&Type>,
    h: &Hook,
    cur_class: (ClassId, IdentId),
) -> Option<FuncId> {
    let body: &[Stmt] = match &h.body {
        HookBody::Abstract => return None,
        HookBody::Block(b) => b,
        HookBody::Expr(e) => {
            // The HIR desugars both shorthands; a tree built by other means
            // still compiles the `get` form, which is exactly `return e;`.
            if h.kind == HookKind::Set {
                unsupported(diags, e.span(), "`set => expr` hook shorthand");
                return None;
            }
            &[]
        }
    };
    let params: &[Param] = h.params.as_deref().unwrap_or(&[]);
    let mut name_bytes: Vec<u8> = vec![b'$'];
    name_bytes.extend_from_slice(prop);
    name_bytes.extend_from_slice(b"::");
    name_bytes.extend_from_slice(h.kind.as_str().as_bytes());
    let name_bytes: Box<[u8]> = name_bytes.into();
    let id = mx.sink.borrow_mut().reserve();
    let expr_body = match &h.body {
        HookBody::Expr(e) => Some(e),
        _ => None,
    };
    let closure_body = match expr_body {
        Some(e) => ClosureBody::ReturnExpr(e),
        None => ClosureBody::Stmts(body),
    };
    let mut fc = FnCompiler::new(
        mx,
        diags,
        params,
        &[],
        closure_body,
        Some(cur_class),
        name_bytes.clone(),
    );
    // A hook is always an instance method, whether or not its body says `$this`.
    fc.flags |= FnFlags::USES_THIS;
    if h.by_ref {
        fc.flags |= FnFlags::RETURNS_REF;
    }
    fc.ret_ty = match h.kind {
        HookKind::Get => prop_ty.map(|t| fc.lower_type(t)),
        HookKind::Set => Some(TypeDecl::Builtin(BuiltinType::Void)),
    };
    fc.mark_line(h.span);
    let defs = fc.compile_params(params);
    match expr_body {
        Some(e) => {
            let mark = fc.temp_top;
            let r = fc.compile_expr(e);
            fc.emit(Op::Ret { src: Some(r) });
            fc.free_to(mark);
        }
        None => fc.compile_stmts(body),
    }
    fc.emit(Op::Ret { src: None });
    let f = fc.finish(name_bytes, defs, h.span);
    mx.sink.borrow_mut().fill(id, f);
    Some(id)
}

/// Lower a declared type outside a function body — a property's, a class
/// constant's — to its bytecode form, class names as FQNs.
///
/// The twin of [`FnCompiler::lower_type`], which the same mapping serves from
/// inside a body; a class-like member has no `FnCompiler` to ask.
pub(crate) fn lower_type_at(interner: &Interner, t: &Type) -> TypeDecl {
    match &t.kind {
        TypeKind::Named(name) => TypeDecl::Named(class_fqn(name, interner).into_boxed_slice()),
        TypeKind::Builtin(b) => TypeDecl::Builtin(builtin_type(*b)),
        TypeKind::Nullable(inner) => TypeDecl::Nullable(Box::new(lower_type_at(interner, inner))),
        TypeKind::Union(parts) => {
            TypeDecl::Union(parts.iter().map(|p| lower_type_at(interner, p)).collect())
        }
        TypeKind::Intersection(parts) => {
            TypeDecl::Intersection(parts.iter().map(|p| lower_type_at(interner, p)).collect())
        }
    }
}

/// The bytecode twin of a type keyword.
pub(crate) fn builtin_type(b: Builtin) -> BuiltinType {
    match b {
        Builtin::Int => BuiltinType::Int,
        Builtin::Float => BuiltinType::Float,
        Builtin::String => BuiltinType::String,
        Builtin::Bool => BuiltinType::Bool,
        Builtin::Array => BuiltinType::Array,
        Builtin::Object => BuiltinType::Object,
        Builtin::Mixed => BuiltinType::Mixed,
        Builtin::Void => BuiltinType::Void,
        Builtin::Never => BuiltinType::Never,
        Builtin::Null => BuiltinType::Null,
        Builtin::True => BuiltinType::True,
        Builtin::False => BuiltinType::False,
        Builtin::Callable => BuiltinType::Callable,
        Builtin::Iterable => BuiltinType::Iterable,
        Builtin::SelfTy => BuiltinType::SelfTy,
        Builtin::StaticTy => BuiltinType::StaticTy,
        Builtin::ParentTy => BuiltinType::ParentTy,
    }
}
