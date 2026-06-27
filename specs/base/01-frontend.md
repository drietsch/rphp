# 01 — Front End: Source, Diagnostics, Lexer, Parser/AST, HIR

**Status:** stable
**Source sections:** `base-idea.md` §3 (source & diagnostics), §4 (lexer), §5 (parser/AST), §6 (HIR)
**Reads with:** [00-overview.md](00-overview.md), [decisions.md](decisions.md) (ADR-007), [05-bytecode-isa.md](05-bytecode-isa.md) (the compile target)

The front end turns source bytes into HIR — a small, regular, resolved, desugared IR that the compiler lowers to bytecode. The governing decision (ADR-007) is that **`rphp-ast` owns the typed-AST contract** while the parser itself is **bootstrapped on `mago-syntax`** for M0, with the lossless CST **deferred**.

---

## 3. Source & diagnostics (§3)

### `rphp-span`
```rust
pub struct Span { pub file: FileId, pub lo: u32, pub hi: u32 } // byte offsets
```
4 GiB per-file ceiling (fine for PHP). Every node from AST through bytecode carries a `Span` for backtraces, error reporting, and JIT source maps. `rphp-span` has **no dependencies**.

### `rphp-source`
Owns source files, byte content, line-start maps (`offset → (line, col)`), and a virtual filesystem trait so embedders and the WASM SAPI supply sources without a real FS. Source is `&[u8]`, never assumed UTF-8.

### `rphp-diagnostics`
Structured error model: stable codes (`RPHP_E0001…`), severity, a primary label + secondary labels, and an `ariadne`/rustc-style renderer. **Parser errors are recoverable**: the parser emits error nodes and continues (required for editor tooling and fault-tolerant batch compilation). Diagnostics are data, not strings — the renderer is one consumer; the LSP/JSON emitter is another.

---

## 4. Lexer (§4)

`rphp-lexer` is a hand-written **byte-level** scanner over `&[u8]`.

- Single forward pass, no backtracking; a branch-predictable hot loop over a **256-entry classification table**.
- Handles the awkward PHP corners directly: `<?php`/`?>` island toggling between literal HTML and code; heredoc/nowdoc with indentation stripping; string-interpolation states; numeric-literal separators (`1_000`); attributes `#[...]`; the 8.5 pipe token `|>`.
- Emits a **flat token stream with trivia** (whitespace, comments) attached as side-channel data, so a future CST can be lossless without slowing the parser.
- **Interns identifiers eagerly**: every identifier token carries an `IdentId` from `rphp-intern`, so downstream stages compare symbols by integer.
- Fully **streamable** for WASM and large-file cases.

> **SIMD note.** String scanning, comment skipping, and UTF-8 validation are SIMD kernels (simdutf-style, AVX-512/SVE2 with an AVX2 floor) — see [03-heap-types.md](03-heap-types.md) §strings for the shared kernels. These are an optimization over the scalar classifier, gated on a lexing micro-benchmark (see O-1).

### O-1 — own lexer vs reuse mago's in M0

For M0 the parser is `mago-syntax` (below), which has its own lexer. **Decision for M0:** reuse mago's lexer; build the owned byte-level SIMD lexer above only once a benchmark shows lexing on the critical path of cold-start/compile throughput. The owned lexer's token contract is specified now so the swap is local.

---

## 5. Parser & AST (§5) — ADR-007

PHP's grammar has context sensitivity (cast-vs-parenthesized-expression, heredoc bodies, magic constants) that fights generators, so the parser is **recursive-descent with Pratt expression precedence** — whether hand-written (later) or `mago-syntax` (now).

### The ownership boundary

- **`rphp-ast` owns the typed-AST contract.** It is the single type the rest of the compiler (`rphp-hir` and below) consumes. Nothing downstream depends on the parser implementation.
- **`rphp-parser` is a thin adapter** that drives `mago-syntax` and maps its nodes onto `rphp-ast`. The adapter is the only crate aware mago exists.
- **Lossless CST is deferred** (non-v1). It is tooling value (formatter, refactor, exact round-trip) and never on the runtime hot path. It can be added later behind the same adapter without touching downstream crates.

```
source ─▶ mago-syntax ─▶ [rphp-parser adapter] ─▶ rphp-ast (typed AST) ─▶ rphp-hir
                                                   ▲ owned contract; impl swappable
```

### Adapter contract requirements

The adapter must:
1. Produce `rphp-ast` nodes with `rphp-span` spans (mapping mago's spans onto `FileId`-relative offsets).
2. Intern every identifier into `rphp-intern`, yielding `IdentId`.
3. Surface mago's recoverable errors as `rphp-diagnostics` with stable `RPHP_E####` codes (a fixed mapping table).
4. Be **pure and deterministic** so the parse artifact is content-addressable for the on-disk cache ([00-overview.md](00-overview.md) §1).

If mago's lossless-CST or error-recovery shape later diverges from rPHP's needs, the owned parser replaces the adapter behind this exact contract.

---

## 6. HIR: name resolution & desugaring (§6)

`rphp-hir` collapses surface syntax into a small, regular core. It is intentionally boring — a few expression kinds, explicit control flow, explicit temporaries — so the compiler ([05-bytecode-isa.md](05-bytecode-isa.md)) and the optimizer ([07-jit.md](07-jit.md)) have a regular target.

### 6.1 Name resolution (`rphp-resolve`)

Resolves class/function/constant/namespace references to fully-qualified `SymbolId`s, wires up `use` imports, and **records autoload points**: a reference unresolved at compile time becomes an explicit `Autoload(SymbolId)` node that triggers runtime class loading. These autoload points are also where the optimizer's closed-world proof (ADR-006) must stop — an unresolved symbol is an open-world edge.

```rust
pub enum Resolution {
    Resolved(SymbolId),          // statically known
    Autoload(QualifiedName),     // resolved at runtime; an open-world edge (ADR-006)
    Dynamic,                     // variable-variable / variable class — slow path (§14)
}
```

### 6.2 Desugaring catalogue

Every convenient-but-redundant surface form is rewritten to a canonical HIR form here, so exactly one shape reaches the compiler:

| Surface form | Canonical HIR |
|--------------|---------------|
| Short closure `fn($x) => e` | full closure with captured-by-value upvalues |
| Pipe `$x \|> f(...)` | straight call chain `f($x)` |
| `clone($o, [p => v])` (8.5 clone-with) | `clone` then typed-property writes respecting hooks/visibility |
| `match (e) { … }` | strict-compare (`===`) decision tree |
| Null-safe `$a?->b` | guarded access: `tmp = $a; tmp === null ? null : tmp->b` |
| Interpolated `"… $x …"` | concatenation, or an optimized join the optimizer may rope-ify |
| First-class callable `f(...)` | closure over the named callable (same repr as §11.4) |
| List/array destructuring | explicit element reads + assigns |
| `??=`, compound assigns | read-modify-write on the canonical lvalue |
| `foreach` | `IterInit`/`IterNext`/`IterValue` over the iterable (see [05](05-bytecode-isa.md)) |

### 6.3 Constant folding & const-expression evaluation

Attribute arguments, const initializers, enum cases, and the 8.5-permitted static closures / first-class callables **in constant expressions** are evaluated to a normal form here. Folded values become `LoadConst` operands downstream. Const evaluation is the same evaluator the runtime uses, run at compile time on a restricted (side-effect-free) subset, so folded results are bit-identical to runtime evaluation.

---

## Deviations from base-idea.md

- **§5 lossless CST → deferred; parser bootstraps on `mago-syntax`** (ADR-007). `rphp-ast` owns the typed-AST contract; the CST is added later behind the adapter only if tooling requires it. Rationale: fastest path to M1, no runtime hot-path dependency on a CST, preserves the option to own the parser later.

## Open questions

- **O-1** — own byte-level SIMD lexer vs reuse mago's lexer in M0. Decided *reuse for M0*; revisit on a lexing benchmark (see [decisions.md](decisions.md)).
- Error-code mapping table (`RPHP_E####` ↔ mago error kinds) is filled in during M0 as the corpus surfaces error shapes.
