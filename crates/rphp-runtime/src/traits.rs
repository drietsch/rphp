//! Traits (plan E6): flattening `use T;` into the using class at declaration
//! time.
//!
//! **This module owns trait composition.** `unit.rs` builds the
//! [`ClassSpec`](crate::ClassSpec) for a class and calls
//! [`Interp::apply_trait_uses`] before linking, so by the time
//! `link_class` runs the trait members are ordinary own members.
//!
//! php copies trait members *into* the using class rather than sharing them:
//! the methods' declaring scope becomes the using class, and each using class
//! gets its **own** copy of a trait's static properties (two classes using the
//! same trait do not share a counter).
//!
//! What lands here as E6 fills in:
//!
//! * copy-in of methods, properties (instance and static) and constants, with
//!   the using class's own members taking precedence over a trait's;
//! * conflict resolution — `insteadof` to pick a winner and `as` to alias or
//!   change visibility — and the fatal when two traits collide unresolved;
//! * abstract trait members, which become abstract members of the using
//!   class, and static trait methods;
//! * nested `use` inside a trait, flattened transitively.

use rphp_bytecode::TraitUse;

use crate::class::ClassSpec;
use crate::registry::Unwind;
use crate::Interp;

impl Interp {
    /// Copy the members of every used trait into `spec`, applying the
    /// `insteadof` / `as` adaptations.
    ///
    /// E6 implements this; until then a class that uses a trait links without
    /// the trait's members.
    pub(crate) fn apply_trait_uses(
        &mut self,
        _spec: &mut ClassSpec,
        _uses: &[TraitUse],
    ) -> Result<(), Unwind> {
        Ok(())
    }
}
