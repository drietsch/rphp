//! Name resolution (plan F4, ADR-020/026): fills [`Name::resolved`] on every
//! class, function and constant reference, folds `X::class` on static names,
//! substitutes magic constants and reports php's compile-time resolution
//! errors with php's own texts.
//!
//! Rules implemented (each verified against php 8.5.0, see `tests/`):
//!
//! * **Namespaces.** The current namespace is per statement: `namespace X;`
//!   applies to the statements that follow it (the adapter groups them into
//!   the statement's body), `namespace X { }` to its block, and each block
//!   starts with empty import tables. Mixing the braced and unbraced forms is
//!   `RPHP_E0206`.
//! * **Imports.** Three tables per namespace block: classes and functions
//!   keyed by the lowercased alias, constants by the alias as written. Group
//!   use (`use A\{B, C as D, function f, const X}`) expands the prefix. A
//!   second import of the same alias, or an alias that spells a symbol
//!   already declared in this file, is `RPHP_E0200` (`Cannot use X as Y
//!   because the name is already in use`); an alias that is a reserved class
//!   name is `RPHP_E0201`. At global scope a non-compound `use` without alias
//!   is php's warning `The use statement with non-compound name 'X' has no
//!   effect`.
//! * **Class names** (`new`, `instanceof`, `extends`, `implements`, `catch`,
//!   `X::`, type hints, attributes, trait use and adaptations): fully
//!   qualified as written; `namespace\X` relative to the current namespace;
//!   unqualified through the class import table else prefixed with the
//!   namespace; qualified with the first segment through the import table.
//!   The result is `Resolved::Class{fqn, key}` with `key` the lowercased
//!   `fqn`. Reserved type names are *not* special in class positions (php
//!   looks `N\int` up); they are rejected as declared class names (`Cannot
//!   use "int" as a class name as it is reserved`), aliases, and as
//!   `extends`/`implements`/`use`/`catch` operands when they spell
//!   `self`/`static`/`parent`.
//! * **Functions.** Qualified/fully qualified/relative names resolve to one
//!   name (`ns_key: None`); an unqualified name goes through the function
//!   import table, else — inside a namespace — to the runtime two-step
//!   `Resolved::Func{ns_key: Some(ns\f), global_key: f}`. Names keep the
//!   spelling php reports in `Call to undefined function N\f()`; the
//!   bytecode's `NameConst` carries the lowercased lookup twin. Nothing is
//!   folded (`strlen` stays a two-step call).
//! * **Constants.** Same shape; the constant import table is case-sensitive
//!   and the runtime's constant table folds only the namespace part
//!   ([`crate::scope::const_key`]). `__COMPILER_HALT_OFFSET__` is folded to
//!   the program's halt offset when there is one.
//! * **`self`/`static`/`parent`.** Kept as `ClassRef` keywords for the
//!   compiler. Where php knows the scope at compile time (free functions,
//!   non-trait class bodies; not closures, the pseudo-main, traits or
//!   constant-expression positions) `self`/`static` without a class scope
//!   are `RPHP_E0204` and `parent` in a class without parent is
//!   `RPHP_E0205`. `\self` and friends are `'\self' is an invalid class name`.
//! * **`::class`.** `Name::class` becomes `Str(fqn)`; `self::class` and
//!   `parent::class` fold too when the scope is known and the class is named;
//!   `static::class` and `$expr::class` stay.
//! * **Magic constants.** See [`crate::magic`].
//! * **Declarations.** A class-like whose unqualified name is an import
//!   alias of a different class is `Cannot redeclare class X (previously
//!   declared as local import)`; likewise functions and constants. Two
//!   declarations of the same function or class-like in top-level position
//!   (the statement list, plain blocks, namespace bodies) are
//!   `RPHP_E0102`/`RPHP_E0106` with php's `Cannot redeclare ...` text. The
//!   declared name of a function, class-like and file-level `const` is
//!   rewritten to its FQN (`N\f`, `N\C`, `N\X`): that is the name the
//!   runtime tables register, and what `__FUNCTION__`/`__CLASS__` print.

use rphp_ast::v2::visit::mutable::{
    walk_arg, walk_class_ref, walk_expr, walk_func_decl, walk_hook, walk_method_decl, walk_stmt,
    walk_type,
};
use rphp_ast::v2::{
    Adaptation, Arg, Callee, ClassKind, ClassLike, ClassRef, ConstSel, Expr, FuncDecl, Hook,
    HookKind, MagicKind, Member, MethodDecl, Name, NameKind, NewTarget, Param, Program, PropMember,
    Resolved, Stmt, Type, TypeKind, UseKind, VisitorMut,
};
use rphp_diagnostics::{codes, Diagnostic};
use rphp_intern::{IdentId, Interner};
use rphp_span::Span;

use crate::magic::{Magic, MagicCtx};
use crate::scope::{
    alias_key, is_reserved_class_name, is_scope_keyword, last_segment, split_first,
    ClassScope, FnKind, Import, NamespaceScope, ScopeStack, SeenSymbols,
};
use crate::LowerOptions;

/// Run the resolver over `program` in place.
pub(crate) fn run(
    program: &mut Program,
    interner: &mut Interner,
    opts: &LowerOptions<'_>,
    diags: &mut Vec<Diagnostic>,
) {
    let mut r = Resolver {
        it: interner,
        diags,
        line_of: opts.line_of,
        file: opts.file_value(),
        dir: opts.dir_value(),
        halt_offset: program.halt_offset,
        ns: NamespaceScope::default(),
        stack: ScopeStack::default(),
        seen: SeenSymbols::default(),
        const_depth: 0,
        top_level: true,
        cur_prop: None,
    };
    r.check_namespace_mixing(&program.items);
    r.visit_block(&mut program.items);
}

struct Resolver<'a> {
    it: &'a mut Interner,
    diags: &'a mut Vec<Diagnostic>,
    line_of: &'a dyn Fn(Span) -> u32,
    file: Vec<u8>,
    dir: Vec<u8>,
    halt_offset: Option<u32>,
    ns: NamespaceScope,
    stack: ScopeStack,
    seen: SeenSymbols,
    /// Depth of constant-expression positions (defaults, class constants,
    /// enum cases, attribute arguments): `self`/`parent` are not scope-checked
    /// there, as in php.
    const_depth: u32,
    /// The statement list being walked is reached by php's top-level
    /// compilation (duplicate-declaration rule).
    top_level: bool,
    /// The property whose hooks are being walked (for `__PROPERTY__`).
    cur_prop: Option<IdentId>,
}

impl Resolver<'_> {
    // ----- helpers -----------------------------------------------------------

    fn error(&mut self, code: &'static str, msg: impl Into<String>, span: Span) {
        self.diags
            .push(Diagnostic::error(code, msg).with_primary(span, ""));
    }

    fn warning(&mut self, code: &'static str, msg: impl Into<String>, span: Span) {
        self.diags
            .push(Diagnostic::warning(code, msg).with_primary(span, ""));
    }

    fn text(&self, id: IdentId) -> Vec<u8> {
        self.it.resolve(id).to_vec()
    }

    fn lossy(&self, bytes: &[u8]) -> String {
        String::from_utf8_lossy(bytes).into_owned()
    }

    fn intern(&mut self, bytes: &[u8]) -> IdentId {
        self.it.intern(bytes)
    }

    fn line(&self, span: Span) -> u32 {
        (self.line_of)(span)
    }

    fn magic_ctx(&self) -> MagicCtx<'_> {
        MagicCtx {
            file: &self.file,
            dir: &self.dir,
            namespace: &self.ns.name,
            stack: &self.stack,
        }
    }

    /// Run `f` with the top-level flag cleared (bodies of `if`, loops,
    /// functions, `declare { }`, ...).
    fn nested<T>(&mut self, f: impl FnOnce(&mut Self) -> T) -> T {
        let saved = self.top_level;
        self.top_level = false;
        let r = f(self);
        self.top_level = saved;
        r
    }

    /// Run `f` inside a constant-expression position.
    fn in_const<T>(&mut self, f: impl FnOnce(&mut Self) -> T) -> T {
        self.const_depth += 1;
        let r = f(self);
        self.const_depth -= 1;
        r
    }

    // ----- name resolution ---------------------------------------------------

    /// The FQN a class name denotes (php's `zend_resolve_class_name`).
    fn class_fqn(&self, n: &Name) -> Vec<u8> {
        let text = self.it.resolve(n.text);
        match n.kind {
            NameKind::FullyQualified => text.to_vec(),
            NameKind::Relative => self.ns.prefix(text),
            NameKind::Unqualified => match self.ns.imports.class.get(&text.to_ascii_lowercase()) {
                Some(i) => i.fqn.clone(),
                None => self.ns.prefix(text),
            },
            NameKind::Qualified => self.qualified_via_class_imports(text),
        }
    }

    /// A qualified name whose first segment may be a class/namespace alias.
    fn qualified_via_class_imports(&self, text: &[u8]) -> Vec<u8> {
        if let Some((first, rest)) = split_first(text) {
            if let Some(i) = self.ns.imports.class.get(&first.to_ascii_lowercase()) {
                let mut out = i.fqn.clone();
                out.push(b'\\');
                out.extend_from_slice(rest);
                return out;
            }
        }
        self.ns.prefix(text)
    }

    /// Fill `n.resolved` for a class position and return the FQN.
    fn resolve_class(&mut self, n: &mut Name) -> Vec<u8> {
        let fqn = self.class_fqn(n);
        let key = fqn.to_ascii_lowercase();
        n.resolved = Some(Resolved::Class {
            fqn: self.intern(&fqn),
            key: self.intern(&key),
        });
        fqn
    }

    /// Fill `n.resolved` for a function position. The names keep the
    /// spelling php reports (`Call to undefined function Foo\BAR\f()`); the
    /// bytecode's `NameConst` derives the case-insensitive lookup key.
    fn resolve_func(&mut self, n: &mut Name) {
        let text = self.text(n.text);
        let (ns_key, global_key): (Option<Vec<u8>>, Vec<u8>) = match n.kind {
            NameKind::FullyQualified => (None, text),
            NameKind::Relative => (None, self.ns.prefix(&text)),
            NameKind::Qualified => (None, self.qualified_via_class_imports(&text)),
            NameKind::Unqualified => {
                let lower = text.to_ascii_lowercase();
                match self.ns.imports.func.get(&lower) {
                    Some(i) => (None, i.fqn.clone()),
                    None if self.ns.name.is_empty() => (None, text),
                    None => (Some(self.ns.prefix(&text)), text),
                }
            }
        };
        n.resolved = Some(Resolved::Func {
            ns_key: ns_key.map(|k| self.intern(&k)),
            global_key: self.intern(&global_key),
        });
    }

    /// Fill `n.resolved` for a constant position, or fold
    /// `__COMPILER_HALT_OFFSET__`. Returns the folded expression if any.
    fn resolve_const(&mut self, n: &mut Name) -> Option<Expr> {
        let text = self.text(n.text);
        if text == b"__COMPILER_HALT_OFFSET__"
            && matches!(n.kind, NameKind::Unqualified | NameKind::FullyQualified)
        {
            if let Some(off) = self.halt_offset {
                return Some(Expr::Int(i64::from(off), n.span));
            }
        }
        // As for functions: the spelling php reports (`Undefined constant
        // "Foo\BAR\X"`); the runtime's constant table folds the namespace
        // part and keeps the last segment case-sensitive.
        let (ns_key, global_key): (Option<Vec<u8>>, Vec<u8>) = match n.kind {
            NameKind::FullyQualified => (None, text),
            NameKind::Relative => (None, self.ns.prefix(&text)),
            NameKind::Qualified => (None, self.qualified_via_class_imports(&text)),
            NameKind::Unqualified => match self.ns.imports.const_.get(&text) {
                Some(i) => (None, i.fqn.clone()),
                None if self.ns.name.is_empty() => (None, text),
                None => (Some(self.ns.prefix(&text)), text),
            },
        };
        n.resolved = Some(Resolved::Const {
            ns_key: ns_key.map(|k| self.intern(&k)),
            global_key: self.intern(&global_key),
        });
        None
    }

    // ----- scope keyword checks ---------------------------------------------

    /// `zend_ensure_valid_class_fetch_type`: `self`/`static`/`parent` in an
    /// expression position where php knows the scope.
    fn check_scope_keyword(&mut self, c: &ClassRef) {
        if self.const_depth > 0 || !self.stack.scope_known() {
            return;
        }
        let (word, span) = match c {
            ClassRef::SelfKw(s) => ("self", *s),
            ClassRef::Static(s) => ("static", *s),
            ClassRef::Parent(s) => ("parent", *s),
            _ => return,
        };
        match self.stack.effective_class() {
            None => self.error(
                codes::NO_CLASS_SCOPE,
                format!("Cannot use \"{word}\" when no class scope is active"),
                span,
            ),
            Some(c) if word == "parent" && c.parent.is_none() => {
                self.error(
                    codes::NO_PARENT_SCOPE,
                    "Cannot use \"parent\" when current class scope has no parent",
                    span,
                );
            }
            Some(_) => {}
        }
    }

    /// A written name that spells `self`/`static`/`parent` in a position that
    /// only takes real class names.
    fn check_reserved_reference(&mut self, n: &Name, what: &str) -> bool {
        let text = self.text(n.text);
        if n.kind == NameKind::Unqualified && is_scope_keyword(&text) {
            let word = self.lossy(&text).to_ascii_lowercase();
            self.error(
                codes::RESERVED_CLASS_NAME,
                format!("Cannot use \"{word}\" as {what}, as it is reserved"),
                n.span,
            );
            return true;
        }
        if n.kind == NameKind::FullyQualified && is_scope_keyword(&text) {
            let word = self.lossy(&text);
            self.error(
                codes::RESERVED_CLASS_NAME,
                format!("'\\{word}' is an invalid class name"),
                n.span,
            );
            return true;
        }
        false
    }

    // ----- namespaces and imports ------------------------------------------

    fn check_namespace_mixing(&mut self, items: &[Stmt]) {
        let mut seen_braced = false;
        let mut seen_unbraced = false;
        for s in items {
            if let Stmt::Namespace { braced, span, .. } = s {
                if (*braced && seen_unbraced) || (!*braced && seen_braced) {
                    self.error(
                        codes::NAMESPACE_MIX,
                        "Cannot mix bracketed namespace declarations with unbracketed namespace declarations",
                        *span,
                    );
                }
                if *braced {
                    seen_braced = true;
                } else {
                    seen_unbraced = true;
                }
            }
        }
    }

    fn use_type_str(kind: UseKind) -> &'static str {
        match kind {
            UseKind::Class => "",
            UseKind::Function => " function",
            UseKind::Const => " const",
        }
    }

    /// One `use` item (php's `zend_compile_use` per name).
    fn import(&mut self, kind: UseKind, fqn: Vec<u8>, alias: Option<Vec<u8>>, span: Span) {
        let compound = fqn.contains(&b'\\');
        let alias_written = alias.is_some();
        let alias = alias.unwrap_or_else(|| last_segment(&fqn).to_vec());
        let lookup = alias_key(kind, &alias);
        let fqn_s = self.lossy(&fqn);
        let alias_s = self.lossy(&alias);
        if kind == UseKind::Class && is_reserved_class_name(&alias) {
            self.error(
                codes::RESERVED_CLASS_NAME,
                format!(
                    "Cannot use {fqn_s} as {alias_s} because '{alias_s}' is a special class name"
                ),
                span,
            );
            return;
        }
        if self.ns.name.is_empty() && !compound && !alias_written {
            self.warning(
                codes::IMPORT_CONFLICT,
                format!("The use statement with non-compound name '{alias_s}' has no effect"),
                span,
            );
        }
        // A declaration seen earlier in this file under the aliased name
        // (php's `zend_have_seen_symbol`; constants are not registered).
        let check_key = self.ns.prefix(&alias).to_ascii_lowercase();
        let seen = match kind {
            UseKind::Class => self.seen.classes.contains_key(&check_key),
            UseKind::Function => self.seen.functions.contains_key(&check_key),
            UseKind::Const => false,
        };
        let already = format!(
            "Cannot use{} {fqn_s} as {alias_s} because the name is already in use",
            Self::use_type_str(kind)
        );
        if seen && !fqn.eq_ignore_ascii_case(&self.ns.prefix(&alias)) {
            self.error(codes::IMPORT_CONFLICT, already, span);
            return;
        }
        let table = self.ns.imports.table_mut(kind);
        if table.contains_key(&lookup) {
            self.error(codes::IMPORT_CONFLICT, already, span);
            return;
        }
        table.insert(lookup, Import { fqn, span });
    }

    fn use_stmt(&mut self, kind: UseKind, prefix: &Option<Name>, items: &[rphp_ast::v2::UseItem]) {
        let prefix_text = prefix.as_ref().map(|p| self.text(p.text));
        for it in items {
            let raw = self.text(it.name.text);
            let raw = raw.strip_prefix(b"\\").map(<[u8]>::to_vec).unwrap_or(raw);
            let fqn = match &prefix_text {
                Some(p) => {
                    let mut f = p.clone();
                    f.push(b'\\');
                    f.extend_from_slice(&raw);
                    f
                }
                None => raw,
            };
            let alias = it.alias.map(|a| self.text(a));
            self.import(it.kind.unwrap_or(kind), fqn, alias, it.span);
        }
    }

    // ----- declarations -----------------------------------------------------

    /// The declared FQN of a function/class-like/constant named `name`.
    fn declared_fqn(&self, name: IdentId) -> Vec<u8> {
        self.ns.prefix(self.it.resolve(name))
    }

    fn declare_function(&mut self, f: &FuncDecl) -> Vec<u8> {
        let fqn = self.declared_fqn(f.name);
        let key = fqn.to_ascii_lowercase();
        let unq = self.text(f.name).to_ascii_lowercase();
        if let Some(i) = self.ns.imports.func.get(&unq) {
            if !i.fqn.eq_ignore_ascii_case(&fqn) {
                let s = self.lossy(&fqn);
                self.error(
                    codes::IMPORT_CONFLICT,
                    format!(
                        "Cannot redeclare function {s}() (previously declared as local import)"
                    ),
                    f.span,
                );
            }
        }
        if self.top_level {
            if let Some(prev) = self.seen.top_functions.get(&key).copied() {
                let s = self.lossy(&fqn);
                let file = self.lossy(&self.file);
                let line = self.line(prev);
                self.error(
                    codes::REDECLARED_FUNCTION,
                    format!(
                        "Cannot redeclare function {s}() (previously declared in {file}:{line})"
                    ),
                    f.span,
                );
            }
            self.seen.top_functions.entry(key.clone()).or_insert(f.span);
        }
        self.seen.functions.entry(key).or_insert(f.span);
        fqn
    }

    fn declare_class(&mut self, c: &ClassLike) -> Option<Vec<u8>> {
        let name = c.name?;
        let text = self.text(name);
        let kind_word = match c.kind {
            ClassKind::Class => "a class name",
            ClassKind::Interface => "an interface name",
            ClassKind::Trait => "a trait name",
            ClassKind::Enum => "an enum name",
        };
        if is_reserved_class_name(&text) {
            let s = self.lossy(&text);
            self.error(
                codes::RESERVED_CLASS_NAME,
                format!("Cannot use \"{s}\" as {kind_word} as it is reserved"),
                c.span,
            );
        }
        let fqn = self.declared_fqn(name);
        let key = fqn.to_ascii_lowercase();
        if let Some(i) = self.ns.imports.class.get(&text.to_ascii_lowercase()) {
            if !i.fqn.eq_ignore_ascii_case(&fqn) {
                let s = self.lossy(&fqn);
                self.error(
                    codes::IMPORT_CONFLICT,
                    format!("Cannot redeclare class {s} (previously declared as local import)"),
                    c.span,
                );
            }
        }
        if self.top_level {
            let prev = self
                .seen
                .top_classes
                .get(&key)
                .map(|(s, k, n)| (*s, *k, n.clone()));
            if let Some((prev_span, prev_kind, prev_name)) = prev {
                let file = self.lossy(&self.file);
                let line = self.line(prev_span);
                let n = self.lossy(&prev_name);
                self.error(
                    codes::REDECLARED_CLASS,
                    format!(
                        "Cannot redeclare {} {n} (previously declared in {file}:{line})",
                        prev_kind.as_str()
                    ),
                    c.span,
                );
            }
            self.seen
                .top_classes
                .entry(key.clone())
                .or_insert((c.span, c.kind, fqn.clone()));
        }
        self.seen.classes.entry(key).or_insert(c.span);
        Some(fqn)
    }

    fn declare_consts(&mut self, items: &[rphp_ast::v2::ConstItem]) {
        for it in items {
            let text = self.text(it.name);
            if let Some(i) = self.ns.imports.const_.get(&text) {
                let fqn = self.declared_fqn(it.name);
                if i.fqn != fqn {
                    let s = self.lossy(&fqn);
                    self.error(
                        codes::IMPORT_CONFLICT,
                        format!("Cannot declare const {s} because the name is already in use"),
                        it.span,
                    );
                }
            }
        }
    }

    // ----- magic constants --------------------------------------------------

    fn magic(&mut self, kind: MagicKind, span: Span) -> Option<Expr> {
        if kind == MagicKind::Line {
            return Some(Expr::Int(i64::from(self.line(span)), span));
        }
        match self.magic_ctx().value(kind) {
            Magic::Str(bytes) => {
                let id = self.intern(&bytes);
                Some(Expr::Str(id, span))
            }
            Magic::Dynamic => None,
        }
    }

    /// `self::class` / `parent::class` folded when php folds them.
    fn fold_scope_class(&mut self, c: &ClassRef) -> Option<Vec<u8>> {
        if !self.stack.scope_known() || self.const_depth > 0 {
            return None;
        }
        let cls = self.stack.effective_class()?;
        match c {
            ClassRef::SelfKw(_) => cls.fqn.clone(),
            ClassRef::Parent(_) => cls.parent.clone(),
            _ => None,
        }
    }
}

impl VisitorMut for Resolver<'_> {
    fn visit_stmt(&mut self, s: &mut Stmt) {
        match s {
            Stmt::Namespace { name, body, .. } => {
                let saved = std::mem::take(&mut self.ns);
                self.ns.name = name.as_ref().map(|n| self.text(n.text)).unwrap_or_default();
                self.visit_block(body);
                self.ns = saved;
            }
            Stmt::Use {
                kind,
                prefix,
                items,
                ..
            } => self.use_stmt(*kind, prefix, items),
            Stmt::Func(f) => {
                let fqn = self.declare_function(f);
                // The declaration registers under its FQN (`N\f`): the
                // compiler reads `FuncDecl::name` as the runtime name.
                f.name = self.intern(&fqn);
                self.stack.push_fn(FnKind::Function { fqn });
                self.nested(|r| walk_func_decl(r, f));
                self.stack.pop();
            }
            Stmt::ClassLike(c) => self.visit_class_like(c),
            Stmt::ConstDecl { attrs, items, .. } => {
                self.declare_consts(items);
                // `const X = 1;` in namespace `N` defines `N\X`.
                if !self.ns.name.is_empty() {
                    for it in items.iter_mut() {
                        let fqn = self.declared_fqn(it.name);
                        it.name = self.intern(&fqn);
                    }
                }
                self.in_const(|r| {
                    for g in attrs {
                        r.visit_attr_group(g);
                    }
                    for it in items {
                        r.visit_const_item(it);
                    }
                });
            }
            Stmt::Try {
                body,
                catches,
                finally,
                ..
            } => {
                self.nested(|r| r.visit_block(body));
                for c in catches {
                    for t in &mut c.types {
                        let text = self.text(t.text);
                        if t.kind == NameKind::Unqualified && is_scope_keyword(&text) {
                            self.error(
                                codes::RESERVED_CLASS_NAME,
                                "Bad class name in the catch statement",
                                t.span,
                            );
                        } else {
                            self.resolve_class(t);
                        }
                    }
                    self.nested(|r| r.visit_block(&mut c.body));
                }
                if let Some(f) = finally {
                    self.nested(|r| r.visit_block(f));
                }
            }
            Stmt::Block { .. } => walk_stmt(self, s),
            Stmt::StaticVar { vars, .. } => {
                for v in vars {
                    self.visit_static_var(v);
                }
            }
            _ => self.nested(|r| walk_stmt(r, s)),
        }
    }

    fn visit_expr(&mut self, e: &mut Expr) {
        match e {
            Expr::Const(n) => {
                if let Some(folded) = self.resolve_const(n) {
                    *e = folded;
                }
            }
            Expr::MagicConst { kind, span } => {
                if let Some(v) = self.magic(*kind, *span) {
                    *e = v;
                }
            }
            Expr::ClassConst {
                class,
                name: ConstSel::Class(_),
                span,
            } => {
                let folded: Option<Vec<u8>> = match class {
                    ClassRef::Named(n) => {
                        if self.check_reserved_reference(n, "class name") {
                            None
                        } else {
                            Some(self.resolve_class(n))
                        }
                    }
                    ClassRef::SelfKw(_) | ClassRef::Parent(_) => {
                        self.check_scope_keyword(class);
                        self.fold_scope_class(class)
                    }
                    ClassRef::Static(_) => {
                        self.check_scope_keyword(class);
                        None
                    }
                    ClassRef::Expr(inner) => {
                        self.visit_expr(inner);
                        None
                    }
                };
                if let Some(fqn) = folded {
                    let id = self.intern(&fqn);
                    *e = Expr::Str(id, *span);
                }
            }
            Expr::Closure(c) => {
                let line = self.line(c.span);
                self.stack.push_fn(FnKind::Closure { line });
                self.visit_closure(c);
                self.stack.pop();
            }
            Expr::ArrowFn(f) => {
                let line = self.line(f.span);
                self.stack.push_fn(FnKind::Closure { line });
                self.visit_arrow_fn(f);
                self.stack.pop();
            }
            Expr::New {
                class: NewTarget::Anon(c),
                args,
                ..
            } => {
                for a in args {
                    self.visit_arg(a);
                }
                self.visit_class_like(c);
            }
            _ => walk_expr(self, e),
        }
    }

    fn visit_class_ref(&mut self, c: &mut ClassRef) {
        match c {
            ClassRef::Named(n) => {
                if !self.check_reserved_reference(n, "class name") {
                    self.resolve_class(n);
                }
            }
            ClassRef::SelfKw(_) | ClassRef::Static(_) | ClassRef::Parent(_) => {
                self.check_scope_keyword(c);
            }
            ClassRef::Expr(_) => walk_class_ref(self, c),
        }
    }

    fn visit_callee(&mut self, c: &mut Callee) {
        match c {
            Callee::Name(n) => self.resolve_func(n),
            Callee::Expr(e) => self.visit_expr(e),
        }
    }

    fn visit_name(&mut self, n: &mut Name) {
        // Every remaining `Name` position is a class reference (attributes,
        // type hints, trait uses and adaptations).
        if !self.check_reserved_reference(n, "class name") {
            self.resolve_class(n);
        }
    }

    fn visit_type(&mut self, t: &mut Type) {
        if let TypeKind::Named(n) = &mut t.kind {
            self.resolve_class(n);
        } else {
            walk_type(self, t);
        }
    }

    fn visit_arg(&mut self, a: &mut Arg) {
        walk_arg(self, a);
    }

    fn visit_attr_group(&mut self, g: &mut rphp_ast::v2::AttrGroup) {
        self.in_const(|r| {
            for a in &mut g.attrs {
                if !r.check_reserved_reference(&a.name, "class name") {
                    r.resolve_class(&mut a.name);
                }
                for arg in &mut a.args {
                    r.visit_arg(arg);
                }
            }
        });
    }

    fn visit_class_like(&mut self, c: &mut ClassLike) {
        let fqn = self.declare_class(c);
        if let Some(f) = &fqn {
            // As for functions: the declared name is the FQN (`N\C`).
            c.name = Some(self.intern(f));
        }
        for g in &mut c.attrs {
            self.visit_attr_group(g);
        }
        let mut parent = None;
        for (i, n) in c.extends.iter_mut().enumerate() {
            let what = match c.kind {
                ClassKind::Interface => "interface name",
                _ => "class name",
            };
            if !self.check_reserved_reference(n, what) {
                let p = self.resolve_class(n);
                if i == 0 && c.kind == ClassKind::Class {
                    parent = Some(p);
                }
            }
        }
        for n in &mut c.implements {
            if !self.check_reserved_reference(n, "interface name") {
                self.resolve_class(n);
            }
        }
        if let Some(b) = &mut c.backing {
            self.visit_type(b);
        }
        self.stack.push_class(ClassScope {
            kind: c.kind,
            fqn,
            parent,
        });
        self.nested(|r| {
            for m in &mut c.members {
                r.visit_member(m);
            }
        });
        self.stack.pop();
    }

    fn visit_member(&mut self, m: &mut Member) {
        match m {
            Member::Const(cm) => {
                for g in &mut cm.attrs {
                    self.visit_attr_group(g);
                }
                if let Some(t) = &mut cm.ty {
                    self.visit_type(t);
                }
                self.in_const(|r| {
                    for it in &mut cm.items {
                        r.visit_const_item(it);
                    }
                });
            }
            Member::Prop(p) => self.visit_prop_member(p),
            Member::Method(md) => self.visit_method_decl(md),
            Member::EnumCase(ec) => {
                for g in &mut ec.attrs {
                    self.visit_attr_group(g);
                }
                if let Some(v) = &mut ec.value {
                    self.in_const(|r| r.visit_expr(v));
                }
            }
            Member::TraitUse(t) => {
                for n in &mut t.traits {
                    if !self.check_reserved_reference(n, "trait name") {
                        self.resolve_class(n);
                    }
                }
                for a in &mut t.adaptations {
                    match a {
                        Adaptation::Precedence {
                            trait_, insteadof, ..
                        } => {
                            if !self.check_reserved_reference(trait_, "trait name") {
                                self.resolve_class(trait_);
                            }
                            for n in insteadof {
                                if !self.check_reserved_reference(n, "trait name") {
                                    self.resolve_class(n);
                                }
                            }
                        }
                        Adaptation::Alias {
                            trait_: Some(t), ..
                        } => {
                            if !self.check_reserved_reference(t, "trait name") {
                                self.resolve_class(t);
                            }
                        }
                        Adaptation::Alias { trait_: None, .. } => {}
                    }
                }
            }
        }
    }

    fn visit_prop_member(&mut self, p: &mut PropMember) {
        let saved = self.cur_prop;
        self.cur_prop = p.items.first().map(|i| i.name);
        for g in &mut p.attrs {
            self.visit_attr_group(g);
        }
        if let Some(t) = &mut p.ty {
            self.visit_type(t);
        }
        for it in &mut p.items {
            if let Some(d) = &mut it.default {
                self.in_const(|r| r.visit_expr(d));
            }
        }
        for h in &mut p.hooks {
            self.visit_hook(h);
        }
        self.cur_prop = saved;
    }

    fn visit_method_decl(&mut self, m: &mut MethodDecl) {
        let name = self.text(m.name);
        self.stack.push_fn(FnKind::Method { name });
        walk_method_decl(self, m);
        self.stack.pop();
    }

    fn visit_param(&mut self, p: &mut Param) {
        for g in &mut p.attrs {
            self.visit_attr_group(g);
        }
        if let Some(t) = &mut p.ty {
            self.visit_type(t);
        }
        if let Some(d) = &mut p.default {
            self.in_const(|r| r.visit_expr(d));
        }
        if !p.hooks.is_empty() {
            let saved = self.cur_prop;
            self.cur_prop = Some(p.name);
            for h in &mut p.hooks {
                self.visit_hook(h);
            }
            self.cur_prop = saved;
        }
    }

    fn visit_hook(&mut self, h: &mut Hook) {
        let prop = self.cur_prop.map(|p| self.text(p)).unwrap_or_default();
        let kind: HookKind = h.kind;
        self.stack.push_fn(FnKind::Hook { prop, kind });
        walk_hook(self, h);
        self.stack.pop();
    }

    fn visit_func_decl(&mut self, f: &mut FuncDecl) {
        // Only reached through `Stmt::Func`, which pushes the scope itself.
        walk_func_decl(self, f);
    }
}
