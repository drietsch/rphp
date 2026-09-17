//! The lexical state the resolver threads through the tree: the current
//! namespace with its three import tables, the function-like scope stack
//! (for magic constants and `self`/`static`/`parent`), the class scope
//! stack, and the per-file "seen symbols" set php uses for import conflicts.
//!
//! Everything here works on raw bytes: PHP names are byte strings, and the
//! tables key on the *lookup key* (lowercased for classes and functions,
//! case-preserved for constants), exactly like `FC(imports)`,
//! `FC(imports_function)` and `FC(imports_const)` in Zend.

use std::collections::HashMap;

use rphp_ast::v2::{ClassKind, HookKind, UseKind};
use rphp_span::Span;

/// One import: the fully qualified target and where it was written.
#[derive(Clone, Debug)]
pub struct Import {
    /// The imported name without a leading `\`, original case.
    pub fqn: Vec<u8>,
    /// Span of the `use` item.
    pub span: Span,
}

/// The three per-namespace import tables.
#[derive(Clone, Debug, Default)]
pub struct ImportTables {
    /// `use A\B [as C]` — keyed by the lowercased alias.
    pub class: HashMap<Vec<u8>, Import>,
    /// `use function a\b [as c]` — keyed by the lowercased alias.
    pub func: HashMap<Vec<u8>, Import>,
    /// `use const A\B [as C]` — keyed by the alias as written.
    pub const_: HashMap<Vec<u8>, Import>,
}

impl ImportTables {
    /// The table for a kind.
    pub fn table(&self, kind: UseKind) -> &HashMap<Vec<u8>, Import> {
        match kind {
            UseKind::Class => &self.class,
            UseKind::Function => &self.func,
            UseKind::Const => &self.const_,
        }
    }

    /// The table for a kind, mutably.
    pub fn table_mut(&mut self, kind: UseKind) -> &mut HashMap<Vec<u8>, Import> {
        match kind {
            UseKind::Class => &mut self.class,
            UseKind::Function => &mut self.func,
            UseKind::Const => &mut self.const_,
        }
    }
}

/// The lookup key of an alias for a kind (php's `lookup_name`).
pub fn alias_key(kind: UseKind, alias: &[u8]) -> Vec<u8> {
    match kind {
        UseKind::Class | UseKind::Function => alias.to_ascii_lowercase(),
        UseKind::Const => alias.to_vec(),
    }
}

/// The current namespace: its name (empty for the global namespace) and its
/// imports. A new one is pushed for every `namespace` statement; the import
/// tables never leak across namespace blocks.
#[derive(Clone, Debug, Default)]
pub struct NamespaceScope {
    /// The namespace name without leading/trailing `\`; empty at global scope.
    pub name: Vec<u8>,
    /// The `use` imports seen so far in this block.
    pub imports: ImportTables,
}

impl NamespaceScope {
    /// `ns\name`, or `name` at global scope (php's `zend_prefix_with_ns`).
    pub fn prefix(&self, name: &[u8]) -> Vec<u8> {
        if self.name.is_empty() {
            return name.to_vec();
        }
        let mut out = Vec::with_capacity(self.name.len() + 1 + name.len());
        out.extend_from_slice(&self.name);
        out.push(b'\\');
        out.extend_from_slice(name);
        out
    }
}

/// What kind of function-like body the resolver is inside.
#[derive(Clone, Debug)]
pub enum FnKind {
    /// The file's pseudo-main (also the body of an `eval` string).
    Main,
    /// A named function; `fqn` is the namespaced name (`Foo\f`).
    Function {
        /// The namespaced function name, original case.
        fqn: Vec<u8>,
    },
    /// A method; `name` as written.
    Method {
        /// The method name as written.
        name: Vec<u8>,
    },
    /// `function (...) {}` or `fn (...) =>`; `line` of the keyword, for the
    /// 8.4 `{closure:...:<line>}` naming.
    Closure {
        /// The 1-based line of the closure.
        line: u32,
    },
    /// A property hook body; `prop` is the property name without `$`.
    Hook {
        /// The property name without `$`.
        prop: Vec<u8>,
        /// Which hook.
        kind: HookKind,
    },
}

/// One function-like entry of the [`ScopeStack`].
#[derive(Clone, Debug)]
pub struct FnScope {
    /// What kind of body.
    pub kind: FnKind,
}

/// One entry of the class scope stack.
#[derive(Clone, Debug)]
pub struct ClassScope {
    /// Which kind of class-like.
    pub kind: ClassKind,
    /// The fully qualified name; `None` for an anonymous class (php names it
    /// `class@anonymous<NUL><file>:<line>$<n>` with a compiler counter, so
    /// every magic constant embedding it stays dynamic).
    pub fqn: Option<Vec<u8>>,
    /// The resolved parent FQN, if `extends` was written (classes only).
    pub parent: Option<Vec<u8>>,
}

impl ClassScope {
    /// `true` for traits: `self`/`__CLASS__` bind to the using class.
    pub fn is_trait(&self) -> bool {
        self.kind == ClassKind::Trait
    }
}

/// One entry of the combined lexical stack: function-likes and class bodies
/// interleave (a class inside a function, a method inside a class, a
/// closure inside a method, ...), and php's scoping rules read the stack
/// from the top: a class body sets the active class, a *named* function
/// clears it (`__CLASS__` is `""` in a free function even when it is
/// declared inside a method), closures and hooks keep it.
#[derive(Clone, Debug)]
pub enum Entry {
    /// A function-like body.
    Fn(FnScope),
    /// A class-like body.
    Class(ClassScope),
}

/// The combined lexical stack, see [`Entry`]. The bottom entry is always
/// the pseudo-main.
#[derive(Clone, Debug)]
pub struct ScopeStack {
    /// The entries, innermost last.
    pub entries: Vec<Entry>,
}

impl Default for ScopeStack {
    fn default() -> Self {
        Self {
            entries: vec![Entry::Fn(FnScope { kind: FnKind::Main })],
        }
    }
}

impl ScopeStack {
    /// Push a function-like body.
    pub fn push_fn(&mut self, kind: FnKind) {
        self.entries.push(Entry::Fn(FnScope { kind }));
    }

    /// Push a class-like body.
    pub fn push_class(&mut self, c: ClassScope) {
        self.entries.push(Entry::Class(c));
    }

    /// Pop the innermost entry (never the pseudo-main).
    pub fn pop(&mut self) {
        debug_assert!(self.entries.len() > 1, "cannot pop the pseudo-main scope");
        if self.entries.len() > 1 {
            self.entries.pop();
        }
    }

    /// Index and scope of the innermost function-like.
    pub fn innermost_fn(&self) -> (usize, &FnScope) {
        for (i, e) in self.entries.iter().enumerate().rev() {
            if let Entry::Fn(f) = e {
                return (i, f);
            }
        }
        unreachable!("the pseudo-main scope is never popped")
    }

    /// The function-like immediately enclosing the entry at `idx`, if any.
    pub fn fn_below(&self, idx: usize) -> Option<(usize, &FnScope)> {
        self.entries[..idx]
            .iter()
            .enumerate()
            .rev()
            .find_map(|(i, e)| match e {
                Entry::Fn(f) => Some((i, f)),
                Entry::Class(_) => None,
            })
    }

    /// php's `CG(active_class_entry)` seen from the top of the stack: the
    /// innermost class body unless a named function was entered after it.
    pub fn effective_class(&self) -> Option<&ClassScope> {
        self.effective_class_from(self.entries.len())
    }

    /// [`effective_class`](Self::effective_class) as seen from just below
    /// the entry at `idx`.
    pub fn effective_class_from(&self, idx: usize) -> Option<&ClassScope> {
        for e in self.entries[..idx].iter().rev() {
            match e {
                Entry::Class(c) => return Some(c),
                Entry::Fn(f) => {
                    if matches!(f.kind, FnKind::Function { .. }) {
                        return None;
                    }
                }
            }
        }
        None
    }

    /// `true` when the top entry is a class body (a constant, property
    /// default or enum case is being resolved rather than a method body).
    pub fn directly_in_class_body(&self) -> bool {
        matches!(self.entries.last(), Some(Entry::Class(_)))
    }

    /// Zend's `zend_is_scope_known()`: whether `self`/`static`/`parent` can
    /// be checked at compile time here. Closures may be rebound, the
    /// pseudo-main inherits the including scope, traits bind to the using
    /// class; a free function and a non-trait class body are known.
    pub fn scope_known(&self) -> bool {
        match self.entries.last() {
            Some(Entry::Class(c)) => !c.is_trait(),
            Some(Entry::Fn(f)) => match f.kind {
                FnKind::Closure { .. } | FnKind::Main => false,
                FnKind::Function { .. } => true,
                FnKind::Method { .. } | FnKind::Hook { .. } => {
                    self.effective_class().is_some_and(|c| !c.is_trait())
                }
            },
            None => false,
        }
    }
}

/// Symbols declared so far in this file, per kind, keyed like the runtime
/// tables (php's `FC(seen_symbols)`): drives the "name is already in use"
/// import checks and the duplicate top-level declaration diagnostics.
#[derive(Clone, Debug, Default)]
pub struct SeenSymbols {
    /// Lowercased class FQN → span of the first declaration (conditional
    /// or not).
    pub classes: HashMap<Vec<u8>, Span>,
    /// Lowercased function FQN → span.
    pub functions: HashMap<Vec<u8>, Span>,
    /// Constant key (lowercased namespace, case-preserved name) → span.
    pub consts: HashMap<Vec<u8>, Span>,
    /// Lowercased FQN → span of functions declared in top-level position
    /// (php's compile-time function table; conditional declarations are
    /// bound at run time and never clash at compile time).
    pub top_functions: HashMap<Vec<u8>, Span>,
    /// Lowercased FQN → (span, kind, spelling) of class-likes declared in
    /// top-level position.
    pub top_classes: HashMap<Vec<u8>, (Span, ClassKind, Vec<u8>)>,
}

/// Lowercase the namespace part of a constant name and keep the last
/// segment as written: the constant-table key.
pub fn const_key(fqn: &[u8]) -> Vec<u8> {
    match fqn.iter().rposition(|&b| b == b'\\') {
        Some(i) => {
            let mut out = fqn[..i].to_ascii_lowercase();
            out.push(b'\\');
            out.extend_from_slice(&fqn[i + 1..]);
            out
        }
        None => fqn.to_vec(),
    }
}

/// The last `\`-separated segment of a name.
pub fn last_segment(name: &[u8]) -> &[u8] {
    match name.iter().rposition(|&b| b == b'\\') {
        Some(i) => &name[i + 1..],
        None => name,
    }
}

/// Split a qualified name at its first `\`.
pub fn split_first(name: &[u8]) -> Option<(&[u8], &[u8])> {
    let i = name.iter().position(|&b| b == b'\\')?;
    Some((&name[..i], &name[i + 1..]))
}

/// php's `zend_is_reserved_class_name`: names that can never be a class.
pub fn is_reserved_class_name(name: &[u8]) -> bool {
    const RESERVED: &[&[u8]] = &[
        b"bool",
        b"false",
        b"float",
        b"int",
        b"null",
        b"parent",
        b"self",
        b"static",
        b"string",
        b"true",
        b"void",
        b"never",
        b"iterable",
        b"object",
        b"mixed",
    ];
    RESERVED.iter().any(|r| name.eq_ignore_ascii_case(r))
}

/// `true` for `self`, `static` and `parent` (case-insensitive).
pub fn is_scope_keyword(name: &[u8]) -> bool {
    name.eq_ignore_ascii_case(b"self")
        || name.eq_ignore_ascii_case(b"static")
        || name.eq_ignore_ascii_case(b"parent")
}

/// `true` for php's auto-globals, which arrow functions never capture and
/// closures cannot `use`.
pub fn is_auto_global(name: &[u8]) -> bool {
    matches!(
        name,
        b"GLOBALS"
            | b"_SERVER"
            | b"_GET"
            | b"_POST"
            | b"_COOKIE"
            | b"_FILES"
            | b"_ENV"
            | b"_REQUEST"
            | b"_SESSION"
    )
}
