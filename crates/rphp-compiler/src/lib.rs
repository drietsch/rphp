//! Lower `rphp-ast` to `rphp-bytecode` for the M0 scalar subset.
//!
//! The lowering is a straightforward tree-walk into three-address register
//! bytecode (`rphp-bytecode`). A pre-pass assigns every top-level function a
//! [`FuncId`] (the synthetic `{main}` is `0`, user functions follow in
//! declaration order) so calls resolve regardless of source order. Each
//! function then pre-scans its body to give every variable a permanent
//! register; intermediate results use a stack of temporaries allocated above
//! the variable region.
//!
//! Errors (undefined function, wrong argument count, duplicate declaration) are
//! collected as [`Diagnostic`]s; the compile fails iff any of them is an error.
#![forbid(unsafe_code)]

use std::collections::HashMap;

use rphp_ast::{BinOp, Class, Expr, Func, Param, Program, Stmt, UnOp, Visibility as AstVis};
use rphp_bytecode::{
    Class as BcClass, ClassId, ClosureProto, CodeAddr, Const, FuncId, Function, Method as BcMethod,
    Module, Op, PropDef, Reg, Visibility,
};
use rphp_diagnostics::{codes, Diagnostic};
use rphp_intern::{IdentId, Interner};
use rphp_span::Span;
use rphp_value::{Str, Value};

/// Diagnostic code for a duplicate function declaration. `rphp-diagnostics`
/// does not (yet) expose a shared constant for this, so the compiler owns it.
const REDECLARED_FUNCTION: &str = "RPHP_E0102";
/// Writing through a nested subscript (`$a[i][j] = v`) is not lowered yet.
const NESTED_ARRAY_WRITE: &str = "RPHP_E0103";
/// Reading `$a[]` (the empty-subscript append form) is not a valid expression.
const INVALID_APPEND_READ: &str = "RPHP_E0104";
/// A by-reference parameter (e.g. `sort($a)`) was passed a non-variable.
const BY_REF_NOT_VARIABLE: &str = "RPHP_E0105";
/// A duplicate class declaration.
const REDECLARED_CLASS: &str = "RPHP_E0106";
/// `new Foo(...)` where `Foo` is not a declared class.
const UNDEFINED_CLASS: &str = "RPHP_E0107";
/// A property default that is not a constant expression.
const NON_CONST_PROP_DEFAULT: &str = "RPHP_E0108";
/// A scoped call (`self::m()` / `parent::m()` / `Class::m()`) to a method that
/// does not exist.
const UNDEFINED_METHOD: &str = "RPHP_E0109";
/// `self::`/`parent::` used outside a class, or `parent::` with no parent.
const INVALID_SCOPE: &str = "RPHP_E0110";

/// Compile a parsed program into a bytecode module. Function `0` is the
/// synthetic `{main}` entry containing the top-level statements; each top-level
/// `Stmt::Func` becomes its own [`Function`] appended afterwards.
pub fn compile(program: &Program, interner: &Interner) -> Result<Module, Vec<Diagnostic>> {
    let mut diags: Vec<Diagnostic> = Vec::new();

    // ---- pre-pass A: function ids (main = 0, user funcs 1..=U) ----
    let mut func_map: HashMap<IdentId, FuncId> = HashMap::new();
    let mut user_funcs: Vec<&Func> = Vec::new();
    // Argument counts indexed by FuncId; index 0 is `{main}` (never called).
    let mut arities: Vec<u16> = vec![0];
    let mut next_id: FuncId = 1;
    for item in &program.items {
        if let Stmt::Func(f) = item {
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
    }

    // ---- pre-pass B: class ids + method func ids (U+1 ..= U+M) ----
    // Methods compile to ordinary functions appended right after the user
    // functions. Their ids are fixed *before* any body is compiled, so closure
    // ids (`top_level_count + sink index`) stay stable as bodies are lowered.
    let mut class_map: HashMap<IdentId, ClassId> = HashMap::new();
    let mut user_classes: Vec<&Class> = Vec::new();
    // The FuncId for each method, parallel to each class's `.methods`.
    let mut method_ids: Vec<Vec<FuncId>> = Vec::new();
    // Whether each class declares its *own* `__construct` (chain-aware below).
    let mut own_ctor: Vec<bool> = Vec::new();
    let mut next_method_id: FuncId = next_id;
    for item in &program.items {
        if let Stmt::Class(c) = item {
            if class_map.contains_key(&c.name) {
                diags.push(
                    Diagnostic::error(
                        REDECLARED_CLASS,
                        format!("cannot redeclare class {}", interner.resolve_lossy(c.name)),
                    )
                    .with_primary(c.span, "duplicate declaration"),
                );
                continue;
            }
            class_map.insert(c.name, user_classes.len() as ClassId);
            let ids: Vec<FuncId> = c
                .methods
                .iter()
                .map(|_| {
                    let id = next_method_id;
                    next_method_id += 1;
                    id
                })
                .collect();
            own_ctor.push(
                c.methods
                    .iter()
                    .any(|m| interner.resolve(m.name).eq_ignore_ascii_case(b"__construct")),
            );
            method_ids.push(ids);
            user_classes.push(c);
        }
    }

    let n_classes = user_classes.len();

    // Resolve each `extends` target to a ClassId; diagnose unknown parents.
    let parent_id: Vec<Option<ClassId>> = user_classes
        .iter()
        .map(|c| match c.parent {
            None => None,
            Some(pname) => {
                let p = class_map.get(&pname).copied();
                if p.is_none() {
                    diags.push(
                        Diagnostic::error(
                            UNDEFINED_CLASS,
                            format!("class \"{}\" not found", interner.resolve_lossy(pname)),
                        )
                        .with_primary(c.span, "unknown parent class"),
                    );
                }
                p
            }
        })
        .collect();

    // Reject inheritance cycles — runtime chain walks must terminate.
    for ci in 0..n_classes {
        let mut cur = parent_id[ci];
        let mut hops = 0usize;
        while let Some(p) = cur {
            hops += 1;
            if hops > n_classes {
                diags.push(
                    Diagnostic::error(
                        REDECLARED_CLASS,
                        format!(
                            "class \"{}\" has a cyclic inheritance chain",
                            interner.resolve_lossy(user_classes[ci].name)
                        ),
                    )
                    .with_primary(user_classes[ci].span, "cyclic `extends`"),
                );
                break;
            }
            cur = parent_id[p as usize];
        }
    }

    // A `new` needs a constructor call iff one exists anywhere up the chain.
    let class_has_ctor: Vec<bool> = (0..n_classes)
        .map(|ci| {
            let mut cur = Some(ci as ClassId);
            let mut hops = 0usize;
            while let Some(cid) = cur {
                if own_ctor[cid as usize] {
                    return true;
                }
                hops += 1;
                if hops > n_classes {
                    break; // cycle (already diagnosed)
                }
                cur = parent_id[cid as usize];
            }
            false
        })
        .collect();

    // Per-class own-method table (name -> FuncId) for compile-time `self::` /
    // `parent::` / `Class::` resolution.
    let methods_ct: Vec<Vec<(Box<[u8]>, FuncId)>> = user_classes
        .iter()
        .enumerate()
        .map(|(ci, c)| {
            c.methods
                .iter()
                .enumerate()
                .map(|(mi, m)| (interner.resolve(m.name).into(), method_ids[ci][mi]))
                .collect()
        })
        .collect();

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
    let mut funcs: Vec<Function> = Vec::with_capacity(top_level_count as usize);
    let mut closure_sink: Vec<Function> = Vec::new();

    // Function 0: synthetic `{main}`. Non-`Func`/`Class` top-level statements
    // only — `compile_stmts` ignores both, so passing all items is correct.
    funcs.push(compile_function(
        interner, &func_map, &class_ctx, &arities, &mut diags, &mut closure_sink,
        top_level_count, IdentId(0), Box::from(&b""[..]), &[], &program.items, Span::dummy(), None,
    ));
    for f in &user_funcs {
        funcs.push(compile_function(
            interner, &func_map, &class_ctx, &arities, &mut diags, &mut closure_sink,
            top_level_count, f.name, interner.resolve(f.name).into(), &f.params, &f.body, f.span, None,
        ));
    }
    // Methods, in the same class-then-declaration order as pre-pass B so each
    // lands at exactly the FuncId reserved for it. Each carries its class as the
    // lexical context (`cur_class`) for `$this`, `self::`/`parent::`, visibility.
    for (ci, c) in user_classes.iter().enumerate() {
        for m in &c.methods {
            funcs.push(compile_function(
                interner, &func_map, &class_ctx, &arities, &mut diags, &mut closure_sink,
                top_level_count, m.name, interner.resolve(m.name).into(), &m.params, &m.body, m.span,
                Some(ci as ClassId),
            ));
        }
    }

    // Build the bytecode classes: defaults evaluated, methods linked to ids.
    let mut classes: Vec<BcClass> = Vec::with_capacity(user_classes.len());
    for (ci, c) in user_classes.iter().enumerate() {
        let props = c
            .props
            .iter()
            .map(|p| {
                let default = match &p.default {
                    None => Value::Null,
                    Some(e) => const_default(e, interner).unwrap_or_else(|| {
                        diags.push(
                            Diagnostic::error(
                                NON_CONST_PROP_DEFAULT,
                                "property default must be a constant expression",
                            )
                            .with_primary(p.span, "not a constant"),
                        );
                        Value::Null
                    }),
                };
                PropDef {
                    name: interner.resolve(p.name).into(),
                    default,
                    visibility: bc_vis(p.visibility),
                }
            })
            .collect();
        let methods = c
            .methods
            .iter()
            .enumerate()
            .map(|(mi, m)| BcMethod {
                name_bytes: interner.resolve(m.name).into(),
                func: method_ids[ci][mi],
                visibility: bc_vis(m.visibility),
            })
            .collect();
        classes.push(BcClass {
            name: c.name,
            name_bytes: interner.resolve(c.name).into(),
            parent: parent_id[ci],
            props,
            methods,
        });
    }

    if diags.iter().any(Diagnostic::is_error) {
        return Err(diags);
    }
    // Append the compiled closures so `FuncId`s line up with their indices.
    funcs.extend(closure_sink);
    Ok(Module { funcs, classes, main: 0 })
}

/// Constant-fold a property default. Only literals (and a unary minus over a
/// numeric literal) are supported; anything else returns `None` and is reported.
fn const_default(e: &Expr, interner: &Interner) -> Option<Value> {
    Some(match e {
        Expr::Null(_) => Value::Null,
        Expr::Bool(b, _) => Value::Bool(*b),
        Expr::Int(i, _) => Value::Int(*i),
        Expr::Float(f, _) => Value::Float(*f),
        Expr::Str(id, _) => Value::Str(Str::new(interner.resolve(*id))),
        Expr::Unary { op: UnOp::Neg, expr, .. } => match const_default(expr, interner)? {
            Value::Int(i) => i
                .checked_neg()
                .map(Value::Int)
                .unwrap_or(Value::Float(-(i as f64))),
            Value::Float(f) => Value::Float(-f),
            _ => return None,
        },
        _ => return None,
    })
}

/// Map an AST visibility to its bytecode counterpart.
fn bc_vis(v: AstVis) -> Visibility {
    match v {
        AstVis::Public => Visibility::Public,
        AstVis::Protected => Visibility::Protected,
        AstVis::Private => Visibility::Private,
    }
}

/// Shared class lookup tables threaded into every function compilation. Indexed
/// by [`ClassId`] except `map` (name -> id): the resolved parent of each class,
/// whether each has a constructor anywhere in its chain, and each class's *own*
/// methods (name -> FuncId) for compile-time scoped-call resolution.
/// A class's own methods as (name, compiled id) pairs.
type MethodTable = Vec<(Box<[u8]>, FuncId)>;

struct ClassCtx<'a> {
    map: &'a HashMap<IdentId, ClassId>,
    has_ctor: &'a [bool],
    parent: &'a [Option<ClassId>],
    methods: &'a [MethodTable],
}

impl ClassCtx<'_> {
    /// Resolve a method name on `class`, walking up the chain (compile time).
    fn resolve_method(&self, class: ClassId, name: &[u8]) -> Option<FuncId> {
        let mut cur = Some(class);
        while let Some(cid) = cur {
            for (n, f) in &self.methods[cid as usize] {
                if n.eq_ignore_ascii_case(name) {
                    return Some(*f);
                }
            }
            cur = self.parent[cid as usize];
        }
        None
    }
}

#[allow(clippy::too_many_arguments)]
fn compile_function(
    interner: &Interner,
    func_map: &HashMap<IdentId, FuncId>,
    class_ctx: &ClassCtx,
    arities: &[u16],
    diags: &mut Vec<Diagnostic>,
    closure_sink: &mut Vec<Function>,
    top_level_count: FuncId,
    name: IdentId,
    name_bytes: Box<[u8]>,
    params: &[Param],
    body: &[Stmt],
    span: Span,
    cur_class: Option<ClassId>,
) -> Function {
    let mut fc = FnCompiler::new(
        interner,
        func_map,
        class_ctx,
        arities,
        diags,
        closure_sink,
        top_level_count,
        params,
        &[],
        body,
        cur_class,
    );
    fc.compile_stmts(body);
    // Always terminate with a fall-through return so every code path (and every
    // branch target that lands at the textual end) has a valid `Ret`.
    fc.emit(Op::Ret { src: None });
    Function {
        name,
        name_bytes,
        // A method's register 0 holds the implicit `$this`, so its declared
        // parameters occupy registers `1 ..= n` and the frame takes `n + 1`.
        num_params: params.len() as u16 + u16::from(cur_class.is_some()),
        num_regs: fc.num_regs,
        code: fc.code,
        consts: fc.consts,
        capture_regs: fc.capture_regs,
        closures: fc.closures,
        span,
    }
}

struct FnCompiler<'a> {
    interner: &'a Interner,
    func_map: &'a HashMap<IdentId, FuncId>,
    class_ctx: &'a ClassCtx<'a>,
    arities: &'a [u16],
    diags: &'a mut Vec<Diagnostic>,
    /// Where nested closures register their compiled `Function`s. Their `FuncId`
    /// is `top_level_count + index` (the sink is appended after the top-level
    /// functions), so ids stay stable as the sink grows.
    closure_sink: &'a mut Vec<Function>,
    top_level_count: FuncId,
    /// The class whose method is being compiled (lexical context for `$this`,
    /// `self::`/`parent::`, and visibility); `None` outside a method.
    cur_class: Option<ClassId>,

    /// Variable -> permanent register. Variables occupy the low registers
    /// (params first, then captured `use` vars, then locals); temporaries live
    /// above them.
    vars: HashMap<IdentId, Reg>,
    /// Current top of the temporary stack (next free temp register).
    temp_top: Reg,
    /// High-water mark: total registers the frame needs.
    num_regs: Reg,
    /// Registers a closure body binds its captures to, in capture order (empty
    /// for an ordinary function).
    capture_regs: Vec<Reg>,
    /// Closure templates this function emits via `Op::MakeClosure`.
    closures: Vec<ClosureProto>,

    code: Vec<Op>,
    consts: Vec<Const>,
}

impl<'a> FnCompiler<'a> {
    #[allow(clippy::too_many_arguments)]
    fn new(
        interner: &'a Interner,
        func_map: &'a HashMap<IdentId, FuncId>,
        class_ctx: &'a ClassCtx<'a>,
        arities: &'a [u16],
        diags: &'a mut Vec<Diagnostic>,
        closure_sink: &'a mut Vec<Function>,
        top_level_count: FuncId,
        params: &[Param],
        captures: &[IdentId],
        body: &[Stmt],
        cur_class: Option<ClassId>,
    ) -> Self {
        // A method reserves register 0 for the implicit `$this`; parameters then
        // start at register 1. (If the body names `$this`, the lexer has interned
        // it, so we can bind that id to register 0.)
        let is_method = cur_class.is_some();
        let mut vars: HashMap<IdentId, Reg> = HashMap::new();
        let mut base = 0;
        if is_method {
            if let Some(this_id) = interner.get(b"this") {
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
        collect_stmts(body, &mut vars, &mut var_count);

        FnCompiler {
            interner,
            func_map,
            class_ctx,
            arities,
            diags,
            closure_sink,
            top_level_count,
            cur_class,
            vars,
            temp_top: var_count,
            num_regs: var_count,
            capture_regs,
            closures: Vec::new(),
            code: Vec::new(),
            consts: Vec::new(),
        }
    }

    // ---- low-level helpers --------------------------------------------------

    fn emit(&mut self, op: Op) -> usize {
        self.code.push(op);
        self.code.len() - 1
    }

    fn here(&self) -> CodeAddr {
        self.code.len() as CodeAddr
    }

    fn patch(&mut self, idx: usize, target: CodeAddr) {
        match &mut self.code[idx] {
            Op::Jmp { target: t }
            | Op::JmpIfTrue { target: t, .. }
            | Op::JmpIfFalse { target: t, .. }
            | Op::ForeachNext { target: t, .. } => *t = target,
            _ => unreachable!("patch on a non-branch op"),
        }
    }

    fn set_top(&mut self, n: Reg) {
        self.temp_top = n;
        if n > self.num_regs {
            self.num_regs = n;
        }
    }

    /// Allocate a fresh temporary register.
    fn alloc_temp(&mut self) -> Reg {
        let r = self.temp_top;
        self.set_top(self.temp_top + 1);
        r
    }

    /// Release temporaries down to `mark` (does not lower the high-water mark).
    fn free_to(&mut self, mark: Reg) {
        self.temp_top = mark;
    }

    fn push_const(&mut self, c: Const) -> u32 {
        let k = self.consts.len() as u32;
        self.consts.push(c);
        k
    }

    fn var_reg(&self, id: IdentId) -> Reg {
        *self
            .vars
            .get(&id)
            .expect("every variable is assigned a register during the pre-scan")
    }

    // ---- statements ---------------------------------------------------------

    fn compile_stmts(&mut self, stmts: &[Stmt]) {
        for s in stmts {
            self.compile_stmt(s);
        }
    }

    fn compile_stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Echo { args, .. } => {
                for a in args {
                    let mark = self.temp_top;
                    let r = self.compile_expr(a);
                    self.emit(Op::Echo { src: r });
                    self.free_to(mark);
                }
            }
            Stmt::Expr(e) => {
                let mark = self.temp_top;
                self.compile_expr(e);
                self.free_to(mark);
            }
            Stmt::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => {
                let mark = self.temp_top;
                let rc = self.compile_expr(cond);
                let jf = self.emit(Op::JmpIfFalse {
                    cond: rc,
                    target: 0,
                });
                self.free_to(mark);
                self.compile_stmts(then_branch);
                if else_branch.is_empty() {
                    let lend = self.here();
                    self.patch(jf, lend);
                } else {
                    let jend = self.emit(Op::Jmp { target: 0 });
                    let lelse = self.here();
                    self.patch(jf, lelse);
                    self.compile_stmts(else_branch);
                    let lend = self.here();
                    self.patch(jend, lend);
                }
            }
            Stmt::While { cond, body, .. } => {
                let ltop = self.here();
                let mark = self.temp_top;
                let rc = self.compile_expr(cond);
                let jf = self.emit(Op::JmpIfFalse {
                    cond: rc,
                    target: 0,
                });
                self.free_to(mark);
                self.compile_stmts(body);
                self.emit(Op::Jmp { target: ltop });
                let lend = self.here();
                self.patch(jf, lend);
            }
            Stmt::Foreach { subject, key_var, value_var, body, .. } => {
                let mark = self.temp_top;
                // Snapshot the subject into a temp we own — a separate COW handle
                // — so the body mutating the original array cannot disturb the
                // iteration (PHP foreach iterates over a copy).
                let sr = self.compile_expr(subject);
                let arr = self.alloc_temp();
                self.emit(Op::Move { dst: arr, src: sr });
                // The iteration cursor (a position into the entry list).
                let cursor = self.alloc_temp();
                let k0 = self.push_const(Const::Int(0));
                self.emit(Op::LoadConst { dst: cursor, k: k0 });
                let val_dst = self.var_reg(*value_var);
                let key_dst = match key_var {
                    Some(k) => self.var_reg(*k),
                    None => self.alloc_temp(), // throwaway key sink
                };
                let ltop = self.here();
                let next = self.emit(Op::ForeachNext { arr, cursor, key_dst, val_dst, target: 0 });
                self.compile_stmts(body);
                self.emit(Op::Jmp { target: ltop });
                let lend = self.here();
                self.patch(next, lend);
                self.free_to(mark);
            }
            Stmt::Return { value, .. } => match value {
                Some(e) => {
                    let mark = self.temp_top;
                    let r = self.compile_expr(e);
                    self.emit(Op::Ret { src: Some(r) });
                    self.free_to(mark);
                }
                None => {
                    self.emit(Op::Ret { src: None });
                }
            },
            // Function and class declarations are compiled as their own
            // `Function`s/`Class`es by the driver; they emit no code in a frame.
            Stmt::Func(_) | Stmt::Class(_) => {}
        }
    }

    // ---- expressions --------------------------------------------------------

    /// Compile `e`, returning the register that holds its value. Invariant:
    /// the call leaves exactly one extra live temporary (the result) when the
    /// result is a fresh temp, or zero when it is an existing variable register.
    fn compile_expr(&mut self, e: &Expr) -> Reg {
        match e {
            Expr::Null(_) => {
                let dst = self.alloc_temp();
                self.emit(Op::LoadNull { dst });
                dst
            }
            Expr::Bool(b, _) => {
                let dst = self.alloc_temp();
                self.emit(Op::LoadBool { dst, val: *b });
                dst
            }
            Expr::Int(i, _) => {
                let k = self.push_const(Const::Int(*i));
                let dst = self.alloc_temp();
                self.emit(Op::LoadConst { dst, k });
                dst
            }
            Expr::Float(f, _) => {
                let k = self.push_const(Const::Float(*f));
                let dst = self.alloc_temp();
                self.emit(Op::LoadConst { dst, k });
                dst
            }
            Expr::Str(id, _) => {
                let k = self.push_const(Const::Str(Str::new(self.interner.resolve(*id))));
                let dst = self.alloc_temp();
                self.emit(Op::LoadConst { dst, k });
                dst
            }
            Expr::Var(id, _) => self.var_reg(*id),
            Expr::Assign { target, value, .. } => {
                let dst = self.var_reg(*target);
                let mark = self.temp_top;
                let r = self.compile_expr(value);
                if r != dst {
                    self.emit(Op::Move { dst, src: r });
                }
                self.free_to(mark);
                // The assignment expression evaluates to the assigned register.
                dst
            }
            Expr::Unary { op, expr, .. } => {
                let mark = self.temp_top;
                let r = self.compile_expr(expr);
                self.free_to(mark);
                let dst = self.alloc_temp();
                let op = match op {
                    UnOp::Neg => Op::Neg { dst, src: r },
                    UnOp::Not => Op::Not { dst, src: r },
                };
                self.emit(op);
                dst
            }
            Expr::Binary { op, lhs, rhs, .. } => match op {
                BinOp::And => self.compile_and(lhs, rhs),
                BinOp::Or => self.compile_or(lhs, rhs),
                _ => {
                    let mark = self.temp_top;
                    let a = self.compile_expr(lhs);
                    let b = self.compile_expr(rhs);
                    self.free_to(mark);
                    let dst = self.alloc_temp();
                    self.emit(binary_op(*op, dst, a, b));
                    dst
                }
            },
            Expr::Call { name, args, span } => self.compile_call(*name, args, *span),
            Expr::CallDynamic { callee, args, .. } => self.compile_dynamic_call(callee, args),
            Expr::Closure { params, uses, body, span } => self.compile_closure(params, uses, body, *span),
            Expr::Array { items, .. } => self.compile_array(items),
            Expr::Index { base, index, span } => self.compile_index_read(base, index.as_deref(), *span),
            Expr::IndexAssign { base, index, value, span } => {
                self.compile_index_assign(base, index.as_deref(), value, *span)
            }
            Expr::New { class, args, span } => self.compile_new(*class, args, *span),
            Expr::PropGet { obj, name, .. } => self.compile_prop_get(obj, *name),
            Expr::PropSet { obj, name, value, .. } => self.compile_prop_set(obj, *name, value),
            Expr::MethodCall { obj, method, args, .. } => self.compile_method_call(obj, *method, args),
            Expr::StaticCall { class, method, args, span } => {
                self.compile_static_call(*class, *method, args, *span)
            }
            Expr::InstanceOf { expr, class, span } => self.compile_instance_of(expr, *class, *span),
        }
    }

    /// `[ ... ]` / `array( ... )`: build a fresh array, then fill it element by
    /// element preserving source order.
    fn compile_array(&mut self, items: &[rphp_ast::ArrayItem]) -> Reg {
        let dst = self.alloc_temp();
        self.emit(Op::NewArray { dst });
        let mark = self.temp_top;
        for item in items {
            match &item.key {
                Some(key) => {
                    let kr = self.compile_expr(key);
                    let vr = self.compile_expr(&item.value);
                    self.emit(Op::ArraySet { arr: dst, key: kr, value: vr });
                }
                None => {
                    let vr = self.compile_expr(&item.value);
                    self.emit(Op::ArrayPush { arr: dst, value: vr });
                }
            }
            self.free_to(mark);
        }
        dst
    }

    /// `base[index]` read. `$a[]` (no index) is not a readable expression.
    fn compile_index_read(&mut self, base: &Expr, index: Option<&Expr>, span: Span) -> Reg {
        let Some(index) = index else {
            self.diags.push(
                Diagnostic::error(INVALID_APPEND_READ, "cannot use `[]` for reading")
                    .with_primary(span, "expected an index"),
            );
            let dst = self.alloc_temp();
            self.emit(Op::LoadNull { dst });
            return dst;
        };
        let mark = self.temp_top;
        let br = self.compile_expr(base);
        let kr = self.compile_expr(index);
        self.free_to(mark);
        let dst = self.alloc_temp();
        self.emit(Op::ArrayGet { dst, base: br, key: kr });
        dst
    }

    /// `base[index] = value` / `base[] = value`. Only a plain `$var` base is
    /// supported so far (nested-subscript writes need an lvalue chain). The
    /// expression evaluates to the assigned value.
    fn compile_index_assign(
        &mut self,
        base: &Expr,
        index: Option<&Expr>,
        value: &Expr,
        span: Span,
    ) -> Reg {
        let Expr::Var(id, _) = base else {
            self.diags.push(
                Diagnostic::error(NESTED_ARRAY_WRITE, "nested array assignment is not supported yet")
                    .with_primary(span, "write through a single `$var[...]` for now"),
            );
            let dst = self.alloc_temp();
            self.emit(Op::LoadNull { dst });
            return dst;
        };
        let arr = self.var_reg(*id);
        // The assigned value is the result of the expression, so keep it live
        // while the (freed) index temp sits above it.
        let mark = self.temp_top;
        let vr = self.compile_expr(value);
        let key_mark = self.temp_top;
        match index {
            Some(index) => {
                let kr = self.compile_expr(index);
                self.emit(Op::ArraySet { arr, key: kr, value: vr });
            }
            None => {
                self.emit(Op::ArrayPush { arr, value: vr });
            }
        }
        self.free_to(key_mark); // release the index temp, keep `vr`
        let _ = mark;
        vr
    }

    /// `a && b` with short-circuit; result register holds a real bool.
    fn compile_and(&mut self, lhs: &Expr, rhs: &Expr) -> Reg {
        let dst = self.alloc_temp();
        let mark = self.temp_top;
        let ra = self.compile_expr(lhs);
        let jf = self.emit(Op::JmpIfFalse {
            cond: ra,
            target: 0,
        });
        self.free_to(mark);
        // True path: dst = (bool) b, via double logical-negation.
        let rb = self.compile_expr(rhs);
        self.emit(Op::Not { dst, src: rb });
        self.emit(Op::Not { dst, src: dst });
        self.free_to(mark);
        let jend = self.emit(Op::Jmp { target: 0 });
        // False path: lhs was falsy -> result is `false`.
        let lfalse = self.here();
        self.patch(jf, lfalse);
        self.emit(Op::LoadBool { dst, val: false });
        let lend = self.here();
        self.patch(jend, lend);
        dst
    }

    /// `a || b` with short-circuit; result register holds a real bool.
    fn compile_or(&mut self, lhs: &Expr, rhs: &Expr) -> Reg {
        let dst = self.alloc_temp();
        let mark = self.temp_top;
        let ra = self.compile_expr(lhs);
        let jt = self.emit(Op::JmpIfTrue {
            cond: ra,
            target: 0,
        });
        self.free_to(mark);
        // Fall-through path: lhs was falsy -> result = (bool) b.
        let rb = self.compile_expr(rhs);
        self.emit(Op::Not { dst, src: rb });
        self.emit(Op::Not { dst, src: dst });
        self.free_to(mark);
        let jend = self.emit(Op::Jmp { target: 0 });
        // True path: lhs was truthy -> result is `true`.
        let ltrue = self.here();
        self.patch(jt, ltrue);
        self.emit(Op::LoadBool { dst, val: true });
        let lend = self.here();
        self.patch(jend, lend);
        dst
    }

    fn compile_call(&mut self, name: IdentId, args: &[Expr], span: Span) -> Reg {
        let argc = args.len() as u16;

        // Resolve the callee: a user function takes precedence over a builtin of
        // the same name; a builtin is matched case-insensitively by its bytes.
        let target = if let Some(&id) = self.func_map.get(&name) {
            self.check_user_arity(name, id, argc, span);
            Some(CallTarget::User(id))
        } else if let Some(nid) = rphp_stdlib::resolve(self.interner.resolve(name)) {
            self.check_native_arity(name, nid, argc, span);
            Some(CallTarget::Native(nid.0))
        } else {
            self.diags.push(
                Diagnostic::error(
                    codes::UNDEFINED_FUNCTION,
                    format!(
                        "call to undefined function {}()",
                        self.interner.resolve_lossy(name)
                    ),
                )
                .with_primary(span, "not defined"),
            );
            None
        };
        let Some(target) = target else {
            let dst = self.alloc_temp();
            self.emit(Op::LoadNull { dst });
            return dst;
        };

        // Builtins may declare by-reference parameters (user by-ref is not
        // modelled yet). A call that actually passes an argument into a by-ref
        // slot needs a write-back, handled on a separate path.
        let by_ref = match &target {
            CallTarget::Native(n) => rphp_stdlib::descriptor(rphp_stdlib::NativeId(*n)).by_ref,
            CallTarget::User(_) => 0,
        };
        if let (true, CallTarget::Native(native)) =
            ((0..argc).any(|i| by_ref & (1 << i) != 0), &target)
        {
            return self.compile_native_by_ref(name, *native, by_ref, args, argc);
        }

        // Stage args into the contiguous window `base ..= base+argc-1`.
        let base = self.temp_top;
        self.set_top(base + argc);
        for (i, arg) in args.iter().enumerate() {
            let slot = base + i as Reg;
            let mark = self.temp_top;
            let r = self.compile_expr(arg);
            if r != slot {
                self.emit(Op::Move { dst: slot, src: r });
            }
            self.free_to(mark);
        }
        // Free the window; the result lands in `dst == base` (the runtime copies
        // the args into the callee frame before writing the return value).
        self.free_to(base);
        let dst = self.alloc_temp();
        debug_assert_eq!(dst, base);
        let op = match target {
            CallTarget::User(func) => Op::Call { dst, func, base, argc },
            CallTarget::Native(native) => Op::CallNative { dst, native, base, argc },
        };
        self.emit(op);
        dst
    }

    /// Lower a builtin call that passes one or more arguments **by reference**
    /// (`sort($a)`, `array_push($a, …)`, `preg_match($p, $s, $m)`). A by-ref
    /// argument must be a plain variable; its value is copied into the call
    /// window, and after the call the (mutated) window slot is copied back into
    /// that variable. The result is brought down to a single temporary so the
    /// usual "the result is the top live temp" invariant still holds.
    fn compile_native_by_ref(
        &mut self,
        name: IdentId,
        native: u32,
        by_ref: u32,
        args: &[Expr],
        argc: u16,
    ) -> Reg {
        let base = self.temp_top;
        self.set_top(base + argc);
        // (variable register, window slot) pairs to copy back after the call.
        let mut write_backs: Vec<(Reg, Reg)> = Vec::new();
        for (i, arg) in args.iter().enumerate() {
            let slot = base + i as Reg;
            if by_ref & (1 << i) != 0 {
                if let Expr::Var(id, _) = arg {
                    let vr = self.var_reg(*id);
                    self.emit(Op::Move { dst: slot, src: vr });
                    write_backs.push((vr, slot));
                    continue;
                }
                self.diags.push(
                    Diagnostic::error(
                        BY_REF_NOT_VARIABLE,
                        format!(
                            "{}(): only a variable can be passed by reference",
                            self.interner.resolve_lossy(name)
                        ),
                    )
                    .with_primary(arg.span(), "not a variable"),
                );
            }
            let mark = self.temp_top;
            let r = self.compile_expr(arg);
            if r != slot {
                self.emit(Op::Move { dst: slot, src: r });
            }
            self.free_to(mark);
        }
        // The result goes into a temp ABOVE the window, so it cannot alias a
        // by-ref slot the runtime writes back into the window.
        let dst_high = self.alloc_temp();
        debug_assert_eq!(dst_high, base + argc);
        self.emit(Op::CallNative { dst: dst_high, native, base, argc });
        // Copy each mutated by-ref slot back into its variable.
        for (vr, slot) in &write_backs {
            self.emit(Op::Move { dst: *vr, src: *slot });
        }
        // Bring the result down to `base`, releasing the window and the high temp.
        self.emit(Op::Move { dst: base, src: dst_high });
        self.free_to(base + 1);
        base
    }

    /// A user function takes a fixed parameter count (defaults/variadics are not
    /// modelled yet), so the arg count must match exactly.
    fn check_user_arity(&mut self, name: IdentId, id: FuncId, argc: u16, span: Span) {
        let expected = self.arities[id as usize];
        if argc != expected {
            self.diags.push(
                Diagnostic::error(
                    codes::WRONG_ARG_COUNT,
                    format!(
                        "function {}() expects {} argument(s), {} given",
                        self.interner.resolve_lossy(name),
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
    fn check_native_arity(&mut self, name: IdentId, nid: rphp_stdlib::NativeId, argc: u16, span: Span) {
        let desc = rphp_stdlib::descriptor(nid);
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
                        self.interner.resolve_lossy(name),
                    ),
                )
                .with_primary(span, "wrong number of arguments"),
            );
        }
    }
}

impl FnCompiler<'_> {
    /// Lower `callee(args...)` where the callee is a runtime value (a closure or
    /// callable string). The callee is evaluated first and kept live below the
    /// argument window; the runtime resolves and invokes it.
    fn compile_dynamic_call(&mut self, callee: &Expr, args: &[Expr]) -> Reg {
        let argc = args.len() as u16;
        let callee_reg = self.compile_expr(callee);
        // Stage args into a fresh window above the (still-live) callee register.
        let base = self.temp_top;
        self.set_top(base + argc);
        for (i, arg) in args.iter().enumerate() {
            let slot = base + i as Reg;
            let mark = self.temp_top;
            let r = self.compile_expr(arg);
            if r != slot {
                self.emit(Op::Move { dst: slot, src: r });
            }
            self.free_to(mark);
        }
        self.free_to(base);
        let dst = self.alloc_temp();
        debug_assert_eq!(dst, base);
        self.emit(Op::CallDynamic { dst, callee: callee_reg, base, argc });
        dst
    }

    /// Intern a member name (property / method) as a string constant in the
    /// pool, returning its index — the form `PropGet`/`PropSet`/`MethodCall` use.
    fn name_const(&mut self, name: IdentId) -> u32 {
        self.push_const(Const::Str(Str::new(self.interner.resolve(name))))
    }

    /// `new Class(args...)`: allocate the instance with its default properties,
    /// then — if the class declares a constructor — invoke `__construct` with the
    /// arguments, discarding its result. Evaluates to the new object.
    fn compile_new(&mut self, class: IdentId, args: &[Expr], span: Span) -> Reg {
        let Some(&cid) = self.class_ctx.map.get(&class) else {
            self.diags.push(
                Diagnostic::error(
                    UNDEFINED_CLASS,
                    format!("class \"{}\" not found", self.interner.resolve_lossy(class)),
                )
                .with_primary(span, "not defined"),
            );
            let dst = self.alloc_temp();
            self.emit(Op::LoadNull { dst });
            return dst;
        };
        let dst = self.alloc_temp();
        self.emit(Op::New { dst, class: cid });
        if self.class_ctx.has_ctor[cid as usize] {
            let argc = args.len() as u16;
            // Stage constructor args in a window *above* the object register, so
            // the object (the result) is never clobbered.
            let base = self.temp_top;
            self.set_top(base + argc);
            for (i, arg) in args.iter().enumerate() {
                let slot = base + i as Reg;
                let mark = self.temp_top;
                let r = self.compile_expr(arg);
                if r != slot {
                    self.emit(Op::Move { dst: slot, src: r });
                }
                self.free_to(mark);
            }
            self.free_to(base);
            let ret = self.alloc_temp(); // constructor result, discarded
            let method = self.push_const(Const::Str(Str::new(b"__construct")));
            self.emit(Op::MethodCall { dst: ret, obj: dst, method, base, argc });
            self.free_to(dst + 1); // release the window and discarded result
        }
        dst
    }

    /// `obj->name` property read.
    fn compile_prop_get(&mut self, obj: &Expr, name: IdentId) -> Reg {
        let mark = self.temp_top;
        let obj_reg = self.compile_expr(obj);
        self.free_to(mark);
        let dst = self.alloc_temp();
        let name = self.name_const(name);
        self.emit(Op::PropGet { dst, obj: obj_reg, name });
        dst
    }

    /// `obj->name = value`. The value is the expression's result; the write goes
    /// through the object's shared cell (objects are reference handles), so `obj`
    /// may be any expression, not just a variable.
    fn compile_prop_set(&mut self, obj: &Expr, name: IdentId, value: &Expr) -> Reg {
        let vr = self.compile_expr(value);
        let obj_mark = self.temp_top;
        let obj_reg = self.compile_expr(obj);
        let name = self.name_const(name);
        self.emit(Op::PropSet { obj: obj_reg, name, value: vr });
        self.free_to(obj_mark); // drop the object temp, keep the value
        vr
    }

    /// `obj->method(args...)`. The object is evaluated and kept live below the
    /// argument window; the runtime binds it to the callee's `$this`.
    fn compile_method_call(&mut self, obj: &Expr, method: IdentId, args: &[Expr]) -> Reg {
        let argc = args.len() as u16;
        let obj_reg = self.compile_expr(obj);
        let base = self.temp_top;
        self.set_top(base + argc);
        for (i, arg) in args.iter().enumerate() {
            let slot = base + i as Reg;
            let mark = self.temp_top;
            let r = self.compile_expr(arg);
            if r != slot {
                self.emit(Op::Move { dst: slot, src: r });
            }
            self.free_to(mark);
        }
        self.free_to(base);
        let dst = self.alloc_temp();
        debug_assert_eq!(dst, base);
        let method = self.name_const(method);
        self.emit(Op::MethodCall { dst, obj: obj_reg, method, base, argc });
        dst
    }

    /// `class::method(args...)` — a scoped (non-virtual) call. Resolves the
    /// target class (`self`/`parent`/name) and the method (compile time, walking
    /// the chain), then forwards the current `$this` (register 0 in a method) and
    /// the arguments to the resolved function.
    fn compile_static_call(&mut self, class: IdentId, method: IdentId, args: &[Expr], span: Span) -> Reg {
        let cname = self.interner.resolve(class);
        let target: Option<ClassId> = if cname.eq_ignore_ascii_case(b"self") {
            if self.cur_class.is_none() {
                self.scope_error("cannot use \"self::\" outside a class", span);
            }
            self.cur_class
        } else if cname.eq_ignore_ascii_case(b"parent") {
            match self.cur_class {
                None => {
                    self.scope_error("cannot use \"parent::\" outside a class", span);
                    None
                }
                Some(c) => {
                    let p = self.class_ctx.parent[c as usize];
                    if p.is_none() {
                        self.scope_error("current class has no parent", span);
                    }
                    p
                }
            }
        } else {
            let t = self.class_ctx.map.get(&class).copied();
            if t.is_none() {
                self.diags.push(
                    Diagnostic::error(
                        UNDEFINED_CLASS,
                        format!("class \"{}\" not found", self.interner.resolve_lossy(class)),
                    )
                    .with_primary(span, "not defined"),
                );
            }
            t
        };

        let func = target.and_then(|c| self.class_ctx.resolve_method(c, self.interner.resolve(method)));
        let Some(func) = func else {
            if target.is_some() {
                self.diags.push(
                    Diagnostic::error(
                        UNDEFINED_METHOD,
                        format!("call to undefined method {}()", self.interner.resolve_lossy(method)),
                    )
                    .with_primary(span, "no such method"),
                );
            }
            let dst = self.alloc_temp();
            self.emit(Op::LoadNull { dst });
            return dst;
        };

        // `$this` to forward: register 0 inside a method, otherwise a fresh null.
        let this_reg = if self.cur_class.is_some() {
            0
        } else {
            let t = self.alloc_temp();
            self.emit(Op::LoadNull { dst: t });
            t
        };
        let argc = args.len() as u16;
        let base = self.temp_top;
        self.set_top(base + argc);
        for (i, arg) in args.iter().enumerate() {
            let slot = base + i as Reg;
            let mark = self.temp_top;
            let r = self.compile_expr(arg);
            if r != slot {
                self.emit(Op::Move { dst: slot, src: r });
            }
            self.free_to(mark);
        }
        self.free_to(base);
        let dst = self.alloc_temp();
        debug_assert_eq!(dst, base);
        self.emit(Op::StaticCall { dst, this: this_reg, func, base, argc });
        dst
    }

    /// `expr instanceof Class`. The class name resolves at compile time; an
    /// unknown name yields a constant `false` (PHP does not error there), while
    /// `self`/`parent` outside a class is a hard error.
    fn compile_instance_of(&mut self, expr: &Expr, class: IdentId, span: Span) -> Reg {
        let cname = self.interner.resolve(class);
        let target: Option<ClassId> = if cname.eq_ignore_ascii_case(b"self") {
            if self.cur_class.is_none() {
                self.scope_error("cannot use \"self\" outside a class", span);
            }
            self.cur_class
        } else if cname.eq_ignore_ascii_case(b"parent") {
            match self.cur_class {
                None => {
                    self.scope_error("cannot use \"parent\" outside a class", span);
                    None
                }
                Some(c) => self.class_ctx.parent[c as usize],
            }
        } else {
            self.class_ctx.map.get(&class).copied()
        };

        let mark = self.temp_top;
        let obj_reg = self.compile_expr(expr);
        self.free_to(mark);
        let dst = self.alloc_temp();
        match target {
            Some(cid) => {
                self.emit(Op::InstanceOf { dst, obj: obj_reg, class: cid });
            }
            None => {
                self.emit(Op::LoadBool { dst, val: false });
            }
        }
        dst
    }

    /// Push a `self::`/`parent::` scope-misuse diagnostic.
    fn scope_error(&mut self, msg: &str, span: Span) {
        self.diags
            .push(Diagnostic::error(INVALID_SCOPE, msg).with_primary(span, "invalid scope"));
    }

    /// Lower a closure expression: capture the current values of its `use`
    /// variables from this frame, compile its body as its own function, and emit
    /// a `MakeClosure` that binds them together at runtime.
    fn compile_closure(
        &mut self,
        params: &[Param],
        uses: &[IdentId],
        body: &[Stmt],
        span: Span,
    ) -> Reg {
        // Registers in *this* frame holding the captured variables' current
        // values (snapshotted by value when the closure is built).
        let src_regs: Vec<Reg> = uses.iter().map(|u| self.var_reg(*u)).collect();
        let closure_fn = self.compile_closure_fn(params, uses, body, span);
        // FuncId = top-level count + position in the sink (nested closures of
        // this body were already pushed during compilation).
        let func = self.top_level_count + self.closure_sink.len() as FuncId;
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
        body: &[Stmt],
        span: Span,
    ) -> Function {
        let mut fc = FnCompiler::new(
            self.interner,
            self.func_map,
            self.class_ctx,
            self.arities,
            &mut *self.diags,
            &mut *self.closure_sink,
            self.top_level_count,
            params,
            uses,
            body,
            None,
        );
        fc.compile_stmts(body);
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
        }
    }
}

/// What a call site resolves to during lowering.
enum CallTarget {
    /// A user-defined function, by [`FuncId`].
    User(FuncId),
    /// A builtin, by its `rphp-stdlib` registry index.
    Native(u32),
}

fn binary_op(op: BinOp, dst: Reg, a: Reg, b: Reg) -> Op {
    match op {
        BinOp::Add => Op::Add { dst, a, b },
        BinOp::Sub => Op::Sub { dst, a, b },
        BinOp::Mul => Op::Mul { dst, a, b },
        BinOp::Div => Op::Div { dst, a, b },
        BinOp::Mod => Op::Mod { dst, a, b },
        BinOp::Pow => Op::Pow { dst, a, b },
        BinOp::Concat => Op::Concat { dst, a, b },
        BinOp::Eq => Op::CmpEq { dst, a, b },
        BinOp::Ne => Op::CmpNe { dst, a, b },
        BinOp::Identical => Op::CmpIdentical { dst, a, b },
        BinOp::NotIdentical => Op::CmpNotIdentical { dst, a, b },
        BinOp::Lt => Op::CmpLt { dst, a, b },
        BinOp::Le => Op::CmpLe { dst, a, b },
        BinOp::Gt => Op::CmpGt { dst, a, b },
        BinOp::Ge => Op::CmpGe { dst, a, b },
        BinOp::Spaceship => Op::Spaceship { dst, a, b },
        BinOp::And | BinOp::Or => unreachable!("&&/|| are lowered to branches"),
    }
}

// ---- variable pre-scan ------------------------------------------------------

fn collect_stmts(stmts: &[Stmt], vars: &mut HashMap<IdentId, Reg>, next: &mut Reg) {
    for s in stmts {
        collect_stmt(s, vars, next);
    }
}

fn collect_stmt(s: &Stmt, vars: &mut HashMap<IdentId, Reg>, next: &mut Reg) {
    match s {
        Stmt::Echo { args, .. } => {
            for a in args {
                collect_expr(a, vars, next);
            }
        }
        Stmt::Expr(e) => collect_expr(e, vars, next),
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            collect_expr(cond, vars, next);
            collect_stmts(then_branch, vars, next);
            collect_stmts(else_branch, vars, next);
        }
        Stmt::While { cond, body, .. } => {
            collect_expr(cond, vars, next);
            collect_stmts(body, vars, next);
        }
        Stmt::Foreach { subject, key_var, value_var, body, .. } => {
            collect_expr(subject, vars, next);
            if let Some(k) = key_var {
                ensure_var(*k, vars, next);
            }
            ensure_var(*value_var, vars, next);
            collect_stmts(body, vars, next);
        }
        Stmt::Return { value, .. } => {
            if let Some(e) = value {
                collect_expr(e, vars, next);
            }
        }
        // Functions and classes are separate scopes; do not pull their
        // variables (or their methods' variables) into this frame.
        Stmt::Func(_) | Stmt::Class(_) => {}
    }
}

fn collect_expr(e: &Expr, vars: &mut HashMap<IdentId, Reg>, next: &mut Reg) {
    match e {
        Expr::Var(id, _) => ensure_var(*id, vars, next),
        Expr::Assign { target, value, .. } => {
            ensure_var(*target, vars, next);
            collect_expr(value, vars, next);
        }
        Expr::Unary { expr, .. } => collect_expr(expr, vars, next),
        Expr::Binary { lhs, rhs, .. } => {
            collect_expr(lhs, vars, next);
            collect_expr(rhs, vars, next);
        }
        Expr::Call { args, .. } => {
            for a in args {
                collect_expr(a, vars, next);
            }
        }
        Expr::CallDynamic { callee, args, .. } => {
            collect_expr(callee, vars, next);
            for a in args {
                collect_expr(a, vars, next);
            }
        }
        // A closure captures `use` variables from this scope by value, so they
        // must have registers here. Its body/params are a separate scope and are
        // pre-scanned when the closure is compiled — do not descend into them.
        Expr::Closure { uses, .. } => {
            for u in uses {
                ensure_var(*u, vars, next);
            }
        }
        Expr::Array { items, .. } => {
            for it in items {
                if let Some(k) = &it.key {
                    collect_expr(k, vars, next);
                }
                collect_expr(&it.value, vars, next);
            }
        }
        Expr::Index { base, index, .. } => {
            collect_expr(base, vars, next);
            if let Some(i) = index {
                collect_expr(i, vars, next);
            }
        }
        Expr::New { args, .. } => {
            for a in args {
                collect_expr(a, vars, next);
            }
        }
        Expr::PropGet { obj, .. } => collect_expr(obj, vars, next),
        Expr::PropSet { obj, value, .. } => {
            collect_expr(obj, vars, next);
            collect_expr(value, vars, next);
        }
        Expr::MethodCall { obj, args, .. } => {
            collect_expr(obj, vars, next);
            for a in args {
                collect_expr(a, vars, next);
            }
        }
        Expr::StaticCall { args, .. } => {
            for a in args {
                collect_expr(a, vars, next);
            }
        }
        Expr::InstanceOf { expr, .. } => collect_expr(expr, vars, next),
        Expr::IndexAssign { base, index, value, .. } => {
            collect_expr(base, vars, next);
            if let Some(i) = index {
                collect_expr(i, vars, next);
            }
            collect_expr(value, vars, next);
        }
        Expr::Null(_)
        | Expr::Bool(_, _)
        | Expr::Int(_, _)
        | Expr::Float(_, _)
        | Expr::Str(_, _) => {}
    }
}

fn ensure_var(id: IdentId, vars: &mut HashMap<IdentId, Reg>, next: &mut Reg) {
    vars.entry(id).or_insert_with(|| {
        let r = *next;
        *next += 1;
        r
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use rphp_span::Span;

    fn sp() -> Span {
        Span::dummy()
    }
    fn int(n: i64) -> Expr {
        Expr::Int(n, sp())
    }
    fn boolean(b: bool) -> Expr {
        Expr::Bool(b, sp())
    }
    fn var(id: IdentId) -> Expr {
        Expr::Var(id, sp())
    }
    fn bin(op: BinOp, l: Expr, r: Expr) -> Expr {
        Expr::Binary {
            op,
            lhs: Box::new(l),
            rhs: Box::new(r),
            span: sp(),
        }
    }
    fn echo(args: Vec<Expr>) -> Stmt {
        Stmt::Echo { args, span: sp() }
    }
    fn expr_stmt(e: Expr) -> Stmt {
        Stmt::Expr(e)
    }

    fn compile_ok(items: Vec<Stmt>, interner: &Interner) -> Module {
        compile(&Program { items }, interner).expect("expected successful compile")
    }

    fn compile_err(items: Vec<Stmt>, interner: &Interner) -> Vec<Diagnostic> {
        compile(&Program { items }, interner).expect_err("expected a diagnostic")
    }

    #[test]
    fn echo_add_emits_add_and_echo() {
        let interner = Interner::new();
        let m = compile_ok(vec![echo(vec![bin(BinOp::Add, int(1), int(2))])], &interner);

        assert_eq!(m.main, 0);
        assert_eq!(m.funcs.len(), 1);
        let main = m.func(0);
        assert_eq!(main.num_params, 0);
        assert_eq!(main.consts, vec![Const::Int(1), Const::Int(2)]);
        assert_eq!(
            main.code,
            vec![
                Op::LoadConst { dst: 0, k: 0 },
                Op::LoadConst { dst: 1, k: 1 },
                Op::Add { dst: 0, a: 0, b: 1 },
                Op::Echo { src: 0 },
                Op::Ret { src: None },
            ]
        );
        assert_eq!(main.num_regs, 2);
    }

    #[test]
    fn every_function_ends_with_ret() {
        let interner = Interner::new();
        let m = compile_ok(vec![echo(vec![int(7)])], &interner);
        assert!(matches!(
            m.func(0).code.last(),
            Some(Op::Ret { src: None })
        ));
    }

    #[test]
    fn assignment_then_use() {
        let mut interner = Interner::new();
        let x = interner.intern_str("x");
        // $x = 5; echo $x;
        let m = compile_ok(
            vec![
                expr_stmt(Expr::Assign {
                    target: x,
                    value: Box::new(int(5)),
                    span: sp(),
                }),
                echo(vec![var(x)]),
            ],
            &interner,
        );
        let main = m.func(0);
        // $x is register 0 (the only variable, no params).
        assert!(main
            .code
            .iter()
            .any(|op| matches!(op, Op::Echo { src: 0 })));
        // The constant 5 is loaded and the value reaches register 0.
        assert_eq!(main.consts, vec![Const::Int(5)]);
        assert!(main
            .code
            .iter()
            .any(|op| matches!(op, Op::Move { dst: 0, .. } | Op::LoadConst { dst: 0, .. })));
    }

    #[test]
    fn self_referential_assignment_reads_before_write() {
        let mut interner = Interner::new();
        let x = interner.intern_str("x");
        // $x = $x + 1;  — old $x must be read before the result lands in reg 0.
        let m = compile_ok(
            vec![expr_stmt(Expr::Assign {
                target: x,
                value: Box::new(bin(BinOp::Add, var(x), int(1))),
                span: sp(),
            })],
            &interner,
        );
        let main = m.func(0);
        // The Add reads reg 0 (the live $x) as an operand; the result is moved
        // back into reg 0 only afterwards.
        let add = main
            .code
            .iter()
            .find_map(|op| match op {
                Op::Add { dst, a, b } => Some((*dst, *a, *b)),
                _ => None,
            })
            .expect("expected an Add");
        assert!(add.1 == 0 || add.2 == 0, "Add should read $x (reg 0)");
        assert_ne!(add.0, 0, "Add result should go to a temp, not clobber $x");
    }

    #[test]
    fn call_resolves_to_func_id_and_arity() {
        let mut interner = Interner::new();
        let foo = interner.intern_str("foo");
        let a = interner.intern_str("a");
        // function foo($a) { return $a; }  foo(7);
        let func = Stmt::Func(Func {
            name: foo,
            params: vec![Param { name: a, span: sp() }],
            body: vec![Stmt::Return {
                value: Some(var(a)),
                span: sp(),
            }],
            span: sp(),
        });
        let call = expr_stmt(Expr::Call {
            name: foo,
            args: vec![int(7)],
            span: sp(),
        });
        let m = compile_ok(vec![func, call], &interner);

        assert_eq!(m.funcs.len(), 2);
        // foo is FuncId 1 with one param.
        let foo_fn = m.func(1);
        assert_eq!(foo_fn.num_params, 1);
        assert!(matches!(foo_fn.code.first(), Some(Op::Ret { src: Some(0) })));

        // main contains a Call to FuncId 1 with argc 1.
        let main = m.func(0);
        let callop = main
            .code
            .iter()
            .find_map(|op| match op {
                Op::Call { func, base, argc, .. } => Some((*func, *base, *argc)),
                _ => None,
            })
            .expect("expected a Call op");
        assert_eq!(callop.0, 1, "resolved FuncId");
        assert_eq!(callop.2, 1, "argc");
    }

    #[test]
    fn forward_reference_resolves() {
        let mut interner = Interner::new();
        let foo = interner.intern_str("foo");
        // foo();  function foo() {}   — call appears before the declaration.
        let call = expr_stmt(Expr::Call {
            name: foo,
            args: vec![],
            span: sp(),
        });
        let func = Stmt::Func(Func {
            name: foo,
            params: vec![],
            body: vec![],
            span: sp(),
        });
        let m = compile_ok(vec![call, func], &interner);
        let main = m.func(0);
        assert!(main
            .code
            .iter()
            .any(|op| matches!(op, Op::Call { func: 1, argc: 0, .. })));
    }

    #[test]
    fn undefined_function_diagnoses() {
        let mut interner = Interner::new();
        let bar = interner.intern_str("bar");
        let diags = compile_err(
            vec![expr_stmt(Expr::Call {
                name: bar,
                args: vec![int(1)],
                span: sp(),
            })],
            &interner,
        );
        assert!(diags
            .iter()
            .any(|d| d.code == codes::UNDEFINED_FUNCTION && d.is_error()));
    }

    #[test]
    fn native_call_lowers_to_call_native() {
        let mut interner = Interner::new();
        let strlen = interner.intern_str("strlen");
        let x = interner.intern_str("x");
        // strlen("x"); — a builtin, with no matching user function.
        let m = compile_ok(
            vec![expr_stmt(Expr::Call {
                name: strlen,
                args: vec![Expr::Str(x, sp())],
                span: sp(),
            })],
            &interner,
        );
        assert!(m
            .func(0)
            .code
            .iter()
            .any(|op| matches!(op, Op::CallNative { argc: 1, .. })));
        // The builtin is not lowered into a user `Function`.
        assert_eq!(m.funcs.len(), 1);
    }

    #[test]
    fn user_function_shadows_builtin_of_same_name() {
        let mut interner = Interner::new();
        let count = interner.intern_str("count");
        // function count() { return 1; } count(); — the user def wins, so the
        // call is a user `Call`, not a `CallNative`.
        let func = Stmt::Func(Func {
            name: count,
            params: vec![],
            body: vec![Stmt::Return { value: Some(int(1)), span: sp() }],
            span: sp(),
        });
        let call = expr_stmt(Expr::Call { name: count, args: vec![], span: sp() });
        let m = compile_ok(vec![func, call], &interner);
        let main = &m.func(0).code;
        assert!(main.iter().any(|op| matches!(op, Op::Call { func: 1, .. })));
        assert!(!main.iter().any(|op| matches!(op, Op::CallNative { .. })));
    }

    #[test]
    fn wrong_native_arity_diagnoses() {
        let mut interner = Interner::new();
        let strlen = interner.intern_str("strlen");
        let x = interner.intern_str("x");
        // strlen("x", "x") — strlen takes exactly one argument.
        let diags = compile_err(
            vec![expr_stmt(Expr::Call {
                name: strlen,
                args: vec![Expr::Str(x, sp()), Expr::Str(x, sp())],
                span: sp(),
            })],
            &interner,
        );
        assert!(diags
            .iter()
            .any(|d| d.code == codes::WRONG_ARG_COUNT && d.is_error()));
    }

    #[test]
    fn by_ref_builtin_writes_back_to_variable() {
        let mut interner = Interner::new();
        let sort = interner.intern_str("sort");
        let a = interner.intern_str("a");
        // $a = []; sort($a);  — sort takes its array by reference.
        let m = compile_ok(
            vec![
                expr_stmt(Expr::Assign {
                    target: a,
                    value: Box::new(Expr::Array { items: vec![], span: sp() }),
                    span: sp(),
                }),
                expr_stmt(Expr::Call { name: sort, args: vec![var(a)], span: sp() }),
            ],
            &interner,
        );
        let code = &m.func(0).code;
        let call = code.iter().position(|op| matches!(op, Op::CallNative { .. })).unwrap();
        // $a is register 0; after the call its register is written back.
        assert!(code[call + 1..]
            .iter()
            .any(|op| matches!(op, Op::Move { dst: 0, .. })));
    }

    #[test]
    fn by_ref_non_variable_diagnoses() {
        let mut interner = Interner::new();
        let sort = interner.intern_str("sort");
        // sort([1]) — a by-ref parameter requires a variable, not a literal.
        let diags = compile_err(
            vec![expr_stmt(Expr::Call {
                name: sort,
                args: vec![Expr::Array {
                    items: vec![rphp_ast::ArrayItem { key: None, value: int(1) }],
                    span: sp(),
                }],
                span: sp(),
            })],
            &interner,
        );
        assert!(diags.iter().any(|d| d.code == BY_REF_NOT_VARIABLE && d.is_error()));
    }

    #[test]
    fn wrong_arg_count_diagnoses() {
        let mut interner = Interner::new();
        let foo = interner.intern_str("foo");
        let a = interner.intern_str("a");
        let func = Stmt::Func(Func {
            name: foo,
            params: vec![Param { name: a, span: sp() }],
            body: vec![],
            span: sp(),
        });
        // foo(1, 2) — two args for a one-param function.
        let call = expr_stmt(Expr::Call {
            name: foo,
            args: vec![int(1), int(2)],
            span: sp(),
        });
        let diags = compile_err(vec![func, call], &interner);
        assert!(diags
            .iter()
            .any(|d| d.code == codes::WRONG_ARG_COUNT && d.is_error()));
    }

    #[test]
    fn duplicate_function_diagnoses() {
        let mut interner = Interner::new();
        let foo = interner.intern_str("foo");
        let dup = || {
            Stmt::Func(Func {
                name: foo,
                params: vec![],
                body: vec![],
                span: sp(),
            })
        };
        let diags = compile_err(vec![dup(), dup()], &interner);
        assert!(diags.iter().any(|d| d.code == REDECLARED_FUNCTION && d.is_error()));
    }

    #[test]
    fn logical_and_short_circuits_with_branches() {
        let interner = Interner::new();
        // echo true && false;
        let m = compile_ok(
            vec![echo(vec![bin(BinOp::And, boolean(true), boolean(false))])],
            &interner,
        );
        let code = &m.func(0).code;
        // A short-circuit branch and a join jump are present, plus the bool
        // normalization (two Nots), and no boolean opcode was invented.
        assert!(code.iter().any(|op| matches!(op, Op::JmpIfFalse { .. })));
        assert!(code.iter().any(|op| matches!(op, Op::Jmp { .. })));
        assert_eq!(
            code.iter().filter(|op| matches!(op, Op::Not { .. })).count(),
            2
        );
        assert_branch_targets_in_range(code);
    }

    #[test]
    fn logical_or_uses_jmp_if_true() {
        let interner = Interner::new();
        let m = compile_ok(
            vec![echo(vec![bin(BinOp::Or, boolean(false), boolean(true))])],
            &interner,
        );
        let code = &m.func(0).code;
        assert!(code.iter().any(|op| matches!(op, Op::JmpIfTrue { .. })));
        assert_branch_targets_in_range(code);
    }

    #[test]
    fn if_else_backpatches() {
        let interner = Interner::new();
        // if (1) { echo 1; } else { echo 2; }
        let m = compile_ok(
            vec![Stmt::If {
                cond: int(1),
                then_branch: vec![echo(vec![int(1)])],
                else_branch: vec![echo(vec![int(2)])],
                span: sp(),
            }],
            &interner,
        );
        let code = &m.func(0).code;
        assert!(code.iter().any(|op| matches!(op, Op::JmpIfFalse { .. })));
        assert!(code.iter().any(|op| matches!(op, Op::Jmp { .. })));
        assert_branch_targets_in_range(code);
    }

    #[test]
    fn while_jumps_backward() {
        let interner = Interner::new();
        // while (1) { echo 1; }
        let m = compile_ok(
            vec![Stmt::While {
                cond: int(1),
                body: vec![echo(vec![int(1)])],
                span: sp(),
            }],
            &interner,
        );
        let code = &m.func(0).code;
        // The back-edge jump targets an index at or before its own position.
        let back_jmp = code.iter().enumerate().find_map(|(i, op)| match op {
            Op::Jmp { target } if (*target as usize) <= i => Some(*target),
            _ => None,
        });
        assert!(back_jmp.is_some(), "expected a backward loop jump");
        assert!(code.iter().any(|op| matches!(op, Op::JmpIfFalse { .. })));
        assert_branch_targets_in_range(code);
    }

    #[test]
    fn unary_and_comparison_ops() {
        let interner = Interner::new();
        let m = compile_ok(
            vec![
                echo(vec![Expr::Unary {
                    op: UnOp::Neg,
                    expr: Box::new(int(1)),
                    span: sp(),
                }]),
                echo(vec![Expr::Unary {
                    op: UnOp::Not,
                    expr: Box::new(int(0)),
                    span: sp(),
                }]),
                echo(vec![bin(BinOp::Spaceship, int(1), int(2))]),
                echo(vec![bin(BinOp::Lt, int(1), int(2))]),
            ],
            &interner,
        );
        let code = &m.func(0).code;
        assert!(code.iter().any(|op| matches!(op, Op::Neg { .. })));
        assert!(code.iter().any(|op| matches!(op, Op::Not { .. })));
        assert!(code.iter().any(|op| matches!(op, Op::Spaceship { .. })));
        assert!(code.iter().any(|op| matches!(op, Op::CmpLt { .. })));
    }

    #[test]
    fn string_literal_and_concat() {
        let mut interner = Interner::new();
        let hi = interner.intern_str("hi");
        // echo "hi" . "!";
        let m = compile_ok(
            vec![echo(vec![Expr::Binary {
                op: BinOp::Concat,
                lhs: Box::new(Expr::Str(hi, sp())),
                rhs: Box::new(Expr::Str(interner.intern_str("!"), sp())),
                span: sp(),
            }])],
            &interner,
        );
        let main = m.func(0);
        // Both string literals reach the constant pool as Const::Str.
        assert!(main.consts.contains(&Const::Str(Str::new(b"hi"))));
        assert!(main.consts.contains(&Const::Str(Str::new(b"!"))));
        // A Concat op was emitted (not an Add).
        assert!(main.code.iter().any(|op| matches!(op, Op::Concat { .. })));
        assert!(main.code.iter().any(|op| matches!(op, Op::Echo { .. })));
    }

    #[test]
    fn array_literal_emits_new_and_fills() {
        let mut interner = Interner::new();
        let k = interner.intern_str("k");
        // [1, "k" => 2]
        let m = compile_ok(
            vec![expr_stmt(Expr::Array {
                items: vec![
                    rphp_ast::ArrayItem { key: None, value: int(1) },
                    rphp_ast::ArrayItem {
                        key: Some(Expr::Str(k, sp())),
                        value: int(2),
                    },
                ],
                span: sp(),
            })],
            &interner,
        );
        let code = &m.func(0).code;
        assert!(code.iter().any(|op| matches!(op, Op::NewArray { .. })));
        assert!(code.iter().any(|op| matches!(op, Op::ArrayPush { .. })));
        assert!(code.iter().any(|op| matches!(op, Op::ArraySet { .. })));
    }

    #[test]
    fn append_lowers_to_array_push() {
        let mut interner = Interner::new();
        let a = interner.intern_str("a");
        // $a[] = 5;
        let m = compile_ok(
            vec![expr_stmt(Expr::IndexAssign {
                base: Box::new(var(a)),
                index: None,
                value: Box::new(int(5)),
                span: sp(),
            })],
            &interner,
        );
        assert!(m.func(0).code.iter().any(|op| matches!(op, Op::ArrayPush { .. })));
    }

    #[test]
    fn foreach_lowers_to_foreach_next() {
        let mut interner = Interner::new();
        let a = interner.intern_str("a");
        let v = interner.intern_str("v");
        // foreach ($a as $v) { echo $v; }
        let m = compile_ok(
            vec![Stmt::Foreach {
                subject: var(a),
                key_var: None,
                value_var: v,
                body: vec![echo(vec![var(v)])],
                span: sp(),
            }],
            &interner,
        );
        let code = &m.func(0).code;
        assert!(code.iter().any(|op| matches!(op, Op::ForeachNext { .. })));
        // The exhaustion target must be a valid instruction index.
        let n = code.len() as CodeAddr;
        for op in code {
            if let Op::ForeachNext { target, .. } = op {
                assert!(*target <= n);
            }
        }
    }

    #[test]
    fn nested_array_write_diagnoses() {
        let mut interner = Interner::new();
        let a = interner.intern_str("a");
        // $a[0][1] = 5;  — nested lvalue write is not supported yet.
        let diags = compile_err(
            vec![expr_stmt(Expr::IndexAssign {
                base: Box::new(Expr::Index {
                    base: Box::new(var(a)),
                    index: Some(Box::new(int(0))),
                    span: sp(),
                }),
                index: Some(Box::new(int(1))),
                value: Box::new(int(5)),
                span: sp(),
            })],
            &interner,
        );
        assert!(diags.iter().any(|d| d.code == NESTED_ARRAY_WRITE && d.is_error()));
    }

    #[test]
    fn params_occupy_low_registers() {
        let mut interner = Interner::new();
        let add = interner.intern_str("add");
        let a = interner.intern_str("a");
        let b = interner.intern_str("b");
        // function add($a, $b) { return $a + $b; }
        let func = Stmt::Func(Func {
            name: add,
            params: vec![
                Param { name: a, span: sp() },
                Param { name: b, span: sp() },
            ],
            body: vec![Stmt::Return {
                value: Some(bin(BinOp::Add, var(a), var(b))),
                span: sp(),
            }],
            span: sp(),
        });
        let m = compile_ok(vec![func], &interner);
        let add_fn = m.func(1);
        assert_eq!(add_fn.num_params, 2);
        // $a -> reg 0, $b -> reg 1; their Add reads exactly those.
        let add_op = add_fn
            .code
            .iter()
            .find_map(|op| match op {
                Op::Add { a, b, .. } => Some((*a, *b)),
                _ => None,
            })
            .expect("expected Add");
        assert_eq!(add_op, (0, 1));
        assert!(add_fn.num_regs >= 2);
    }

    #[test]
    fn class_compiles_methods_and_binds_this_to_reg_zero() {
        let mut interner = Interner::new();
        let c = interner.intern_str("C");
        let get = interner.intern_str("get");
        let v = interner.intern_str("v");
        let this = interner.intern_str("this");
        let cvar = interner.intern_str("c");

        // class C { public $v = 1; function get() { return $this->v; } }
        let class = Stmt::Class(rphp_ast::Class {
            name: c,
            parent: None,
            props: vec![rphp_ast::PropDecl {
                name: v,
                default: Some(int(1)),
                visibility: AstVis::Public,
                span: sp(),
            }],
            methods: vec![rphp_ast::Method {
                name: get,
                params: vec![],
                body: vec![Stmt::Return {
                    value: Some(Expr::PropGet { obj: Box::new(var(this)), name: v, span: sp() }),
                    span: sp(),
                }],
                visibility: AstVis::Public,
                span: sp(),
            }],
            span: sp(),
        });
        // $c = new C(); $c->get();
        let new_c = expr_stmt(Expr::Assign {
            target: cvar,
            value: Box::new(Expr::New { class: c, args: vec![], span: sp() }),
            span: sp(),
        });
        let call = expr_stmt(Expr::MethodCall {
            obj: Box::new(var(cvar)),
            method: get,
            args: vec![],
            span: sp(),
        });
        let m = compile_ok(vec![class, new_c, call], &interner);

        // One class, with its property default folded to a constant value.
        assert_eq!(m.classes.len(), 1);
        assert_eq!(m.classes[0].props.len(), 1);
        assert_eq!(m.classes[0].props[0].default, rphp_value::Value::Int(1));
        assert_eq!(m.classes[0].methods.len(), 1);

        // main instantiates and dispatches.
        let main = &m.func(0).code;
        assert!(main.iter().any(|op| matches!(op, Op::New { class: 0, .. })));
        assert!(main.iter().any(|op| matches!(op, Op::MethodCall { .. })));

        // The method body reads `$this` (register 0) for the property access, and
        // the method frame reserves register 0 for `$this` (num_params == 1).
        let fid = m.classes[0].methods[0].func;
        let method = m.func(fid);
        assert_eq!(method.num_params, 1, "$this occupies the sole parameter slot");
        assert!(method.code.iter().any(|op| matches!(op, Op::PropGet { obj: 0, .. })));
    }

    #[test]
    fn undefined_class_diagnoses() {
        let mut interner = Interner::new();
        let nope = interner.intern_str("Nope");
        let diags = compile_err(
            vec![expr_stmt(Expr::New { class: nope, args: vec![], span: sp() })],
            &interner,
        );
        assert!(diags.iter().any(|d| d.code == UNDEFINED_CLASS && d.is_error()));
    }

    /// Every branch target must point at a valid instruction index.
    fn assert_branch_targets_in_range(code: &[Op]) {
        let n = code.len() as CodeAddr;
        for op in code {
            match op {
                Op::Jmp { target }
                | Op::JmpIfTrue { target, .. }
                | Op::JmpIfFalse { target, .. } => {
                    assert!(*target < n, "branch target {target} out of range (len {n})");
                }
                _ => {}
            }
        }
    }
}
