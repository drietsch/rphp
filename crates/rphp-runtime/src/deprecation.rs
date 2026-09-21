//! php 8.4's `#[\Deprecated]` attribute on user functions, methods and
//! class constants: the notice php raises on every call or fetch —
//! `Function f() is deprecated`, then ` since <since>` when given, then
//! `, <message>` when given. php requires the arguments to be literals,
//! so they are read from the constant pool (or a literal thunk) at link
//! time and never evaluated as code.

use rphp_bytecode::{AttrDef, Const, InitRef};
use rphp_value::Value;

use crate::unit::UnitRt;
use crate::Interp;

/// The tail of the notice (` since 2.0, no more`) for a `#[\Deprecated]`
/// attribute among `attrs`, whose arguments resolve in `pool`
/// (`InitRef::Const`) or as thunks of `unit` (`InitRef::Thunk`).
pub(crate) fn deprecation_note(
    it: &mut Interp,
    attrs: &[AttrDef],
    pool: &[Const],
    unit: &UnitRt,
) -> Option<Box<str>> {
    let attr = attrs.iter().find(|a| a.name.eq_ignore_ascii_case(b"Deprecated"))?;
    let mut message = None;
    let mut since = None;
    for (i, (name, init)) in attr.args.iter().enumerate() {
        let v = match init {
            InitRef::Const(k) => pool.get(*k as usize).map(Const::to_value),
            InitRef::Thunk(t) => it.run_thunk(unit.func_id(*t), None, None).ok(),
        };
        let Some(v) = v else { continue };
        let text = match v {
            Value::Null => None,
            v => Some(String::from_utf8_lossy(&v.to_php_bytes()).into_owned()),
        };
        let which = match name.as_deref() {
            Some(b"message") => 0,
            Some(b"since") => 1,
            Some(_) => continue,
            None => i,
        };
        match which {
            0 => message = text,
            1 => since = text,
            _ => {}
        }
    }
    let mut note = String::new();
    if let Some(s) = since {
        note.push_str(" since ");
        note.push_str(&s);
    }
    if let Some(m) = message {
        note.push_str(", ");
        note.push_str(&m);
    }
    Some(note.into_boxed_str())
}
