//! The explicit frame stack (ADR-016/018): one [`Frame`] per active call,
//! the pending-call records the `Init*`/`Send*`/`DoCall` sequence builds on
//! it, and php's `Stack trace:` rendering over it.
//!
//! User frames own a window of the interpreter's contiguous register stack
//! (`Interp::stack[base .. base + num_regs]`); native frames and internal
//! markers own no registers and exist for line numbers and traces.

use std::rc::Rc;

use rphp_bytecode::IncludeKind;
use rphp_value::{Array, ArrayKey, Closure, Object, PhpRef, Value};

use crate::class::MethodDef;
use crate::registry::NativeId;
use crate::symtab::Symtab;
use crate::unit::FuncRt;
use crate::Interp;

/// What kind of activation a frame is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FrameKind {
    /// An ordinary user function / method / closure frame.
    Normal,
    /// A user frame entered from a native (`call_value`): when it returns
    /// or unwinds, `run_until` stops and hands the result to the native.
    ReentryBoundary,
    /// A unit's `{main}` run by `include`/`require`, sharing the includer's
    /// symbol table, `$this` and scope.
    Include,
    /// A native function frame (`intdiv(…)`), for traces.
    Native,
    /// An engine-internal caller (shutdown functions, output handlers):
    /// renders as `[internal function]` for the frame it calls.
    Internal,
    /// A generator body, resumed from `Generator::current/next/send/throw`.
    /// The frame is *parked* between resumptions: its registers live in the
    /// generator's state, not on the register stack (E8, `generator.rs`).
    Generator,
}

/// Where a returning frame delivers its value.
#[derive(Clone)]
pub enum RetTarget {
    /// Nowhere (a shutdown function, a thunk whose value is read elsewhere).
    Discard,
    /// Into this absolute register-stack slot of the caller.
    Reg(usize),
    /// `new`: the caller's slot receives the constructed object, not the
    /// constructor's return value.
    New { reg: usize, obj: Object },
}

/// The target of a pending call.
#[derive(Clone)]
pub enum CallTarget {
    /// A user function (with, for a closure, the closure value to bind).
    User {
        func: Rc<FuncRt>,
        closure: Option<Closure>,
    },
    /// A registered native.
    Native(NativeId),
    /// A native method (`$this` is the pending call's `this`).
    NativeMethod(Rc<MethodDef>),
    /// `new` of a class without a constructor: the arguments are evaluated
    /// and dropped.
    NoCtor,
}

/// What a native frame is running.
#[derive(Clone)]
pub enum NativeTarget {
    /// A native function.
    Func(NativeId),
    /// A native method.
    Method(Rc<MethodDef>),
}

/// A call being set up by `Init*` / `Send*` ops, consumed by `DoCall`.
pub struct PendingCall {
    pub target: CallTarget,
    /// `$this` for the callee.
    pub this: Option<Object>,
    /// The callee's lexical scope class.
    pub scope: Option<u32>,
    /// The callee's late-static-bound class.
    pub static_class: Option<u32>,
    /// Absolute register-stack index of argument 0.
    pub args_base: usize,
    /// Positional arguments sent so far.
    pub argc: usize,
    /// Named arguments (after the positional ones).
    pub named: Vec<(Box<[u8]>, Value)>,
    /// The object being constructed (`InitNew`).
    pub new_obj: Option<Object>,
}

/// The state of a `foreach` iterator register.
pub enum IterState {
    /// A by-value loop over an array snapshot.
    Array { arr: rphp_value::Array, pos: usize },
    /// A by-reference loop over the array behind a reference cell.
    ByRef { cell: PhpRef, pos: usize },
    /// A native `Iterator` stepped through its class's handlers directly
    /// (`ClassDef::native_iter`); `pos` counts the steps taken. `by_ref`
    /// binds the loop variable to the element cell the class's `dim_ref`
    /// hook hands out (`foreach ($arrayIterator as &$v)`).
    Native { obj: rphp_value::Object, iter: crate::class::NativeIter, pos: usize, by_ref: bool },
    /// Nothing to iterate (non-iterable subject: the loop is skipped).
    Empty,
}

/// One activation on the frame stack. The fields most calls never touch
/// live behind [`Frame::extra`], allocated on first use, so that pushing
/// and popping a frame moves and drops little.
pub struct Frame {
    pub kind: FrameKind,
    /// The running function (`None` for native / internal frames).
    pub func: Option<Rc<FuncRt>>,
    /// Absolute index of register 0 in `Interp::stack`.
    pub base: usize,
    /// The op being executed (saved on every frame switch and fault).
    pub pc: usize,
    /// Positional arguments passed.
    pub argc: usize,
    /// `$this`.
    pub this: Option<Object>,
    /// Lexical scope class (visibility checks, `self::`).
    pub scope: Option<u32>,
    /// Late-static-bound class (`static::`).
    pub static_class: Option<u32>,
    /// Where the return value goes.
    pub ret: RetTarget,
    /// Calls being set up in this frame (innermost last).
    pub pending: Vec<PendingCall>,
    /// The `@` depth on entry (restored when the frame unwinds).
    pub silence_base: u32,
    /// `declare(strict_types=1)` unit.
    pub strict: bool,
    /// For native frames: the native and the arguments it was called with.
    pub native: Option<(NativeTarget, Vec<Value>)>,
    /// What few frames carry (see [`FrameExtra`]).
    pub extra: Option<Box<FrameExtra>>,
}

/// The parts of a [`Frame`] that an ordinary call never needs: variadic
/// leftovers, a symbol table, `foreach` iterators, generator and include
/// bookkeeping, a native's by-reference cells, a closure's statics.
#[derive(Default)]
pub struct FrameExtra {
    /// Positional arguments beyond the declared parameters
    /// (`func_get_args`, `...$rest`).
    pub extra_args: Vec<Value>,
    /// Named arguments beyond the declared parameters (`...$rest`).
    pub extra_named: Vec<(Box<[u8]>, Value)>,
    /// The named symbol table (`NEEDS_SYMTAB` frames, includes, `{main}`).
    pub symtab: Option<Symtab>,
    /// The cells behind a native's by-reference arguments (position,
    /// cell): what `bindParam()`-style natives keep hold of.
    pub ref_cells: Vec<(usize, PhpRef)>,
    /// For `Include` frames: which keyword.
    pub include_kind: Option<IncludeKind>,
    /// Live `foreach` iterators by iterator register.
    pub iters: Vec<(u16, IterState)>,
    /// For `Generator` frames: the generator this body belongs to, as an
    /// index into `Interp::generators`.
    pub generator: Option<u32>,
    /// A closure frame's own `static` table. php gives each closure *object*
    /// its own, so it cannot come from the compiled function; `None` for an
    /// ordinary function, which uses `FuncRt::statics`.
    pub statics: Option<std::rc::Rc<std::cell::RefCell<Vec<Option<rphp_value::PhpRef>>>>>,
}

thread_local! {
    /// What [`Frame::extra`] answers for a frame without extras (the
    /// contents hold `Rc`s, so this cannot be a plain static).
    static NO_EXTRA: &'static FrameExtra = Box::leak(Box::default());
}

impl Frame {
    /// The rarely used parts, empty when the frame never needed them.
    #[inline]
    pub fn extra(&self) -> &FrameExtra {
        match &self.extra {
            Some(e) => e,
            None => NO_EXTRA.with(|e| *e),
        }
    }

    /// The rarely used parts for writing, allocated on first use.
    #[inline]
    pub fn extra_mut(&mut self) -> &mut FrameExtra {
        self.extra.get_or_insert_with(Default::default)
    }
}

impl Frame {
    /// A native frame.
    pub fn native(id: NativeId, args: Vec<Value>, silence_base: u32) -> Frame {
        Frame {
            kind: FrameKind::Native,
            func: None,
            base: 0,
            pc: 0,
            argc: args.len(),
            this: None,
            scope: None,
            static_class: None,
            ret: RetTarget::Discard,
            pending: Vec::new(),
            silence_base,
            strict: false,
            native: Some((NativeTarget::Func(id), args)),
            extra: None,
        }
    }

    /// A native method frame.
    pub fn native_method(m: Rc<MethodDef>, this: Option<Object>, args: Vec<Value>, silence_base: u32) -> Frame {
        let mut f = Frame::native(NativeId(0), args, silence_base);
        f.native = f.native.map(|(_, args)| (NativeTarget::Method(m), args));
        f.this = this;
        f
    }

    /// An internal-caller marker.
    pub fn internal(silence_base: u32) -> Frame {
        Frame {
            kind: FrameKind::Internal,
            ..Frame::native(NativeId(0), Vec::new(), silence_base)
        }
    }

    /// Whether this frame runs bytecode.
    pub fn is_user(&self) -> bool {
        self.func.is_some()
    }
}

/// One argument as php prints it in a trace line: `1`, `'abc'`, `Array`,
/// `Object(Foo)`, `NULL`, `true`, strings truncated to 15 bytes + `...`.
pub fn trace_arg(interp: &Interp, v: &Value) -> String {
    match &*v.deref() {
        Value::Null | Value::Uninit => "NULL".to_string(),
        Value::Bool(true) => "true".to_string(),
        Value::Bool(false) => "false".to_string(),
        Value::Int(i) => i.to_string(),
        Value::Float(_) => interp.float_to_string_precision(v),
        Value::Str(s) => {
            let b = s.as_bytes();
            if b.len() > 15 {
                format!("'{}...'", escape_trace_bytes(&b[..15]))
            } else {
                format!("'{}'", escape_trace_bytes(b))
            }
        }
        Value::Array(_) => "Array".to_string(),
        Value::Closure(_) => "Object(Closure)".to_string(),
        Value::Object(o) => format!("Object({})", interp.class_name_of(o)),
        Value::Resource(r) => format!("Resource id #{}", r.id()),
        Value::Ref(_) => unreachable!("deref'd above"),
    }
}

/// php's `smart_str_append_escaped`: the C escapes for `\n`, `\r`,
/// `\t`, `\f`, `\v`, `\\`, `\e`, and `\xHH` for every other byte
/// outside printable ASCII (quotes are left alone).
fn escape_trace_bytes(b: &[u8]) -> String {
    let mut out = String::with_capacity(b.len());
    for &c in b {
        match c {
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b'\t' => out.push_str("\\t"),
            0x0c => out.push_str("\\f"),
            0x0b => out.push_str("\\v"),
            b'\\' => out.push_str("\\\\"),
            0x1b => out.push_str("\\e"),
            0x20..=0x7e => out.push(c as char),
            _ => out.push_str(&format!("\\x{c:02X}")),
        }
    }
    out
}

impl Interp {
    /// The frame stack (bottom first).
    pub fn frames(&self) -> &[Frame] {
        &self.frames
    }

    /// The innermost frame that runs bytecode.
    pub fn current_user_frame(&self) -> Option<&Frame> {
        self.frames.iter().rev().find(|f| f.is_user())
    }

    /// The innermost user frame's symbol table, if it has one.
    pub fn current_symtab(&self) -> Option<Symtab> {
        self.current_user_frame().and_then(|f| f.extra().symtab.clone())
    }

    /// The arguments a user frame was called with, as php reports them in
    /// traces and `func_get_args()`: the current values of the passed
    /// parameters, then the extra arguments.
    pub fn frame_args(&self, frame: &Frame) -> Vec<Value> {
        let Some(func) = &frame.func else {
            return frame
                .native
                .as_ref()
                .map(|(_, a)| a.clone())
                .unwrap_or_default();
        };
        // The variadic parameter holds the extras, which are listed from
        // `extra_args` instead.
        let np = func.f.num_params as usize
            - usize::from(func.f.flags.contains(rphp_bytecode::FnFlags::VARIADIC));
        let n = frame.argc.min(np);
        let mut out: Vec<Value> = (0..n)
            .map(|i| {
                self.stack
                    .get(frame.base + i)
                    .map(|v| v.deref().into_owned())
                    .unwrap_or(Value::Null)
            })
            .collect();
        out.extend(frame.extra().extra_args.iter().cloned());
        out
    }

    /// The name a frame prints in a trace: `f`, `A->m`, `A::m`,
    /// `{closure:…}`, `intdiv`, `include`.
    pub fn frame_name(&self, frame: &Frame) -> String {
        match self.frame_parts(frame) {
            (Some(class), Some(sep), name) => format!("{class}{sep}{name}"),
            (_, _, name) => name,
        }
    }

    /// php's `get_active_function_or_method_name()`: the running native as
    /// `func` or `Class::method` (what ext/intl prefixes its messages with).
    pub fn active_function_name(&self) -> String {
        let Some(frame) = self.frames.last() else {
            return String::new();
        };
        match self.frame_parts(frame) {
            (Some(class), _, name) => format!("{class}::{name}"),
            (None, _, name) => name,
        }
    }

    /// The `class`, `type` (`->`/`::`) and `function` a frame reports in a
    /// backtrace entry.
    pub fn frame_parts(&self, frame: &Frame) -> (Option<String>, Option<&'static str>, String) {
        match frame.kind {
            FrameKind::Native => match &frame.native {
                Some((NativeTarget::Func(id), _)) => {
                    (None, None, self.natives[id.0 as usize].name.to_string())
                }
                Some((NativeTarget::Method(m), _)) => {
                    let class = self.classes[m.decl as usize].name_str();
                    let sep = if frame.this.is_some() { "->" } else { "::" };
                    (Some(class), Some(sep), String::from_utf8_lossy(&m.name).into_owned())
                }
                None => (None, None, "[internal]".to_string()),
            },
            FrameKind::Internal => (None, None, "[internal]".to_string()),
            // A generator body shows under the function that declared it,
            // class and all.
            FrameKind::Generator => {
                let Some(func) = &frame.func else {
                    return (None, None, "{generator}".to_string());
                };
                let name = String::from_utf8_lossy(&func.f.name_bytes).into_owned();
                let class = if func.f.flags.contains(rphp_bytecode::FnFlags::CLOSURE) {
                    frame.scope
                } else {
                    frame.scope.or(func.class)
                };
                match class {
                    Some(cid) => {
                        let sep = if frame.this.is_some() { "->" } else { "::" };
                        (Some(self.classes[cid as usize].name_str()), Some(sep), name)
                    }
                    None => (None, None, name),
                }
            }
            FrameKind::Include => (
                None,
                None,
                match frame.extra().include_kind {
                    Some(IncludeKind::Include) => "include".to_string(),
                    Some(IncludeKind::IncludeOnce) => "include_once".to_string(),
                    Some(IncludeKind::Require) => "require".to_string(),
                    Some(IncludeKind::RequireOnce) | None => "require_once".to_string(),
                },
            ),
            FrameKind::Normal | FrameKind::ReentryBoundary => {
                let Some(func) = &frame.func else {
                    return (None, None, "{main}".to_string());
                };
                if func.is_main() {
                    return (None, None, "{main}".to_string());
                }
                let name = String::from_utf8_lossy(&func.f.name_bytes).into_owned();
                // A closure reports the class it is scoped to (bound or
                // declared in), `->` with a `$this`, `::` without one; a
                // method reports the class it runs in — the *using* class
                // for a trait's method, as php copies it there.
                let class = if func.f.flags.contains(rphp_bytecode::FnFlags::CLOSURE) {
                    frame.scope
                } else {
                    frame.scope.or(func.class)
                };
                match class {
                    Some(cid) => {
                        let sep = if frame.this.is_some() { "->" } else { "::" };
                        (Some(self.classes[cid as usize].name_str()), Some(sep), name)
                    }
                    None => (None, None, name),
                }
            }
        }
    }

    /// The file a frame's code lives in.
    pub fn frame_file(&self, frame: &Frame) -> String {
        match &frame.func {
            Some(f) => f.unit.file.to_string(),
            None => self.script_name.clone(),
        }
    }

    /// The source line a user frame is at (0 without line info).
    pub fn frame_line(&self, frame: &Frame) -> u32 {
        frame.func.as_ref().map_or(0, |f| f.line_at(frame.pc))
    }

    /// php's backtrace over the current frames as `debug_backtrace()` /
    /// `Exception::getTrace()` build it: one entry per call, innermost
    /// first — `['file', 'line', 'function', 'class', 'object', 'type',
    /// 'args']` with `file`/`line` being the *call site* in the caller (absent
    /// when the caller is a native or internal frame: `[internal function]`).
    /// `skip` frames are dropped from the top (a native that builds its own
    /// trace skips itself), `limit` caps the entries (0 = all).
    pub fn build_trace(&self, opts: TraceOpts) -> Array {
        // The stack as php shows it: a generator body serving a `yield
        // from` chain sits above the (parked) bodies delegating to it,
        // outermost first, each "called" at the other's `yield from`.
        let mut frames: Vec<&Frame> = Vec::with_capacity(self.frames.len());
        for f in &self.frames {
            if let Some(g) = f.extra.as_ref().and_then(|e| e.generator) {
                let mut chain: Vec<&Frame> = Vec::new();
                let mut cur = g;
                while let Some(p) = self.generators.get(cur as usize).and_then(|s| s.delegated_by()) {
                    match self.generators[p as usize].parked_frame() {
                        Some(pf) => chain.push(pf),
                        None => break,
                    }
                    cur = p;
                }
                frames.extend(chain.into_iter().rev());
            }
            frames.push(f);
        }
        let frames = &frames;
        let mut out = Array::new();
        let mut skipped = 0usize;
        let mut n = 0usize;
        for i in (1..frames.len()).rev() {
            let callee = frames[i];
            if callee.kind == FrameKind::Internal {
                continue;
            }
            if callee.func.as_ref().is_some_and(|f| f.is_main()) && callee.kind != FrameKind::Include {
                // A nested entry `{main}` (shutdown functions after the main
                // frame is gone) is the final `{main}` line only.
                continue;
            }
            if skipped < opts.skip {
                skipped += 1;
                continue;
            }
            if opts.limit != 0 && n >= opts.limit {
                break;
            }
            let caller = frames[i - 1];
            let mut entry = Array::new();
            if caller.is_user() {
                entry.set(ArrayKey::str(b"file"), Value::string(self.frame_file(caller).as_bytes()));
                entry.set(ArrayKey::str(b"line"), Value::Int(i64::from(self.frame_line(caller))));
            }
            let (class, sep, name) = self.frame_parts(callee);
            entry.set(ArrayKey::str(b"function"), Value::string(name.as_bytes()));
            if let Some(class) = class {
                entry.set(ArrayKey::str(b"class"), Value::string(class.as_bytes()));
                if opts.provide_object {
                    if let Some(this) = &callee.this {
                        entry.set(ArrayKey::str(b"object"), Value::Object(this.clone()));
                    }
                }
                if let Some(sep) = sep {
                    entry.set(ArrayKey::str(b"type"), Value::string(sep.as_bytes()));
                }
            }
            if !opts.ignore_args {
                let mut args = Array::new();
                match callee.kind {
                    FrameKind::Include => {
                        if let Some(f) = &callee.func {
                            args.push(Value::string(f.unit.file.as_bytes()));
                        }
                    }
                    _ => {
                        for a in self.frame_args(callee) {
                            args.push(a);
                        }
                        // Unknown named arguments a variadic collected keep
                        // their names (`f(1, k: 3)` in the rendering).
                        for (k, v) in &callee.extra().extra_named {
                            args.set(ArrayKey::str(k), v.clone());
                        }
                    }
                }
                entry.set(ArrayKey::str(b"args"), Value::Array(args));
            }
            out.push(Value::Array(entry));
            n += 1;
        }
        out
    }

    /// Render a backtrace array as php's `Stack trace:` body /
    /// `getTraceAsString()`: `#0 file(line): callee(args)` per entry, then
    /// `#N {main}`.
    /// The same rendering without the closing `#N {main}` line, which
    /// `debug_print_backtrace()` does not print (an exception's
    /// `getTraceAsString()` does).
    pub fn trace_to_string_bare(&self, trace: &Array) -> String {
        let full = self.trace_to_string(trace);
        match full.rfind("\n#") {
            Some(i) => full[..=i].to_string(),
            // A single entry: everything before the `#0 {main}` line.
            None => String::new(),
        }
    }

    pub fn trace_to_string(&self, trace: &Array) -> String {
        let mut out = String::new();
        let mut n = 0;
        for (_, entry) in trace.iter() {
            let Value::Array(e) = &*entry.deref() else { continue };
            let get = |k: &[u8]| e.get_deref(&ArrayKey::str(k));
            let site = match (get(b"file"), get(b"line")) {
                (Some(f), Some(l)) => format!("{}({})", f.to_php_string(), l.to_int()),
                _ => "[internal function]".to_string(),
            };
            let mut name = String::new();
            if let Some(c) = get(b"class") {
                name.push_str(&c.to_php_string());
                if let Some(t) = get(b"type") {
                    name.push_str(&t.to_php_string());
                }
            }
            if let Some(f) = get(b"function") {
                name.push_str(&f.to_php_string());
            }
            let args: Vec<String> = match get(b"args") {
                Some(Value::Array(a)) => a
                    .iter()
                    .map(|(k, v)| match k {
                        ArrayKey::Str(k) => format!("{}: {}", String::from_utf8_lossy(k), trace_arg(self, v)),
                        ArrayKey::Int(_) => trace_arg(self, v),
                    })
                    .collect(),
                _ => Vec::new(),
            };
            out.push_str(&format!("#{n} {site}: {name}({})\n", args.join(", ")));
            n += 1;
        }
        out.push_str(&format!("#{n} {{main}}"));
        out
    }

    /// Render php's `Stack trace:` body for the current frames.
    pub fn render_trace(&self) -> String {
        self.trace_to_string(&self.build_trace(TraceOpts::default()))
    }

    /// A float as php prints it with the `precision` ini setting (the form
    /// backtrace arguments use).
    pub fn float_to_string_precision(&self, v: &Value) -> String {
        let precision = self.ini_get("precision").and_then(|p| p.parse::<i64>().ok()).unwrap_or(14);
        match &*v.deref() {
            Value::Float(f) => format_float_precision(*f, precision),
            other => other.to_php_string(),
        }
    }
}

/// Options for [`Interp::build_trace`].
#[derive(Clone, Copy, Debug, Default)]
pub struct TraceOpts {
    /// Include the `object` entry for method frames (`DEBUG_BACKTRACE_PROVIDE_OBJECT`).
    pub provide_object: bool,
    /// Leave out `args` (`DEBUG_BACKTRACE_IGNORE_ARGS`).
    pub ignore_args: bool,
    /// Maximum number of entries (0 = unlimited).
    pub limit: usize,
    /// Frames to skip from the top.
    pub skip: usize,
}

/// php's `%.*G`-style float rendering with `precision` significant digits
/// (`smart_str_append_double` without the `.0` suffix).
pub fn format_float_precision(f: f64, precision: i64) -> String {
    if f.is_nan() {
        return "NAN".to_string();
    }
    if f.is_infinite() {
        return if f > 0.0 { "INF".to_string() } else { "-INF".to_string() };
    }
    let p = precision.clamp(1, 40) as usize;
    let s = format!("{:.*e}", p - 1, f);
    // Split mantissa / exponent and re-render like %G.
    let (mant, exp) = s.split_once('e').unwrap_or((&s, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    if exp < -4 || exp >= p as i32 {
        let mant = mant.trim_end_matches('0').trim_end_matches('.');
        let mant = if mant.contains('.') { mant.to_string() } else { format!("{mant}.0") };
        format!("{mant}E{}{}", if exp < 0 { "-" } else { "+" }, exp.abs())
    } else {
        let decimals = (p as i32 - 1 - exp).max(0) as usize;
        let s = format!("{:.*}", decimals, f);
        if s.contains('.') {
            s.trim_end_matches('0').trim_end_matches('.').to_string()
        } else {
            s
        }
    }
}