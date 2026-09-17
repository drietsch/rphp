//! The per-file conversion context: interner, diagnostics, span mapping,
//! identifier-span side table and the scope stacks the conformance checks
//! need (function nesting, loop depth, class scope).

use mago_syntax::cst::Trivia;
use rphp_ast::v2::lvalue::SpecialVars;
use rphp_ast::v2::ClassKind;
use rphp_diagnostics::{codes, Diagnostic};
use rphp_intern::{IdentId, Interner};
use rphp_span::{FileId, Span};

/// What kind of function-like body is being converted.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum FnKind {
    /// The file's top-level pseudo-main (also `eval` bodies).
    Main,
    /// A named function declaration (nested or not). Entering one clears the
    /// class scope, as Zend does for free-standing functions.
    Function,
    /// A class-like method.
    Method,
    /// A property hook body.
    Hook,
    /// `function (...) { }` — may be rebound, so the class scope is unknown.
    Closure,
    /// `fn (...) => ...` — same scoping as a closure.
    ArrowFn,
}

/// The declared return type of a function body, as far as the conformance
/// checks care.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum RetKind {
    /// No return type declared.
    None,
    /// `: void`
    Void,
    /// `: never`
    Never,
    /// Any other type (a `return;` without a value is an error unless the
    /// function is a generator); `nullable` when the type admits `null`
    /// (PHP adds a "did you mean return null;" hint).
    Typed {
        /// The type admits `null`.
        nullable: bool,
    },
}

/// What a `return` statement returns, as far as the messages care.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ReturnValue {
    /// `return;`
    None,
    /// `return null;` (the literal).
    Null,
    /// Any other value.
    Value,
}

/// One entry of the function scope stack.
#[derive(Clone, Debug)]
pub(crate) struct FnScope {
    /// What kind of body.
    pub kind: FnKind,
    /// Number of enclosing loops/switches inside this body.
    pub loop_depth: u32,
    /// The declared return type kind.
    pub ret: RetKind,
    /// A `yield` was seen directly in this body (generator).
    pub has_yield: bool,
    /// Every `return` seen directly in this body: span and what it returns.
    /// Checked against `ret` when the body is complete.
    pub returns: Vec<(Span, ReturnValue)>,
    /// Display name for messages (`C::f` / `f`), if any.
    #[allow(dead_code)]
    pub display: Option<String>,
    /// `function &f()`: `return $a[]` fetches for writing.
    pub by_ref: bool,
    /// For a hook body: the property name (without `$`) and which hook.
    pub hook: Option<(Vec<u8>, rphp_ast::v2::HookKind)>,
}

impl FnScope {
    /// A fresh scope of the given kind.
    pub fn new(kind: FnKind, ret: RetKind, display: Option<String>) -> Self {
        Self {
            kind,
            loop_depth: 0,
            ret,
            has_yield: false,
            returns: Vec::new(),
            display,
            by_ref: false,
            hook: None,
        }
    }
}

/// One entry of the class scope stack.
#[derive(Clone, Debug)]
pub(crate) struct ClassScope {
    /// Which kind of class-like.
    pub kind: ClassKind,
    /// `extends` was written (classes) — `parent::` is meaningful.
    pub has_parent: bool,
    /// The declared name for messages; `None` for anonymous classes.
    #[allow(dead_code)]
    pub name: Option<String>,
    /// `abstract class`.
    #[allow(dead_code)]
    pub is_abstract: bool,
    /// `readonly class`: every property is implicitly readonly.
    pub readonly: bool,
}

/// The conversion context for one file.
pub(crate) struct Ctx<'a, 'arena> {
    /// The interner every name, variable and string literal goes through.
    pub interner: &'a mut Interner,
    /// The file id stamped on every span.
    pub file: FileId,
    /// The source bytes (for heredoc indentation checks and inline HTML).
    pub src: &'arena [u8],
    /// mago's trivia sequence, for docblock lookup.
    pub trivia: &'arena [Trivia<'arena>],
    /// Collected diagnostics, in emission order.
    pub diags: Vec<Diagnostic>,
    /// Byte spans of identifiers PHP's `TOKEN_PARSE` reclassifies to `T_STRING`.
    pub ident_spans: Vec<(u32, u32)>,
    /// `$this` / `$GLOBALS` ids for the lvalue validator.
    pub sv: SpecialVars,
    /// Set when `__halt_compiler();` was converted.
    pub halt_offset: Option<u32>,
    /// Set when a valid `declare(strict_types=1)` was converted.
    pub strict_types: bool,
    /// Function scope stack; the bottom entry is the pseudo-main.
    pub fns: Vec<FnScope>,
    /// Class scope stack (anonymous classes push too).
    pub classes: Vec<ClassScope>,
    /// End offset of the most recent `?>` (to swallow the newline PHP drops
    /// from the inline HTML that follows it).
    pub last_close_tag_end: Option<u32>,
    /// A braced `namespace { }` has been seen (mixing rules).
    pub seen_braced_namespace: bool,
    /// An unbraced `namespace X;` has been seen (mixing rules).
    pub seen_unbraced_namespace: bool,
    /// Set by the expression-statement conversion for its top expression:
    /// the only position where `(void)` is legal.
    pub void_cast_ok: bool,
    /// `__halt_compiler();` was converted: nothing after it is code.
    pub halted: bool,
    /// A top-level statement that is neither a `declare` nor empty has been
    /// converted (namespace / strict_types ordering rules).
    pub seen_top_code: bool,
    /// The enum being converted has a backing type (case value rules).
    pub enum_backed: bool,
    /// Depth of constant-expression positions (defaults, constants,
    /// attribute arguments): `self`/`parent` are not scope-checked there.
    pub const_depth: u32,
}

impl<'a, 'arena> Ctx<'a, 'arena> {
    /// A context with the pseudo-main function scope pushed.
    pub fn new(
        interner: &'a mut Interner,
        file: FileId,
        src: &'arena [u8],
        trivia: &'arena [Trivia<'arena>],
    ) -> Self {
        let sv = SpecialVars::intern(interner);
        Self {
            interner,
            file,
            src,
            trivia,
            diags: Vec::new(),
            ident_spans: Vec::new(),
            sv,
            halt_offset: None,
            strict_types: false,
            fns: vec![FnScope::new(FnKind::Main, RetKind::None, None)],
            classes: Vec::new(),
            last_close_tag_end: None,
            seen_braced_namespace: false,
            seen_unbraced_namespace: false,
            void_cast_ok: false,
            halted: false,
            seen_top_code: false,
            enum_backed: false,
            const_depth: 0,
        }
    }

    /// Map a mago span to an `rphp_span::Span` (offsets are 1:1).
    #[inline]
    pub fn sp(&self, s: mago_span::Span) -> Span {
        Span::new(self.file, s.start.offset, s.end.offset)
    }

    /// A span from two byte offsets.
    #[inline]
    pub fn span(&self, lo: u32, hi: u32) -> Span {
        Span::new(self.file, lo, hi)
    }

    /// Intern raw bytes.
    #[inline]
    pub fn intern(&mut self, bytes: &[u8]) -> IdentId {
        self.interner.intern(bytes)
    }

    /// Intern a variable token (`$name` → `name`).
    pub fn intern_var(&mut self, token: &[u8]) -> IdentId {
        let name = token.strip_prefix(b"$").unwrap_or(token);
        self.interner.intern(name)
    }

    /// Record an error diagnostic with a primary span.
    pub fn error(&mut self, code: &'static str, message: impl Into<String>, span: Span) {
        self.diags
            .push(Diagnostic::error(code, message).with_primary(span, ""));
    }

    /// `RPHP_E0011`: mago accepted it, PHP 8.5 does not. `message` is PHP's.
    pub fn reject(&mut self, message: impl Into<String>, span: Span) {
        self.error(codes::SUPERSET_REJECTED, message, span);
    }

    /// `RPHP_E0006`: a literal PHP rejects at parse time.
    pub fn invalid_literal(&mut self, message: impl Into<String>, span: Span) {
        self.error(codes::INVALID_LITERAL, message, span);
    }

    /// `RPHP_E0010`: a node kind without a mapping.
    pub fn unsupported(&mut self, kind: mago_syntax::cst::NodeKind, span: Span) {
        self.error(
            codes::UNSUPPORTED_NODE,
            format!("unsupported syntax node: {kind}"),
            span,
        );
    }

    /// Record an identifier span for the tokenizer's `TOKEN_PARSE` mode.
    #[inline]
    pub fn ident_span(&mut self, s: mago_span::Span) {
        self.ident_spans.push((s.start.offset, s.end.offset));
    }

    /// The innermost function scope.
    pub fn fn_scope(&mut self) -> &mut FnScope {
        self.fns
            .last_mut()
            .expect("the pseudo-main scope is never popped")
    }

    /// The innermost function scope's kind.
    pub fn fn_kind(&self) -> FnKind {
        self.fns.last().map(|f| f.kind).unwrap_or(FnKind::Main)
    }

    /// Zend's `zend_is_scope_known()`: whether `self`/`static`/`parent` can be
    /// checked at compile time in the current position.
    pub fn scope_known(&self) -> bool {
        if self.const_depth > 0 {
            return false;
        }
        match self.fn_kind() {
            FnKind::Closure | FnKind::ArrowFn | FnKind::Main => false,
            FnKind::Function => true,
            FnKind::Method | FnKind::Hook => self
                .effective_class()
                .is_some_and(|c| c.kind != ClassKind::Trait),
        }
    }

    /// The class scope that applies to the current position: closures look
    /// through to their enclosing method, a named function clears it.
    pub fn effective_class(&self) -> Option<&ClassScope> {
        for f in self.fns.iter().rev() {
            match f.kind {
                FnKind::Closure | FnKind::ArrowFn => continue,
                FnKind::Function | FnKind::Main => return None,
                FnKind::Method | FnKind::Hook => return self.classes.last(),
            }
        }
        None
    }

    /// Push a function scope; the matching `pop_fn` returns it for checks.
    pub fn push_fn(&mut self, kind: FnKind, ret: RetKind, display: Option<String>) {
        self.fns.push(FnScope::new(kind, ret, display));
    }

    /// Pop the innermost function scope.
    pub fn pop_fn(&mut self) -> FnScope {
        debug_assert!(self.fns.len() > 1, "cannot pop the pseudo-main scope");
        self.fns.pop().unwrap_or_else(|| FnScope::new(FnKind::Main, RetKind::None, None))
    }

    /// Run `f` with the loop depth of the innermost function incremented.
    pub fn in_loop<T>(&mut self, f: impl FnOnce(&mut Self) -> T) -> T {
        self.fn_scope().loop_depth += 1;
        let r = f(self);
        self.fn_scope().loop_depth -= 1;
        r
    }
}

impl Ctx<'_, '_> {
    /// Drop the conformance / validation diagnostics recorded since `mark`
    /// (parse errors and unsupported-node reports stay). Used for code PHP
    /// never compiles (`false && ...`).
    pub fn drop_conformance_since(&mut self, mark: usize) {
        let mut i = mark;
        while i < self.diags.len() {
            let code = self.diags[i].code;
            if code == codes::SUPERSET_REJECTED || code.starts_with("RPHP_E002") {
                self.diags.remove(i);
            } else {
                i += 1;
            }
        }
    }
}

/// Case-insensitive ASCII comparison of a token with a keyword.
#[inline]
pub(crate) fn eq_ci(bytes: &[u8], kw: &str) -> bool {
    bytes.eq_ignore_ascii_case(kw.as_bytes())
}
