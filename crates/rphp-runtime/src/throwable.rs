//! The engine side of `Throwable` (plan E5, ADR-022): filling `file`/`line`/
//! `trace` into a new exception object, materializing engine faults
//! ([`PendingThrow`]) as instances of their class, php's
//! `getTraceAsString()` / `__toString()` renderings, the object
//! stringification hook (`__toString` dispatch) and the uncaught rendering.
//!
//! The exception *classes* themselves (`Throwable`, `Exception`, `Error`,
//! the SPL tree) are registered by the stdlib through the class builder;
//! their methods call the helpers here. Without them (a bare test
//! interpreter) faults stay pending and render from their tag.

use rphp_value::{Array, Object, Str, Value};

use crate::frame::TraceOpts;
use crate::registry::{ErrorKind, FaultSite, PendingThrow, Unwind};
use crate::Interp;

impl Interp {
    /// The class an engine fault of `kind` instantiates, if registered.
    pub fn class_for_kind(&self, kind: ErrorKind) -> Option<u32> {
        let wk = &self.well_known;
        match kind {
            ErrorKind::Error => wk.error,
            ErrorKind::TypeError => wk.type_error,
            ErrorKind::ValueError => wk.value_error,
            ErrorKind::ArgumentCountError => wk.argument_count_error,
            ErrorKind::DivisionByZeroError => wk.division_by_zero_error,
            ErrorKind::ArithmeticError => wk.arithmetic_error,
            ErrorKind::UnhandledMatchError => wk.unhandled_match_error,
            ErrorKind::Exception(name) => self.class_by_name(name.as_bytes()),
        }
    }

    /// The trace a `Throwable` created now records: the current frames
    /// (the frame executing `new` first), honouring
    /// `zend.exception_ignore_args`.
    pub fn exception_trace(&self, skip: usize) -> Array {
        self.build_trace(TraceOpts {
            provide_object: false,
            ignore_args: self.ini.bool("zend.exception_ignore_args"),
            limit: 0,
            skip,
        })
    }

    /// The `native_init` hook of `Exception`/`Error`: `file` and `line` of
    /// the current user frame (where `new` runs) and the trace from there.
    pub fn throwable_init(&mut self, o: &Object) -> Result<(), Unwind> {
        let file = self.current_file();
        let line = self.current_line();
        let trace = self.exception_trace(0);
        o.set(b"file", Value::string(file.as_bytes()));
        o.set(b"line", Value::Int(i64::from(line)));
        o.set(b"trace", Value::Array(trace));
        Ok(())
    }

    /// Capture where a fault happened (the innermost user frame's file and
    /// line, the trace) while the faulting frame is still on top.
    pub(crate) fn capture_site(&self) -> FaultSite {
        FaultSite {
            file: self.current_file(),
            line: self.current_line(),
            trace: self.exception_trace(0),
        }
    }

    /// Build an unwind for an engine fault at an explicit location (class
    /// linking, declaration-time errors).
    pub fn throw_at(&self, kind: ErrorKind, message: impl Into<String>, file: &str, line: u32) -> Unwind {
        Unwind::Pending(PendingThrow {
            kind,
            message: message.into(),
            site: Some(Box::new(FaultSite {
                file: file.to_string(),
                line,
                trace: self.exception_trace(0),
            })),
        })
    }

    /// Instantiate `class` (a `Throwable`) with `message`, `code` and
    /// `previous`, its `file`/`line`/`trace` taken from `site` (else the
    /// current frame). No constructor runs.
    pub fn create_throwable(
        &mut self,
        class: u32,
        message: &str,
        code: i64,
        previous: Option<Object>,
        site: Option<&FaultSite>,
    ) -> Object {
        let o = self.instantiate(class);
        o.set(b"message", Value::string(message.as_bytes()));
        o.set(b"code", Value::Int(code));
        if let Some(p) = previous {
            o.set(b"previous", Value::Object(p));
        }
        match site {
            Some(s) => {
                o.set(b"file", Value::string(s.file.as_bytes()));
                o.set(b"line", Value::Int(i64::from(s.line)));
                o.set(b"trace", Value::Array(s.trace.clone()));
            }
            None => {
                let _ = self.throwable_init(&o);
            }
        }
        o
    }

    /// Turn a pending engine fault into an instance of its class; `Err`
    /// hands the fault back when its class is not registered.
    pub fn materialize(&mut self, p: PendingThrow) -> Result<Object, PendingThrow> {
        let Some(class) = self.class_for_kind(p.kind) else {
            return Err(p);
        };
        let site = match &p.site {
            Some(s) => (**s).clone(),
            None => self.capture_site(),
        };
        Ok(self.create_throwable(class, &p.message, 0, None, Some(&site)))
    }

    /// An unwind as a thrown object when its class exists (`Pending` faults
    /// are materialized), else unchanged.
    pub fn materialize_unwind(&mut self, u: Unwind) -> Unwind {
        match u {
            Unwind::Pending(p) => match self.materialize(p) {
                Ok(o) => Unwind::Throw(o),
                Err(p) => Unwind::Pending(p),
            },
            other => other,
        }
    }

    /// `throw new <kind>(message)` as an object, when the class exists.
    pub fn new_fault(&mut self, kind: ErrorKind, message: &str) -> Unwind {
        self.materialize_unwind(Unwind::Pending(PendingThrow {
            kind,
            message: message.to_string(),
            site: None,
        }))
    }

    // ---- accessors --------------------------------------------------------

    /// A string property of a throwable (`message`, `file`, …).
    pub fn throwable_str(&self, o: &Object, name: &[u8]) -> String {
        o.get_deref(name).map(|v| v.to_php_string()).unwrap_or_default()
    }

    /// An int property of a throwable (`line`, `code`).
    pub fn throwable_int(&self, o: &Object, name: &[u8]) -> i64 {
        o.get_deref(name).map(|v| v.to_int()).unwrap_or(0)
    }

    /// The `previous` throwable, if any.
    pub fn throwable_previous(&self, o: &Object) -> Option<Object> {
        match o.get_deref(b"previous") {
            Some(Value::Object(p)) => Some(p),
            _ => None,
        }
    }

    /// Append `previous` at the end of `o`'s previous chain (php's
    /// `zend_exception_set_previous`): what a throw inside a `finally` with
    /// a pending exception does. A cycle is refused.
    pub fn throwable_chain_previous(&self, o: &Object, previous: Object) {
        if o.ptr_eq(&previous) {
            return;
        }
        let mut cur = o.clone();
        loop {
            match self.throwable_previous(&cur) {
                Some(p) => {
                    if p.ptr_eq(&previous) {
                        return;
                    }
                    cur = p;
                }
                None => break,
            }
        }
        // `previous`'s own chain must not contain `o`.
        let mut c = Some(previous.clone());
        while let Some(p) = c {
            if p.ptr_eq(o) {
                return;
            }
            c = self.throwable_previous(&p);
        }
        cur.set(b"previous", Value::Object(previous));
    }

    /// `getTraceAsString()`.
    pub fn throwable_trace_string(&self, o: &Object) -> String {
        match o.get_deref(b"trace") {
            Some(Value::Array(a)) => self.trace_to_string(&a),
            _ => "#0 {main}".to_string(),
        }
    }

    /// php's `Exception::__toString()`: the chain from the innermost
    /// `previous` outwards, each `Class: message in file:line\nStack
    /// trace:\n…`, joined by `\n\nNext `.
    pub fn throwable_to_string(&self, o: &Object) -> String {
        self.throwable_to_string_ex(o, false)
    }

    /// [`Interp::throwable_to_string`]; `uncaught` applies php's one
    /// finishing touch for the top-level rendering: a `TypeError` whose
    /// message says `, called in X on line N` gets ` and defined` appended,
    /// so the line reads `… called in X on line N and defined in F:L`.
    fn throwable_to_string_ex(&self, o: &Object, uncaught: bool) -> String {
        let mut str = String::new();
        let mut cur = Some(o.clone());
        let mut guard = 0;
        while let Some(e) = cur {
            guard += 1;
            if guard > 10_000 {
                break;
            }
            let class = self.class_name_of(&e);
            let mut message = self.throwable_str(&e, b"message");
            if uncaught && guard == 1 && class == "TypeError" && message.contains(", called in ") {
                message.push_str(" and defined");
            }
            let file = self.throwable_str(&e, b"file");
            let line = self.throwable_int(&e, b"line");
            let trace = self.throwable_trace_string(&e);
            let next = if str.is_empty() {
                String::new()
            } else {
                format!("\n\nNext {str}")
            };
            str = if message.is_empty() {
                format!("{class} in {file}:{line}\nStack trace:\n{trace}{next}")
            } else {
                format!("{class}: {message} in {file}:{line}\nStack trace:\n{trace}{next}")
            };
            cur = self.throwable_previous(&e);
        }
        str
    }

    // ---- stringification ---------------------------------------------------

    /// Convert a value to a string the way `echo`/`.`/`(string)` do:
    /// objects go through `__toString` (an `Error` without it), arrays
    /// warn and become `Array`.
    pub fn to_string(&mut self, v: &Value) -> Result<Str, Unwind> {
        match &*v.deref() {
            Value::Object(o) => self.object_to_string(o),
            Value::Array(_) => {
                self.warn("Array to string conversion")?;
                Ok(Str::new(b"Array"))
            }
            other => Ok(Str::from_vec(other.to_php_bytes())),
        }
    }

    /// `(string) $object`: `__toString()` (which must return a string), or
    /// php's `Object of class X could not be converted to string`.
    pub fn object_to_string(&mut self, o: &Object) -> Result<Str, Unwind> {
        let class = self.class_of(o).clone();
        if class.magic.contains(crate::class::MagicFlags::TOSTRING) {
            let r = self.call_method(o, b"__toString", &[])?;
            return match r.deref().into_owned() {
                Value::Str(s) => Ok(s),
                other => Err(Unwind::type_error(format!(
                    "{}::__toString(): Return value must be of type string, {} returned",
                    class.name_str(),
                    crate::ops::value_name(&other)
                ))),
            };
        }
        Err(Unwind::error(format!(
            "Object of class {} could not be converted to string",
            class.name_str()
        )))
    }

    /// Whether the object can be converted to a string (`Stringable`).
    pub fn has_to_string(&self, o: &Object) -> bool {
        self.class_of(o).magic.contains(crate::class::MagicFlags::TOSTRING)
    }

    // ---- uncaught -------------------------------------------------------------

    /// Render an uncaught throwable object like php-cli (`Uncaught
    /// <__toString()>\n  thrown in <file> on line <N>` through the fatal
    /// error path); a user `__toString` that throws renders that
    /// exception's fault instead.
    pub fn render_uncaught_object(&mut self, o: &Object) {
        let file = self.throwable_str(o, b"file");
        let line = self.throwable_int(o, b"line") as u32;
        let text = if self.class_of(o).internal || !self.has_to_string(o) {
            self.throwable_to_string_ex(o, true)
        } else {
            match self.object_to_string(o) {
                Ok(s) => s.to_string_lossy().into_owned(),
                Err(u) => {
                    let u = self.materialize_unwind(u);
                    let inner = match &u {
                        Unwind::Throw(e) => self.throwable_to_string(e),
                        Unwind::Pending(p) => format!("{}: {}", p.kind.class_name(), p.message),
                        Unwind::Exit(_) => String::new(),
                    };
                    format!(
                        "{inner} in exception handling during call to {}::__toString()",
                        self.class_name_of(o)
                    )
                }
            }
        };
        let message = format!("Uncaught {text}\n  thrown");
        let _ = self.emit_error_full(crate::ErrLevel::Error, &message, &file, line, false);
    }

    /// A one-line `Uncaught Class: message` for an unwind (tests, embedders).
    pub fn describe_unwind(&self, u: &Unwind) -> String {
        match u {
            Unwind::Throw(o) => {
                let message = self.throwable_str(o, b"message");
                let class = self.class_name_of(o);
                if message.is_empty() {
                    format!("Uncaught {class}")
                } else {
                    format!("Uncaught {class}: {message}")
                }
            }
            other => other.describe(),
        }
    }
}
