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
//! The copy reads the trait's *linked* [`ClassDef`](crate::ClassDef), which is
//! what gives nested `use` for free: a trait that uses other traits was itself
//! flattened when it was declared, so its members are already own members by
//! the time a class uses it. It is also what makes the declaring scope come
//! out right — `link_class` stamps every member of the spec with the id of the
//! class being linked, so a copied-in method's `decl` (and therefore `self::`,
//! `parent::` and visibility) is the *using* class, and a copied-in static
//! property gets a fresh [`StaticPropInfo::cell`](crate::StaticPropInfo::cell).
//!
//! ## What php does, member kind by member kind
//!
//! * **Methods** — a method declared in the class body silently wins over the
//!   trait's (even over a `final` one); a trait's method wins over an
//!   inherited one, because it is applied as an own member. Two traits
//!   offering the same name is a fatal unless `insteadof` picks a winner or
//!   the class body declares the name itself. `as` adds an alias and/or
//!   changes visibility. An `abstract` trait method never displaces a body and
//!   is displaced by one, wherever the two appear in the composition.
//! * **Properties and constants** — there is *no* precedence here: php
//!   compares the definitions and a difference (default, visibility, `static`,
//!   `readonly`, declared type) is a fatal, whether it is two traits or the
//!   class body against a trait. Only a *parent's* member is overridden
//!   silently.
//!
//! ## Member order
//!
//! php appends the trait members after the class's own ones, per trait in
//! `use` order and inside a trait in declaration order — that is the order
//! `get_class_methods`, `var_dump($obj)` and `ReflectionClass::getConstants`
//! report. Aliases are emitted just before the method they rename, and a
//! concrete method that displaces an abstract one takes the end of the table
//! rather than the abstract's place.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use rphp_bytecode::{ClassKind, TraitAdaptation, TraitUse, TypeDecl, Visibility};
use rphp_value::Value;

use crate::class::{
    ClassSpec, ConstSpec, ConstState, MethodBody, MethodDef, MethodSpec, PropDefault,
};
use crate::registry::{ErrorKind, Unwind};
use crate::unit::FuncRt;
use crate::Interp;

/// The lowercased lookup key of a member name.
fn lkey(name: &[u8]) -> Box<[u8]> {
    name.to_ascii_lowercase().into_boxed_slice()
}

/// Clone a method body for the copy into the using class: a user body shares
/// the compiled [`FuncRt`] (php shares the op array too), a native one is
/// `Copy`.
fn clone_body(body: &MethodBody) -> MethodBody {
    match body {
        MethodBody::User(f) => MethodBody::User(f.clone()),
        MethodBody::Native(n) => MethodBody::Native(*n),
    }
}

/// Whether two initializers are the same declaration, for php's
/// "the definition differs and is considered incompatible" check.
///
/// Two thunks compare equal: evaluating them would run user code, which
/// linking must never do. php folds the constant expressions and compares the
/// results, so this errs towards accepting a composition php accepts
/// (`trait T { const K = 7; public $p = self::K; }` in two traits).
fn same_default(a: &PropDefault, b: &PropDefault) -> bool {
    match (a, b) {
        (PropDefault::Value(x), PropDefault::Value(y)) => x.identical(y),
        (PropDefault::Thunk(_), PropDefault::Thunk(_)) => true,
        _ => false,
    }
}

/// [`same_default`] over declarations that may have no initializer at all.
fn same_init(a: &Option<PropDefault>, b: &Option<PropDefault>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(x), Some(y)) => same_default(x, y),
        _ => false,
    }
}

/// An `as` rule with its trait resolved.
struct AliasRule {
    /// The trait the rule applies to.
    tid: u32,
    /// Lowercased name of the trait method it renames.
    method: Box<[u8]>,
    /// The visibility it forces, if any.
    vis: Option<Visibility>,
    /// The new name, if the rule introduces one.
    alias: Option<Box<[u8]>>,
}

/// A trait method already placed into the using class, for collision
/// detection and for displacing an abstract declaration.
struct Applied {
    /// Index into the pending method vector.
    idx: usize,
    /// The trait it came from (for the collision message).
    tid: u32,
    /// Its name in that trait (for the collision message).
    orig: Box<[u8]>,
    /// Whether it is an abstract declaration.
    is_abstract: bool,
    /// The visibility it was applied with.
    vis: Visibility,
    /// The compiled body, for the "same function" test.
    func: Option<Rc<FuncRt>>,
}

/// The declaration of a property, for the compatibility check. Instance and
/// static properties share one namespace, as they do in php's property table.
struct PropSig {
    /// The trait it came from; `None` for the class's own declaration.
    owner: Option<u32>,
    vis: Visibility,
    is_static: bool,
    readonly: bool,
    ty: Option<TypeDecl>,
    default: Option<PropDefault>,
}

impl PropSig {
    /// Whether two declarations of the same property name are the same
    /// declaration.
    fn compatible(&self, other: &PropSig) -> bool {
        self.vis == other.vis
            && self.is_static == other.is_static
            && self.readonly == other.readonly
            && self.ty == other.ty
            && same_init(&self.default, &other.default)
    }
}

/// The declaration of a class constant, for the compatibility check.
struct ConstSig {
    /// The trait it came from; `None` for the class's own declaration.
    owner: Option<u32>,
    vis: Visibility,
    is_final: bool,
    ty: Option<TypeDecl>,
    init: PropDefault,
}

impl ConstSig {
    /// Whether two declarations of the same constant are the same declaration.
    fn compatible(&self, other: &ConstSig) -> bool {
        self.vis == other.vis
            && self.is_final == other.is_final
            && self.ty == other.ty
            && same_default(&self.init, &other.init)
    }
}

impl Interp {
    /// Copy the members of every used trait into `spec`, applying the
    /// `insteadof` / `as` adaptations.
    ///
    /// Called by `declare_class` just before [`Interp::link_class`], so every
    /// member added here is an *own* member of the using class.
    pub(crate) fn apply_trait_uses(
        &mut self,
        spec: &mut ClassSpec,
        uses: &[TraitUse],
    ) -> Result<(), Unwind> {
        if uses.is_empty() {
            return Ok(());
        }
        let class_name = String::from_utf8_lossy(&spec.name).into_owned();
        // php reports every composition fault at the line of the class
        // declaration, not of the `use` statement.
        let (file, line) = match &spec.declared_at {
            Some((f, l)) => (f.to_string(), *l),
            None => (String::from("Unknown"), 0),
        };

        let tids = self.resolve_used_traits(uses, &class_name, &file, line)?;
        let (excludes, aliases) = self.resolve_adaptations(uses, &tids, &class_name, &file, line)?;

        self.copy_trait_methods(spec, &tids, &excludes, &aliases, &class_name, &file, line)?;
        self.copy_trait_props(spec, &tids, &class_name, &file, line)?;
        self.copy_trait_consts(spec, &tids, &class_name, &file, line)?;
        // Keep them for `class_uses()`, which reports the names a class used
        // itself — not what its parent used.
        spec.used_traits = tids;
        Ok(())
    }

    /// Resolve the names of every `use` statement to linked trait ids, in
    /// order and without repeats (`use T; use T;` is legal and idempotent).
    fn resolve_used_traits(
        &mut self,
        uses: &[TraitUse],
        class_name: &str,
        file: &str,
        line: u32,
    ) -> Result<Vec<u32>, Unwind> {
        let mut tids: Vec<u32> = Vec::new();
        for u in uses {
            for name in &u.names {
                // Same resolution path as `extends`/`implements`, autoload
                // included: a trait in another file is how Composer-managed
                // code ships one (`use LoggerTrait;` in `AbstractLogger`).
                let tid = match self.lookup_class(name)? {
                    Some(tid) if self.classes[tid as usize].linked => tid,
                    _ => {
                        let msg =
                            format!("Trait \"{}\" not found", String::from_utf8_lossy(name));
                        return Err(self.throw_at(ErrorKind::Error, msg, file, line));
                    }
                };
                let t = self.classes[tid as usize].clone();
                if t.kind != ClassKind::Trait {
                    let msg = format!(
                        "{class_name} cannot use {} - it is not a trait",
                        t.name_str()
                    );
                    return Err(self.throw_at(ErrorKind::Error, msg, file, line));
                }
                if !tids.contains(&tid) {
                    tids.push(tid);
                }
            }
        }
        Ok(tids)
    }

    /// The trait an `insteadof`/`as` rule names, which must be one of the
    /// traits the class actually uses.
    fn adaptation_trait(&self, tids: &[u32], name: &[u8], class_name: &str) -> Result<u32, String> {
        let key = lkey(name);
        for &tid in tids {
            if self.classes[tid as usize].lname == key {
                return Ok(tid);
            }
        }
        match self.class_by_name(name) {
            Some(other) => Err(format!(
                "Required Trait {} wasn't added to {class_name}",
                self.classes[other as usize].name_str()
            )),
            None => Err(format!(
                "Could not find trait {}",
                String::from_utf8_lossy(name)
            )),
        }
    }

    /// Turn the `use` statements' adaptation blocks into an exclusion list
    /// (`insteadof`) and a list of resolved `as` rules. Adaptations apply to
    /// the whole composition, not only to the statement they appear in.
    fn resolve_adaptations(
        &mut self,
        uses: &[TraitUse],
        tids: &[u32],
        class_name: &str,
        file: &str,
        line: u32,
    ) -> Result<(Vec<(u32, Box<[u8]>)>, Vec<AliasRule>), Unwind> {
        let mut excludes: Vec<(u32, Box<[u8]>)> = Vec::new();
        let mut aliases: Vec<AliasRule> = Vec::new();
        for a in uses.iter().flat_map(|u| u.adaptations.iter()) {
            match a {
                TraitAdaptation::Precedence {
                    trait_,
                    method,
                    insteadof,
                } => {
                    let win = match self.adaptation_trait(tids, trait_, class_name) {
                        Ok(id) => id,
                        Err(msg) => return Err(self.fatal_at(&msg, file, line)),
                    };
                    if self.classes[win as usize].method(method).is_none() {
                        let msg = format!(
                            "A precedence rule was defined for {}::{} but this method does not exist",
                            self.classes[win as usize].name_str(),
                            String::from_utf8_lossy(method)
                        );
                        return Err(self.fatal_at(&msg, file, line));
                    }
                    // php does not require the losing traits to have the
                    // method; the rule just excludes it if they do.
                    for other in insteadof {
                        let oid = match self.adaptation_trait(tids, other, class_name) {
                            Ok(id) => id,
                            Err(msg) => return Err(self.fatal_at(&msg, file, line)),
                        };
                        excludes.push((oid, lkey(method)));
                    }
                }
                TraitAdaptation::Alias {
                    trait_,
                    method,
                    vis,
                    alias,
                } => {
                    let tid = match trait_ {
                        Some(t) => {
                            let tid = match self.adaptation_trait(tids, t, class_name) {
                                Ok(id) => id,
                                Err(msg) => return Err(self.fatal_at(&msg, file, line)),
                            };
                            if self.classes[tid as usize].method(method).is_none() {
                                let msg = format!(
                                    "An alias was defined for {}::{} but this method does not exist",
                                    self.classes[tid as usize].name_str(),
                                    String::from_utf8_lossy(method)
                                );
                                return Err(self.fatal_at(&msg, file, line));
                            }
                            tid
                        }
                        None => {
                            let a = alias.as_deref();
                            self.unqualified_alias_trait(tids, method, a, file, line)?
                        }
                    };
                    aliases.push(AliasRule {
                        tid,
                        method: lkey(method),
                        vis: *vis,
                        alias: alias.clone(),
                    });
                }
            }
        }
        Ok((excludes, aliases))
    }

    /// The trait a bare `m as …` rule refers to: exactly one of the used
    /// traits must offer `m`. (Exclusion by `insteadof` does not disambiguate
    /// — php still calls it ambiguous.)
    fn unqualified_alias_trait(
        &mut self,
        tids: &[u32],
        method: &[u8],
        alias: Option<&[u8]>,
        file: &str,
        line: u32,
    ) -> Result<u32, Unwind> {
        let found: Vec<u32> = tids
            .iter()
            .copied()
            .filter(|&tid| self.classes[tid as usize].method(method).is_some())
            .collect();
        match found.as_slice() {
            [one] => Ok(*one),
            [] => {
                let m = String::from_utf8_lossy(method).into_owned();
                let msg = match alias {
                    Some(a) => format!(
                        "An alias ({}) was defined for method {m}(), but this method does not exist",
                        String::from_utf8_lossy(a)
                    ),
                    None => format!(
                        "The modifiers of the trait method {m}() are changed, but this method does not exist. Error"
                    ),
                };
                Err(self.fatal_at(&msg, file, line))
            }
            [a, b, ..] => {
                let m = String::from_utf8_lossy(method).into_owned();
                let an = self.classes[*a as usize].name_str();
                let bn = self.classes[*b as usize].name_str();
                let msg = format!(
                    "An alias was defined for method {m}(), which exists in both {an} and {bn}. \
                     Use {an}::{m} or {bn}::{m} to resolve the ambiguity"
                );
                Err(self.fatal_at(&msg, file, line))
            }
        }
    }

    /// Append every used trait's methods to `spec`, applying the aliases and
    /// exclusions and reporting unresolved collisions.
    #[allow(clippy::too_many_arguments)]
    fn copy_trait_methods(
        &mut self,
        spec: &mut ClassSpec,
        tids: &[u32],
        excludes: &[(u32, Box<[u8]>)],
        aliases: &[AliasRule],
        class_name: &str,
        file: &str,
        line: u32,
    ) -> Result<(), Unwind> {
        // A method the class body declares beats the trait's silently — it is
        // also why a class can resolve a two-trait collision by declaring the
        // name itself.
        let own: HashSet<Box<[u8]>> = spec.methods.iter().map(|m| lkey(&m.name)).collect();
        let mut out: Vec<Option<MethodSpec>> = Vec::new();
        let mut applied: HashMap<Box<[u8]>, Applied> = HashMap::new();

        for &tid in tids {
            let t = self.classes[tid as usize].clone();
            for key in &t.method_order {
                let Some(m) = t.methods.get(key) else { continue };
                // A trait has neither parent nor interfaces, so every entry is
                // its own; guard anyway so a future model change cannot leak
                // foreign members in.
                if m.decl != tid {
                    continue;
                }
                // The rules of this composition that name this very method.
                let rules: Vec<&AliasRule> = aliases
                    .iter()
                    .filter(|r| r.tid == tid && r.method == *key)
                    .collect();
                // `T::m as n` — the alias is applied just before `m` itself.
                for r in &rules {
                    if let Some(a) = &r.alias {
                        let vis = r.vis.unwrap_or(m.vis);
                        self.place_trait_method(
                            &mut out, &mut applied, &own, a, vis, m, tid, class_name, file, line,
                        )?;
                    }
                }
                if excludes.iter().any(|(x, k)| *x == tid && k == key) {
                    continue;
                }
                // `T::m as protected` — a rule without a new name only
                // changes the visibility of `m` itself.
                let mut vis = m.vis;
                for r in &rules {
                    if r.alias.is_none() {
                        if let Some(v) = r.vis {
                            vis = v;
                        }
                    }
                }
                self.place_trait_method(
                    &mut out, &mut applied, &own, &m.name, vis, m, tid, class_name, file, line,
                )?;
            }
        }
        spec.methods.extend(out.into_iter().flatten());
        Ok(())
    }

    /// Place one trait method under `name`, or diagnose the collision.
    #[allow(clippy::too_many_arguments)]
    fn place_trait_method(
        &mut self,
        out: &mut Vec<Option<MethodSpec>>,
        applied: &mut HashMap<Box<[u8]>, Applied>,
        own: &HashSet<Box<[u8]>>,
        name: &[u8],
        vis: Visibility,
        m: &MethodDef,
        tid: u32,
        class_name: &str,
        file: &str,
        line: u32,
    ) -> Result<(), Unwind> {
        let key = lkey(name);
        if own.contains(&key) {
            return Ok(());
        }
        if let Some(prev) = applied.get(&key) {
            // An abstract declaration never displaces anything already there,
            // whether that is a body or another abstract declaration.
            if m.is_abstract {
                return Ok(());
            }
            if prev.is_abstract {
                // A body displaces an abstract declaration. php removes the
                // abstract entry and re-adds the body, so the method moves to
                // the end of the table.
                let idx = prev.idx;
                out[idx] = None;
            } else {
                // The same function applied twice under the same visibility is
                // not a collision — that is how `use T, U;` works when `U`
                // itself uses `T`, and how `T::m as m;` works.
                let same_fn = match (&prev.func, m.user_func()) {
                    (Some(a), Some(b)) => Rc::ptr_eq(a, b),
                    _ => false,
                };
                if same_fn && prev.vis == vis {
                    return Ok(());
                }
                let msg = format!(
                    "Trait method {}::{} has not been applied as {class_name}::{}, \
                     because of collision with {}::{}",
                    self.classes[tid as usize].name_str(),
                    String::from_utf8_lossy(&m.name),
                    String::from_utf8_lossy(name),
                    self.classes[prev.tid as usize].name_str(),
                    String::from_utf8_lossy(&prev.orig),
                );
                return Err(self.fatal_at(&msg, file, line));
            }
        }
        let idx = out.len();
        out.push(Some(MethodSpec {
            name: Box::from(name),
            body: clone_body(&m.body),
            vis,
            is_static: m.is_static,
            is_abstract: m.is_abstract,
            is_final: m.is_final,
        }));
        applied.insert(
            key,
            Applied {
                idx,
                tid,
                orig: m.name.clone(),
                is_abstract: m.is_abstract,
                vis,
                func: m.user_func().cloned(),
            },
        );
        Ok(())
    }

    /// Append every used trait's properties — instance and static — to
    /// `spec`. A name the composition already declares must be declared
    /// identically; there is no precedence between a trait and the class body
    /// here, only between a trait and the *parent* (which `link_class`
    /// resolves in the trait's favour, since the copy is an own member).
    fn copy_trait_props(
        &mut self,
        spec: &mut ClassSpec,
        tids: &[u32],
        class_name: &str,
        file: &str,
        line: u32,
    ) -> Result<(), Unwind> {
        let mut sigs: HashMap<Box<[u8]>, PropSig> = HashMap::new();
        for ps in &spec.props {
            let (name, vis, ty, readonly, default) = (
                &ps.name,
                &ps.vis,
                &ps.ty,
                &ps.readonly,
                &ps.default,
            );
            sigs.insert(
                name.clone(),
                PropSig {
                    owner: None,
                    vis: *vis,
                    is_static: false,
                    readonly: *readonly,
                    ty: ty.clone(),
                    default: Some(default.clone()),
                },
            );
        }
        for (name, vis, ty, init) in &spec.static_props {
            sigs.insert(
                name.clone(),
                PropSig {
                    owner: None,
                    vis: *vis,
                    is_static: true,
                    readonly: false,
                    ty: ty.clone(),
                    default: init.clone(),
                },
            );
        }

        for &tid in tids {
            let t = self.classes[tid as usize].clone();
            for p in &t.props {
                let sig = PropSig {
                    owner: Some(tid),
                    vis: p.vis,
                    is_static: false,
                    readonly: p.readonly,
                    ty: p.ty.clone(),
                    default: Some(p.default.clone()),
                };
                if let Some(prev) = sigs.get(&p.name) {
                    if !prev.compatible(&sig) {
                        let member = format!("${}", String::from_utf8_lossy(&p.name));
                        let msg =
                            self.member_conflict(prev.owner, tid, "property", &member, class_name);
                        return Err(self.fatal_at(&msg, file, line));
                    }
                    continue;
                }
                spec.props.push(crate::class::PropSpec {
                    name: p.name.clone(),
                    vis: p.vis,
                    set_vis: p.set_vis,
                    ty: p.ty.clone(),
                    readonly: p.readonly,
                    hooks: p.hooks,
                    default: p.default.clone(),
                });
                sigs.insert(p.name.clone(), sig);
            }
            for s in &t.static_props {
                // Each using class gets its own cell: only the *initializer*
                // is copied, and `link_class` makes a fresh
                // `Rc<RefCell<Value>>` for it.
                let init = match (&s.init, s.ready.get()) {
                    (Some(pd), _) => Some(pd.clone()),
                    (None, true) => Some(PropDefault::Value(s.cell.borrow().clone())),
                    (None, false) => None,
                };
                let sig = PropSig {
                    owner: Some(tid),
                    vis: s.vis,
                    is_static: true,
                    readonly: false,
                    ty: s.ty.clone(),
                    default: init.clone(),
                };
                if let Some(prev) = sigs.get(&s.name) {
                    if !prev.compatible(&sig) {
                        let member = format!("${}", String::from_utf8_lossy(&s.name));
                        let msg =
                            self.member_conflict(prev.owner, tid, "property", &member, class_name);
                        return Err(self.fatal_at(&msg, file, line));
                    }
                    continue;
                }
                spec.static_props
                    .push((s.name.clone(), s.vis, s.ty.clone(), init));
                sigs.insert(s.name.clone(), sig);
            }
        }
        Ok(())
    }

    /// Append every used trait's constants (php 8.2) to `spec`, with the same
    /// "must be declared identically" rule as properties.
    fn copy_trait_consts(
        &mut self,
        spec: &mut ClassSpec,
        tids: &[u32],
        class_name: &str,
        file: &str,
        line: u32,
    ) -> Result<(), Unwind> {
        let mut sigs: HashMap<Box<[u8]>, ConstSig> = HashMap::new();
        for c in &spec.consts {
            sigs.insert(
                c.name.clone(),
                ConstSig {
                    owner: None,
                    vis: c.vis,
                    is_final: c.is_final,
                    ty: c.ty.clone(),
                    init: c.init.clone(),
                },
            );
        }
        for &tid in tids {
            let t = self.classes[tid as usize].clone();
            for name in &t.const_order {
                let Some(c) = t.consts.get(name) else { continue };
                if c.decl != tid {
                    continue;
                }
                // The copy is re-evaluated in the using class's scope, so a
                // trait constant written as `self::X` sees the using class.
                let init = match &*c.state.borrow() {
                    ConstState::Pending(pd) => pd.clone(),
                    ConstState::Ready(v) => PropDefault::Value(v.clone()),
                    ConstState::Evaluating => PropDefault::Value(Value::Null),
                };
                let sig = ConstSig {
                    owner: Some(tid),
                    vis: c.vis,
                    is_final: c.is_final,
                    ty: c.ty.clone(),
                    init: init.clone(),
                };
                if let Some(prev) = sigs.get(&c.name) {
                    if !prev.compatible(&sig) {
                        let msg = self.member_conflict(
                            prev.owner,
                            tid,
                            "constant",
                            &String::from_utf8_lossy(&c.name),
                            class_name,
                        );
                        return Err(self.fatal_at(&msg, file, line));
                    }
                    continue;
                }
                spec.consts.push(ConstSpec {
                    name: c.name.clone(),
                    vis: c.vis,
                    is_final: c.is_final,
                    ty: c.ty.clone(),
                    init,
                });
                sigs.insert(c.name.clone(), sig);
            }
        }
        Ok(())
    }

    /// php's text for two incompatible declarations of the same property or
    /// constant in a composition. `existing` is the trait that declared it
    /// first, or `None` for the class body.
    fn member_conflict(
        &self,
        existing: Option<u32>,
        new: u32,
        kind: &str,
        member: &str,
        class_name: &str,
    ) -> String {
        let first = match existing {
            Some(tid) => self.classes[tid as usize].name_str(),
            None => class_name.to_string(),
        };
        format!(
            "{first} and {} define the same {kind} ({member}) in the composition of {class_name}. \
             However, the definition differs and is considered incompatible. Class was composed",
            self.classes[new as usize].name_str()
        )
    }
}
