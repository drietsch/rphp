//! The frame-info stack: one record per active call (user function, native,
//! or an engine-internal marker) so warnings can say "on line N" and an
//! uncaught fault can print php's `Stack trace:` — before E3's explicit
//! frame stack exists. The interpreter pushes/pops around every call and
//! stores the current `pc` at each op.

use rphp_bytecode::{FuncId, Module};
use rphp_value::Value;

/// What a [`FrameInfo`] describes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FrameKind {
    /// A user function / method / closure / `{main}` frame.
    User { func: FuncId },
    /// A native function frame (`intdiv(…)`), by registered name.
    Native { name: &'static str },
    /// An engine-internal caller (shutdown functions, output handlers):
    /// renders as `[internal function]` for the frame it calls.
    Internal,
}

/// One entry of the frame-info stack.
#[derive(Clone, Debug)]
pub struct FrameInfo {
    pub kind: FrameKind,
    /// The op being executed in a user frame (index into `Function::code`).
    pub pc: usize,
    /// The arguments the frame was entered with (for `Stack trace:` lines).
    pub args: Vec<Value>,
}

impl FrameInfo {
    /// A user frame.
    pub fn user(func: FuncId, args: Vec<Value>) -> FrameInfo {
        FrameInfo {
            kind: FrameKind::User { func },
            pc: 0,
            args,
        }
    }

    /// A native frame.
    pub fn native(name: &'static str, args: Vec<Value>) -> FrameInfo {
        FrameInfo {
            kind: FrameKind::Native { name },
            pc: 0,
            args,
        }
    }

    /// An internal-caller marker.
    pub fn internal() -> FrameInfo {
        FrameInfo {
            kind: FrameKind::Internal,
            pc: 0,
            args: Vec::new(),
        }
    }
}

/// The source line of a user frame's current op (0 without line info).
pub fn frame_line(module: &Module, frame: &FrameInfo) -> u32 {
    match frame.kind {
        FrameKind::User { func } => module.func(func).line_at(frame.pc).unwrap_or(0),
        _ => 0,
    }
}

/// The name a frame prints in a trace: `f`, `A->m`, `{closure}`, `intdiv`.
pub fn frame_name(module: Option<&Module>, frame: &FrameInfo) -> String {
    match frame.kind {
        FrameKind::Native { name } => name.to_string(),
        FrameKind::Internal => "[internal]".to_string(),
        FrameKind::User { func } => {
            let Some(module) = module else {
                return "{main}".to_string();
            };
            if func == module.main {
                return "{main}".to_string();
            }
            let f = module.func(func);
            if f.name_bytes.is_empty() {
                return "{closure}".to_string();
            }
            let name = String::from_utf8_lossy(&f.name_bytes);
            match module.method_owner(func) {
                Some(cid) => format!(
                    "{}->{}",
                    String::from_utf8_lossy(&module.class(cid).name_bytes),
                    name
                ),
                None => name.into_owned(),
            }
        }
    }
}

/// One argument as php prints it in a trace line: `1`, `'abc'`, `Array`,
/// `Object(Foo)`, `NULL`, `true`, strings truncated to 15 bytes + `...`.
pub fn trace_arg(v: &Value) -> String {
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
        Value::Object(o) => format!(
            "Object({})",
            String::from_utf8_lossy(o.layout().class_name())
        ),
        Value::Resource(r) => format!("Resource id #{}", r.id()),
        Value::Ref(_) => unreachable!("deref'd above"),
    }
}

/// Render php's `Stack trace:` body for `frames` (bottom = `{main}` first):
/// `#0 file(line): callee(args)` per call, innermost first, then `#N {main}`.
/// A callee whose caller is a native or internal frame prints
/// `[internal function]` as its site.
pub fn render_trace(module: Option<&Module>, frames: &[FrameInfo], file: &str) -> String {
    let mut out = String::new();
    let mut n = 0;
    for i in (1..frames.len()).rev() {
        let callee = &frames[i];
        if callee.kind == FrameKind::Internal {
            continue;
        }
        let caller = &frames[i - 1];
        let site = match (&caller.kind, module) {
            (FrameKind::User { .. }, Some(m)) => format!("{file}({})", frame_line(m, caller)),
            _ => "[internal function]".to_string(),
        };
        let args: Vec<String> = callee.args.iter().map(trace_arg).collect();
        out.push_str(&format!(
            "#{n} {site}: {}({})\n",
            frame_name(module, callee),
            args.join(", ")
        ));
        n += 1;
    }
    out.push_str(&format!("#{n} {{main}}"));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trace_args_render_like_php() {
        assert_eq!(trace_arg(&Value::Int(1)), "1");
        assert_eq!(trace_arg(&Value::string(b"abc")), "'abc'");
        assert_eq!(
            trace_arg(&Value::string(b"a long string over 15 chars")),
            "'a long string o...'"
        );
        assert_eq!(trace_arg(&Value::empty_array()), "Array");
        assert_eq!(trace_arg(&Value::Null), "NULL");
        assert_eq!(trace_arg(&Value::Bool(true)), "true");
        assert_eq!(trace_arg(&Value::Float(1.5)), "1.5");
    }

    #[test]
    fn main_only_trace() {
        let frames = vec![FrameInfo::user(0, vec![])];
        assert_eq!(render_trace(None, &frames, "f.php"), "#0 {main}");
        assert_eq!(render_trace(None, &[], "f.php"), "#0 {main}");
    }

    #[test]
    fn native_caller_is_internal_function() {
        let frames = vec![
            FrameInfo::user(0, vec![]),
            FrameInfo::native("array_map", vec![Value::string(b"f"), Value::empty_array()]),
            FrameInfo::user(1, vec![Value::Int(1)]),
        ];
        // Without a module the user frames have no names/lines, but the shape
        // (internal caller, arg rendering, numbering) is what matters here.
        let t = render_trace(None, &frames, "f.php");
        assert_eq!(t, "#0 [internal function]: {main}(1)\n#1 [internal function]: array_map('f', Array)\n#2 {main}");
    }
}
