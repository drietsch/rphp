//! Enums (plan E6).
//!
//! **This module owns enum cases.** A case is a *singleton* object: the first
//! evaluation of `E::Case` materializes it into
//! [`EnumCaseInfo::instance`](crate::EnumCaseInfo::instance) and every later
//! one — including `from()`, `tryFrom()` and `cases()` — hands back the same
//! object, so `E::A === E::A` holds and cases compare by identity.
//!
//! What lands here as E6 fills in:
//!
//! * materializing a case with its `name` (and `value` for a backed enum) as
//!   readonly properties;
//! * `cases()`, `from()` and `tryFrom()`, with php's `ValueError` text for a
//!   value that matches no case;
//! * the `UnitEnum` / `BackedEnum` interfaces, which every enum implements
//!   implicitly, so `instanceof` and type checks see them;
//! * the restrictions php enforces on enums: no instance properties, no
//!   `new`, no inheritance, constants and methods allowed.

use rphp_value::Value;

use crate::registry::Unwind;
use crate::Interp;

impl Interp {
    /// `E::Case` — the singleton instance of an enum case.
    ///
    /// E6 implements this; until then no class is an enum.
    pub(crate) fn enum_case(&mut self, cid: u32, name: &[u8]) -> Result<Value, Unwind> {
        Err(Unwind::error(format!(
            "Undefined constant {}::{}",
            self.classes[cid as usize].name_str(),
            String::from_utf8_lossy(name)
        )))
    }
}
