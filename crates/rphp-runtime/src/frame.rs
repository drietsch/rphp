//! The explicit frame stack (ADR-016/018): one [`Frame`] per active call,
//! the pending-call records the `Init*`/`Send*`/`DoCall` sequence builds on
//! it, and php's `Stack trace:` rendering over it.
//!
//! User frames own a window of the interpreter's contiguous register stack
//! (`Interp::stack[base .. base + num_regs]`); native frames and internal
//! markers own no registers and exist for line numbers and traces.

use std::rc::Rc;

use rphp_bytecode::IncludeKind;
use rphp_value::{Closure, Object, PhpRef, Value};

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
    /// `new` of a class without a constructor: the arguments are evaluated
    /// and dropped.
    NoCtor,
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
    /// The name the call was made under (for messages).
    pub name: Box<[u8]>,
}

/// The state of a `foreach` iterator register.
pub enum IterState {
    /// A by-value loop over an array snapshot.
    Array { arr: rphp_value::Array, pos: usize },
    /// A by-reference loop over the array behind a reference cell.
    ByRef { cell: PhpRef, pos: usize },
    /// Nothing to iterate (non-iterable subject: the loop is skipped).
    Empty,
}

/// One activation on the frame stack.
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
    /// Positional arguments beyond the declared parameters
    /// (`func_get_args`, `...$rest`).
    pub extra_args: Vec<Value>,
    /// Named arguments beyond the declared parameters (`...$rest`).
    pub extra_named: Vec<(Box<[u8]>, Value)>,
    /// `$this`.
    pub this: Option<Object>,
    /// Lexical scope class (visibility checks, `self::`).
    pub scope: Option<u32>,
    /// Late-static-bound class (`static::`).
    pub static_class: Option<u32>,
    /// Where the return value goes.
    pub ret: RetTarget,
    /// The named symbol table (`NEEDS_SYMTAB` frames, includes, `{main}`).
    pub symtab: Option<Symtab>,
    /// Calls being set up in this frame (innermost last).
    pub pending: Vec<PendingCall>,
    /// The `@` depth on entry (restored when the frame unwinds).
    pub silence_base: u32,
    /// `declare(strict_types=1)` unit.
    pub strict: bool,
    /// For native frames: the native and the arguments it was called with.
    pub native: Option<(NativeId, Vec<Value>)>,
    /// For `Include` frames: which keyword.
    pub include_kind: Option<IncludeKind>,
    /// Live `foreach` iterators by iterator register.
    pub iters: Vec<(u16, IterState)>,
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
            extra_args: Vec::new(),
            extra_named: Vec::new(),
            this: None,
            scope: None,
            static_class: None,
            ret: RetTarget::Discard,
            symtab: None,
            pending: Vec::new(),
            silence_base,
            strict: false,
            native: Some((id, args)),
            include_kind: None,
            iters: Vec::new(),
        }
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
        Value::Float(_) => v.to_php_string(),
        Value::Str(s) => {
            let b = s.as_bytes();
            if b.len() > 15 {
                format!("'{}...'", String::from_utf8_lossy(&b[..15]))
            } else {
                format!("'{}'", String::from_utf8_lossy(b))
            }
        }
        Value::Array(_) => "Array".to_string(),
        Value::Closure(_) => "Object(Closure)".to_string(),
        Value::Object(o) => format!("Object({})", interp.class_name_of(o)),
        Value::Resource(r) => format!("Resource id #{}", r.id()),
        Value::Ref(_) => unreachable!("deref'd above"),
    }
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
        self.current_user_frame().and_then(|f| f.symtab.clone())
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
        out.extend(frame.extra_args.iter().cloned());
        out
    }

    /// The name a frame prints in a trace: `f`, `A->m`, `A::m`,
    /// `{closure:…}`, `intdiv`, `include`.
    pub fn frame_name(&self, frame: &Frame) -> String {
        match frame.kind {
            FrameKind::Native => match &frame.native {
                Some((id, _)) => self.natives[id.0 as usize].name.to_string(),
                None => "[internal]".to_string(),
            },
            FrameKind::Internal => "[internal]".to_string(),
            FrameKind::Include => match frame.include_kind {
                Some(IncludeKind::Include) => "include".to_string(),
                Some(IncludeKind::IncludeOnce) => "include_once".to_string(),
                Some(IncludeKind::Require) => "require".to_string(),
                Some(IncludeKind::RequireOnce) | None => "require_once".to_string(),
            },
            FrameKind::Normal | FrameKind::ReentryBoundary => {
                let Some(func) = &frame.func else {
                    return "{main}".to_string();
                };
                if func.is_main() {
                    return "{main}".to_string();
                }
                let name = String::from_utf8_lossy(&func.f.name_bytes);
                match func.class {
                    Some(cid) if !func.f.flags.contains(rphp_bytecode::FnFlags::CLOSURE) => {
                        let sep = if frame.this.is_some() { "->" } else { "::" };
                        format!(
                            "{}{sep}{}",
                            String::from_utf8_lossy(&self.classes[cid as usize].name),
                            name
                        )
                    }
                    _ => name.into_owned(),
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

    /// Render php's `Stack trace:` body for the current frames (bottom =
    /// `{main}` first): `#0 file(line): callee(args)` per call, innermost
    /// first, then `#N {main}`. A callee whose caller is a native or internal
    /// frame prints `[internal function]` as its site.
    pub fn render_trace(&self) -> String {
        let frames = &self.frames;
        let mut out = String::new();
        let mut n = 0;
        for i in (1..frames.len()).rev() {
            let callee = &frames[i];
            if callee.kind == FrameKind::Internal {
                continue;
            }
            if callee.func.as_ref().is_some_and(|f| f.is_main())
                && callee.kind != FrameKind::Include
            {
                // A nested entry `{main}` (shutdown functions after the main
                // frame is gone) prints only as the final `{main}` line.
                continue;
            }
            let caller = &frames[i - 1];
            let site = if caller.is_user() {
                format!("{}({})", self.frame_file(caller), self.frame_line(caller))
            } else {
                "[internal function]".to_string()
            };
            let args: Vec<String> = match callee.kind {
                FrameKind::Include => Vec::new(),
                _ => self
                    .frame_args(callee)
                    .iter()
                    .map(|v| trace_arg(self, v))
                    .collect(),
            };
            out.push_str(&format!(
                "#{n} {site}: {}({})\n",
                self.frame_name(callee),
                args.join(", ")
            ));
            n += 1;
        }
        out.push_str(&format!("#{n} {{main}}"));
        out
    }
}
