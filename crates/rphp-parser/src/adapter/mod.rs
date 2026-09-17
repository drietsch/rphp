//! The `mago-syntax` → AST v2 adapter (roadmap F2, ADR-025).
//!
//! One [`LocalArena`] per file, `parse_file_content_with_settings`, then a
//! single pass over mago's CST that builds [`rphp_ast::v2::Program`]:
//! spans map 1:1, every identifier/variable/string is interned, mago's
//! parse errors become `RPHP_E0001..E0005`, and the conformance rules
//! (`conformance.rs`) reject the syntax mago accepts but PHP 8.5.0 does not.
//!
//! | module         | contents                                                      |
//! |----------------|---------------------------------------------------------------|
//! | `ctx`          | per-file context: interner, diagnostics, spans, scope stacks    |
//! | `errors`       | mago `ParseError` → `Diagnostic`                                |
//! | `names`        | names, class references, member names, constant selectors      |
//! | `literals`     | integer / float literal decoding (overflow → float)             |
//! | `strings`      | string literals, heredoc/nowdoc, interpolation                  |
//! | `expr`         | expressions                                                     |
//! | `stmt`         | statements, namespaces, `declare`, `__halt_compiler`, inline    |
//! | `class`        | class-likes, members, modifiers, trait adaptations              |
//! | `func`         | functions, closures, arrow functions, parameters, hooks         |
//! | `types`        | type hints and PHP's type-declaration rules                     |
//! | `attrs`        | attribute groups                                                |
//! | `docs`         | doc-comment attachment                                          |
//! | `conformance`  | the superset rejections (`RPHP_E0011`) and lvalue validation    |
//!
//! [`COVERED`] is the contract with mago's `NodeKind` enum: every kind is
//! either mapped, explicitly rejected, or a recovery node. The test in
//! `tests/node_kinds.rs` checks the table against the pinned crate so a mago
//! upgrade that adds a kind fails loudly instead of silently producing
//! `RPHP_E0010`.

use std::path::Path;

use mago_allocator::LocalArena;
use mago_database::file::FileId as MagoFileId;
use mago_syntax::parser::parse_file_content_with_settings;
use mago_syntax::settings::{LexerSettings, ParserSettings};
use rphp_ast::v2::Program;
use rphp_diagnostics::Diagnostic;
use rphp_intern::Interner;
use rphp_span::FileId;

mod attrs;
mod class;
mod conformance;
mod ctx;
mod docs;
mod errors;
mod expr;
mod func;
mod literals;
mod names;
mod stmt;
mod strings;
mod types;

/// The exact `mago-syntax` version this adapter was written against (ADR-025
/// pins it in the workspace manifest; bumps go through the ADR-028 procedure).
pub const MAGO_SYNTAX_VERSION: &str = "1.49.0";

/// Options for [`parse_v2`].
#[derive(Clone, Copy, Debug)]
pub struct ParseOptions<'a> {
    /// The file id stamped on every span and on the program.
    pub file: FileId,
    /// The file's path, for diagnostics (mago derives its own file id from
    /// it; the adapter never reads it back).
    pub path: Option<&'a Path>,
    /// Honour `<?` as an opening tag (`short_open_tag` ini; PHP's default is
    /// on).
    pub short_open_tag: bool,
}

impl<'a> ParseOptions<'a> {
    /// Options for `file` with `short_open_tag` on and no path.
    pub fn new(file: FileId) -> Self {
        Self {
            file,
            path: None,
            short_open_tag: true,
        }
    }
}

/// The result of [`parse_v2`].
#[derive(Clone, Debug)]
pub struct Parsed {
    /// The tree; partial when `diagnostics` holds errors.
    pub program: Program,
    /// Parse errors (`RPHP_E0001..E0006`), unsupported nodes (`E0010`),
    /// conformance rejections (`E0011`) and write-context/strict_types
    /// validation (`E0020..E0025`), sorted by position.
    pub diagnostics: Vec<Diagnostic>,
    /// Byte spans `(lo, hi)` of identifiers that PHP's `TOKEN_PARSE` mode
    /// reclassifies to `T_STRING` when they spell a keyword: member names
    /// after `->`/`?->`/`::`, named-argument labels, method / class-constant
    /// / enum-case names in declarations, `goto` labels and label statements,
    /// trait-adaptation method and alias names. Every identifier in those
    /// positions is recorded, keyword or not; the tokenizer intersects the
    /// list with its own keyword tokens. Best effort: positions PHP's parser
    /// reclassifies through other productions (`use ... as`, `namespace`
    /// segments) are not recorded.
    pub ident_spans: Vec<(u32, u32)>,
}

/// Parse `src` with mago and convert it to AST v2.
///
/// Never panics on any input; the returned program is complete where the
/// source was valid and holds `Expr::Error` / `Stmt::Nop` placeholders where
/// mago recovered.
pub fn parse_v2(src: &[u8], opts: ParseOptions<'_>, interner: &mut Interner) -> Parsed {
    let arena = LocalArena::new();
    let settings = ParserSettings {
        lexer: LexerSettings {
            enable_short_tags: opts.short_open_tag,
        },
    };
    let mago_file = match opts.path {
        Some(p) => MagoFileId::new(p.as_os_str().as_encoded_bytes()),
        None => MagoFileId::zero(),
    };
    let program = parse_file_content_with_settings(&arena, mago_file, src, settings);
    let mut diagnostics: Vec<Diagnostic> = program
        .errors
        .iter()
        .map(|e| errors::diagnostic(e, opts.file))
        .collect();
    let mut cx = ctx::Ctx::new(interner, opts.file, program.source_text, program.trivia.as_slice());
    let items = stmt::program(&mut cx, program);
    let strict_types = cx.strict_types;
    let halt_offset = cx.halt_offset;
    let ident_spans = cx.ident_spans;
    diagnostics.extend(cx.diags);
    diagnostics.sort_by_key(|d| d.primary.as_ref().map_or((0, 0), |l| (l.span.lo, l.span.hi)));
    Parsed {
        program: Program {
            file: opts.file,
            strict_types,
            items,
            halt_offset,
        },
        diagnostics,
        ident_spans,
    }
}

/// How the adapter treats a mago node kind.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Coverage {
    /// Converted to AST v2 (possibly with conformance rejections for some
    /// of its forms, e.g. `PartialArgumentList` is mapped for first-class
    /// callable syntax and rejected for partial application).
    Mapped,
    /// Always rejected with `RPHP_E0011` (partial-application placeholders).
    Rejected,
    /// A parser-recovery node; becomes `Expr::Error` / `Stmt::Nop` while the
    /// error mago recorded is reported.
    Recovery,
}

/// Every `mago_syntax::cst::NodeKind` of the pinned version and how the
/// adapter treats it. `tests/node_kinds.rs` checks this table against the
/// crate. A kind that is missing here reaches the `_` arms of the
/// converters and produces `RPHP_E0010`.
pub const COVERED: &[(&str, Coverage)] = &[
    ("Program", Coverage::Mapped),
    ("ConstantAccess", Coverage::Mapped),
    ("Access", Coverage::Mapped),
    ("ClassConstantAccess", Coverage::Mapped),
    ("NullSafePropertyAccess", Coverage::Mapped),
    ("PropertyAccess", Coverage::Mapped),
    ("StaticPropertyAccess", Coverage::Mapped),
    ("Argument", Coverage::Mapped),
    ("ArgumentList", Coverage::Mapped),
    ("PartialArgument", Coverage::Mapped),
    ("PartialArgumentList", Coverage::Mapped),
    ("NamedArgument", Coverage::Mapped),
    ("NamedPlaceholderArgument", Coverage::Rejected),
    ("PlaceholderArgument", Coverage::Rejected),
    ("PositionalArgument", Coverage::Mapped),
    ("VariadicPlaceholderArgument", Coverage::Mapped),
    ("Array", Coverage::Mapped),
    ("ArrayAccess", Coverage::Mapped),
    ("ArrayAppend", Coverage::Mapped),
    ("ArrayElement", Coverage::Mapped),
    ("KeyValueArrayElement", Coverage::Mapped),
    ("LegacyArray", Coverage::Mapped),
    ("List", Coverage::Mapped),
    ("MissingArrayElement", Coverage::Mapped),
    ("ValueArrayElement", Coverage::Mapped),
    ("VariadicArrayElement", Coverage::Mapped),
    ("Attribute", Coverage::Mapped),
    ("AttributeList", Coverage::Mapped),
    ("Block", Coverage::Mapped),
    ("Call", Coverage::Mapped),
    ("FunctionCall", Coverage::Mapped),
    ("MethodCall", Coverage::Mapped),
    ("NullSafeMethodCall", Coverage::Mapped),
    ("StaticMethodCall", Coverage::Mapped),
    ("PartialApplication", Coverage::Mapped),
    ("FunctionPartialApplication", Coverage::Mapped),
    ("MethodPartialApplication", Coverage::Mapped),
    ("StaticMethodPartialApplication", Coverage::Mapped),
    ("ClassLikeConstant", Coverage::Mapped),
    ("ClassLikeConstantItem", Coverage::Mapped),
    ("EnumCase", Coverage::Mapped),
    ("EnumCaseBackedItem", Coverage::Mapped),
    ("EnumCaseItem", Coverage::Mapped),
    ("EnumCaseUnitItem", Coverage::Mapped),
    ("Extends", Coverage::Mapped),
    ("Implements", Coverage::Mapped),
    ("ClassLikeConstantSelector", Coverage::Mapped),
    ("ClassLikeMember", Coverage::Mapped),
    ("ClassLikeMemberExpressionSelector", Coverage::Mapped),
    ("ClassLikeMemberSelector", Coverage::Mapped),
    ("Method", Coverage::Mapped),
    ("MethodAbstractBody", Coverage::Mapped),
    ("MethodBody", Coverage::Mapped),
    ("HookedProperty", Coverage::Mapped),
    ("PlainProperty", Coverage::Mapped),
    ("Property", Coverage::Mapped),
    ("PropertyAbstractItem", Coverage::Mapped),
    ("PropertyConcreteItem", Coverage::Mapped),
    ("PropertyHook", Coverage::Mapped),
    ("PropertyHookAbstractBody", Coverage::Mapped),
    ("PropertyHookBody", Coverage::Mapped),
    ("PropertyHookConcreteBody", Coverage::Mapped),
    ("PropertyHookConcreteExpressionBody", Coverage::Mapped),
    ("PropertyHookList", Coverage::Mapped),
    ("PropertyItem", Coverage::Mapped),
    ("TraitUse", Coverage::Mapped),
    ("TraitUseAbsoluteMethodReference", Coverage::Mapped),
    ("TraitUseAbstractSpecification", Coverage::Mapped),
    ("TraitUseAdaptation", Coverage::Mapped),
    ("TraitUseAliasAdaptation", Coverage::Mapped),
    ("TraitUseConcreteSpecification", Coverage::Mapped),
    ("TraitUseMethodReference", Coverage::Mapped),
    ("TraitUsePrecedenceAdaptation", Coverage::Mapped),
    ("TraitUseSpecification", Coverage::Mapped),
    ("AnonymousClass", Coverage::Mapped),
    ("Class", Coverage::Mapped),
    ("Enum", Coverage::Mapped),
    ("EnumBackingTypeHint", Coverage::Mapped),
    ("Interface", Coverage::Mapped),
    ("Trait", Coverage::Mapped),
    ("Clone", Coverage::Mapped),
    ("Constant", Coverage::Mapped),
    ("ConstantItem", Coverage::Mapped),
    ("Construct", Coverage::Mapped),
    ("DieConstruct", Coverage::Mapped),
    ("EmptyConstruct", Coverage::Mapped),
    ("EvalConstruct", Coverage::Mapped),
    ("ExitConstruct", Coverage::Mapped),
    ("IncludeConstruct", Coverage::Mapped),
    ("IncludeOnceConstruct", Coverage::Mapped),
    ("IssetConstruct", Coverage::Mapped),
    ("PrintConstruct", Coverage::Mapped),
    ("RequireConstruct", Coverage::Mapped),
    ("RequireOnceConstruct", Coverage::Mapped),
    ("If", Coverage::Mapped),
    ("IfBody", Coverage::Mapped),
    ("IfColonDelimitedBody", Coverage::Mapped),
    ("IfColonDelimitedBodyElseClause", Coverage::Mapped),
    ("IfColonDelimitedBodyElseIfClause", Coverage::Mapped),
    ("IfStatementBody", Coverage::Mapped),
    ("IfStatementBodyElseClause", Coverage::Mapped),
    ("IfStatementBodyElseIfClause", Coverage::Mapped),
    ("Match", Coverage::Mapped),
    ("MatchArm", Coverage::Mapped),
    ("MatchDefaultArm", Coverage::Mapped),
    ("MatchExpressionArm", Coverage::Mapped),
    ("Switch", Coverage::Mapped),
    ("SwitchBody", Coverage::Mapped),
    ("SwitchBraceDelimitedBody", Coverage::Mapped),
    ("SwitchCase", Coverage::Mapped),
    ("SwitchCaseSeparator", Coverage::Mapped),
    ("SwitchColonDelimitedBody", Coverage::Mapped),
    ("SwitchDefaultCase", Coverage::Mapped),
    ("SwitchExpressionCase", Coverage::Mapped),
    ("Declare", Coverage::Mapped),
    ("DeclareBody", Coverage::Mapped),
    ("DeclareColonDelimitedBody", Coverage::Mapped),
    ("DeclareItem", Coverage::Mapped),
    ("EchoTag", Coverage::Mapped),
    ("Echo", Coverage::Mapped),
    ("Expression", Coverage::Mapped),
    ("Binary", Coverage::Mapped),
    ("BinaryOperator", Coverage::Mapped),
    ("UnaryPrefix", Coverage::Mapped),
    ("UnaryPrefixOperator", Coverage::Mapped),
    ("UnaryPostfix", Coverage::Mapped),
    ("UnaryPostfixOperator", Coverage::Mapped),
    ("Parenthesized", Coverage::Mapped),
    ("ArrowFunction", Coverage::Mapped),
    ("Closure", Coverage::Mapped),
    ("ClosureUseClause", Coverage::Mapped),
    ("ClosureUseClauseVariable", Coverage::Mapped),
    ("Function", Coverage::Mapped),
    ("FunctionLikeParameter", Coverage::Mapped),
    ("FunctionLikeParameterDefaultValue", Coverage::Mapped),
    ("FunctionLikeParameterList", Coverage::Mapped),
    ("FunctionLikeReturnTypeHint", Coverage::Mapped),
    ("Global", Coverage::Mapped),
    ("Goto", Coverage::Mapped),
    ("Label", Coverage::Mapped),
    ("HaltCompiler", Coverage::Mapped),
    ("FullyQualifiedIdentifier", Coverage::Mapped),
    ("Identifier", Coverage::Mapped),
    ("LocalIdentifier", Coverage::Mapped),
    ("QualifiedIdentifier", Coverage::Mapped),
    ("Inline", Coverage::Mapped),
    ("Instantiation", Coverage::Mapped),
    ("Keyword", Coverage::Mapped),
    ("Literal", Coverage::Mapped),
    ("Pipe", Coverage::Mapped),
    ("LiteralFloat", Coverage::Mapped),
    ("LiteralInteger", Coverage::Mapped),
    ("LiteralString", Coverage::Mapped),
    ("MagicConstant", Coverage::Mapped),
    ("Modifier", Coverage::Mapped),
    ("Namespace", Coverage::Mapped),
    ("NamespaceBody", Coverage::Mapped),
    ("NamespaceImplicitBody", Coverage::Mapped),
    ("Assignment", Coverage::Mapped),
    ("AssignmentOperator", Coverage::Mapped),
    ("Conditional", Coverage::Mapped),
    ("DoWhile", Coverage::Mapped),
    ("Foreach", Coverage::Mapped),
    ("ForeachBody", Coverage::Mapped),
    ("ForeachColonDelimitedBody", Coverage::Mapped),
    ("ForeachKeyValueTarget", Coverage::Mapped),
    ("ForeachTarget", Coverage::Mapped),
    ("ForeachValueTarget", Coverage::Mapped),
    ("For", Coverage::Mapped),
    ("ForBody", Coverage::Mapped),
    ("ForColonDelimitedBody", Coverage::Mapped),
    ("While", Coverage::Mapped),
    ("WhileBody", Coverage::Mapped),
    ("WhileColonDelimitedBody", Coverage::Mapped),
    ("Break", Coverage::Mapped),
    ("Continue", Coverage::Mapped),
    ("Return", Coverage::Mapped),
    ("Static", Coverage::Mapped),
    ("StaticAbstractItem", Coverage::Mapped),
    ("StaticConcreteItem", Coverage::Mapped),
    ("StaticItem", Coverage::Mapped),
    ("Try", Coverage::Mapped),
    ("TryCatchClause", Coverage::Mapped),
    ("TryFinallyClause", Coverage::Mapped),
    ("MaybeTypedUseItem", Coverage::Mapped),
    ("MixedUseItemList", Coverage::Mapped),
    ("TypedUseItemList", Coverage::Mapped),
    ("TypedUseItemSequence", Coverage::Mapped),
    ("Use", Coverage::Mapped),
    ("UseItem", Coverage::Mapped),
    ("UseItemAlias", Coverage::Mapped),
    ("UseItemSequence", Coverage::Mapped),
    ("UseItems", Coverage::Mapped),
    ("UseType", Coverage::Mapped),
    ("Yield", Coverage::Mapped),
    ("YieldFrom", Coverage::Mapped),
    ("YieldPair", Coverage::Mapped),
    ("YieldValue", Coverage::Mapped),
    ("Statement", Coverage::Mapped),
    ("ExpressionStatement", Coverage::Mapped),
    ("BracedExpressionStringPart", Coverage::Mapped),
    ("DocumentString", Coverage::Mapped),
    ("InterpolatedString", Coverage::Mapped),
    ("LiteralStringPart", Coverage::Mapped),
    ("ShellExecuteString", Coverage::Mapped),
    ("CompositeString", Coverage::Mapped),
    ("StringPart", Coverage::Mapped),
    ("ClosingTag", Coverage::Mapped),
    ("FullOpeningTag", Coverage::Mapped),
    ("OpeningTag", Coverage::Mapped),
    ("ShortOpeningTag", Coverage::Mapped),
    ("Terminator", Coverage::Mapped),
    ("Throw", Coverage::Mapped),
    ("Hint", Coverage::Mapped),
    ("IntersectionHint", Coverage::Mapped),
    ("NullableHint", Coverage::Mapped),
    ("ParenthesizedHint", Coverage::Mapped),
    ("UnionHint", Coverage::Mapped),
    ("Unset", Coverage::Mapped),
    ("DirectVariable", Coverage::Mapped),
    ("IndirectVariable", Coverage::Mapped),
    ("NestedVariable", Coverage::Mapped),
    ("Variable", Coverage::Mapped),
    ("Error", Coverage::Recovery),
    ("MissingTerminator", Coverage::Recovery),
    ("ClassLikeMemberMissingSelector", Coverage::Recovery),
    ("ClassLikeConstantMissingSelector", Coverage::Recovery),
];

/// Look a node kind up in [`COVERED`].
pub fn coverage_of(kind: &str) -> Option<Coverage> {
    COVERED.iter().find(|(k, _)| *k == kind).map(|(_, c)| *c)
}
