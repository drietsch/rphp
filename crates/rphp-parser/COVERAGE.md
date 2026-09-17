# rphp-parser grammar coverage

Honest state of the front end per grammar family. Columns:

- **Parsed** — the construct produces an AST v2 node (no diagnostic) through `parse_v2`, the
  `mago-syntax` adapter (`src/adapter/`, ADR-025).
- **Resolved** — names/scopes/desugaring handled by `rphp-hir` (F4; the crate does not exist yet).
- **Lowered** — the compiler emits bytecode for it and the runtime executes it (the compiler
  still consumes the M0 AST from the hand-written `parse` until F3).
- **Differential** — covered by a snippet under `examples/tier-a/` that is compared byte-exact
  against stock `php` 8.5.

Legend: `yes` / `partial (what)` / `no`. **F2 is done**: `parse_v2` covers the full PHP 8.4/8.5
grammar (every mago `NodeKind` is mapped or explicitly rejected, `tests/node_kinds.rs`), and
the two-way `php -l` parity gate is green on the whole 22k-file vendor corpus. **F4** fills
*Resolved*; **F3** plus the engine waves (A then B) fill *Lowered*; every wave adds tier-a
snippets that fill *Differential*. Re-generate the numbers below with the commands in the
last section whenever a column changes.

| Grammar family | Parsed | Resolved | Lowered | Differential |
|---|---|---|---|---|
| Operators: arithmetic `+ - * / % **`, concat `.`, comparison `== != === !== < <= > >= <=>`, logical `&& \|\| !`, unary `-` | yes | no | yes | yes (`math.php`, `string.php`) |
| Operators: bitwise, `and/or/xor`, ternary `?:`, `??`/`??=`, compound assign `op=`, `++`/`--`, casts, `@`, `instanceof` | yes | no | partial (`instanceof`) | partial (`inheritance.php`) |
| Literals: decimal int/float (with `_` separators), `true/false/null`, single/double-quoted strings with `$var`/`{$var}` interpolation and escapes | yes | no | yes | yes (`string.php`) |
| Heredoc / nowdoc (incl. flexible indentation) | yes | no | no | no |
| Variables / lvalues: `$x`, `$x[..]`, `$x[] =`, `$o->p =` | yes | no | partial | yes (`array.php`, `byref.php`) |
| Control flow: `if/else`, `while`, `foreach` (`k => v`), `return` | yes | no | partial (same subset) | yes |
| Functions: declarations, calls, dynamic calls `$f()`, closures with `use`, arrow functions `fn` | yes | no | partial (same subset) | yes (`closures.php`, `callables.php`, `dynamic-calls.php`) |
| Classes: `class`, `extends`, properties with visibility, constructors, methods, `$this`, `new`, `::` static calls | yes | no | partial (same subset) | yes (`objects.php`, `inheritance.php`) |
| Property hooks, asymmetric visibility (8.4) | yes | no | no | no |
| Attributes `#[...]` | yes | no | no | no |
| Namespaces / `use` / qualified names `\Foo\Bar` | yes | no | no | no |
| Exceptions: `try/catch/finally`, `throw` (statement and expression) | yes | no | no | no |
| Generators: `yield`, `yield from` | yes | no | no | no |
| `match` | yes | no | no | no |
| Destructuring: `list()`, `[$a, $b] =`, keyed/nested/by-ref, in `foreach` | yes | no | no | no |
| `include`/`require`(`_once`), `eval` | yes | no | no | no |
| Inline HTML, `<?=`, `?>` handling, `__halt_compiler` | yes | no | no | no |
| Pipe operator `\|>`, `clone(...)` with-syntax (8.5) | yes | no | no | no |
| Type declarations (params, returns, properties; nullable/union/intersection/DNF) | yes | no | no | no |
| Constants: `const`, `define()`, bare constant reads, magic constants, `::class` | yes | no | no | no |
| `isset`/`empty`/`unset`, `global`/`static` vars, `print`/`exit`/`die` | yes | no | no | no |

Diagnostics: `RPHP_E0001` unexpected/unrecognised byte, `E0002` unexpected token (mago's
message with the expected-token set), `E0003` unterminated string, `E0004` unexpected end of
file, `E0005` nesting limit (mago caps expressions at 512 levels), `E0006` invalid literal
(numeric literal, `\u{}` escape, heredoc indentation), `E0010` unsupported node (never emitted
for mago 1.49 — the coverage test guards it), `E0011` superset rejected (syntax mago accepts and
PHP 8.5.0 does not, with PHP's own message), `E0020–E0025` write-context and `strict_types`
validation (`rphp_ast::v2::lvalue` wording); `E0300` marks "parsed but not yet lowered" (F3).

### F2 adapter notes (what `parse_v2` does beyond the tree)

- **Conformance pass** (`src/adapter/conformance.rs` and the per-family modules): every rule
  is backed by a `php -l` verdict on a snippet in `tests/negative/` (421 files, all rejected by
  both sides). Beyond the plan's list (partial application `f(?, 1)`, `&&=`/`||=` — a mago parse
  error already —, `(real)`/`(unset)`, `;` case separators — PHP 8.5 only *deprecates* them, so
  they are accepted —, nullsafe writes, positional-after-named, `new`/`?->` in FCC — mago parse
  errors —, `use ($this)`) it covers the compile-time rules `php -l` reports: constant-expression
  shapes, modifier validity/duplicates, magic-method signatures, abstract/interface/enum member
  rules, readonly/asymmetric-visibility/hook rules, type-declaration rules (`void`/`never`/`mixed`
  placement, redundant union/DNF members), `break`/`continue` levels, `return` vs return type,
  namespace ordering and mixing, `strict_types` placement, `__halt_compiler` scope, reserved
  words as names, `$this`/`$GLOBALS` write rules, `yield` outside functions, `isset()` on
  expressions, `parent::$p::get()` hook calls, `100._0` literals.
- **Compile-time dead code**: like Zend, `false && rhs` / `true || rhs` with a literal left side
  never compiles `rhs`, so its conformance diagnostics are dropped (php-src's `assert(false && …)`
  AST-printing tests rely on it).
- **Docblocks** attach by adjacency (`get_docblock_before_position`, looking before the attribute
  list first). PHP's actual rule is "the last docblock since the previous declaration or `}`", so
  `/** x */ $e = 1; function i() {}` gives `i` a docblock in PHP and none here. Deferred to F7
  (declaration metadata); Reflection::getDocComment differs only in that quirk.
- **`T::m as final;`** is accepted (PHP allows it) but AST v2 has no slot for the `final` flag on
  an alias; it is dropped (F1 follow-up).
- **`ident_spans`** records every identifier in a position PHP's `TOKEN_PARSE` reclassifies to
  `T_STRING` (member names after `->`/`?->`/`::`, named-argument labels, method/constant/case
  names in declarations, `goto` labels, trait-adaptation names); the tokenizer intersects it with
  its keyword tokens. Not recorded: `use … as` aliases, namespace segments (both reclassified by
  other productions). Best effort, per the plan.
- **Not the adapter's job** (semantic passes, documented so nobody re-adds them here): `goto`
  label resolution/duplicates and jumps into loops (F4), function/class redeclaration (declaration
  tables), typed class-constant value checks (F7), virtual-vs-backed hooked property defaults
  (needs hook-body analysis), `declare(encoding)` support.
- **Pipe precedence**: `|>` is mapped as a binary operator with mago's precedence (between
  comparison and concatenation, left-assoc; arrow functions on the right must be parenthesized,
  as in PHP). Deep validation of `|>` against php-src's precedence table is deferred (plan
  allowance); the snapshot pins `$x |> strlen(...) |> (fn($n) => $n + 1)`.
- **Stack**: mago's parser needs ~14 KB per nesting level in debug builds (~3 KB in release);
  its 512-level cap fits in 4 MB release / 16 MB debug. Left-associative chains
  (`"a" . "b" . …`, 20k terms) are folded iteratively by the adapter; dropping such a tree still
  recurses in `rphp-ast`'s drop glue (F1 follow-up). xtask runs the front end on 1 GiB stacks.

## Parity jobs (the F2 gate)

Both jobs live in `xtask/` and share one report shape (false rejects grouped by the first
diagnostic's code + message prefix with three `file:line` examples each, false accepts,
panics, timing; JSON copy under `target/`). They exit non-zero on any disagreement unless
`--allow-failures`. Each calls the front end through one tiny `our_verdict(&[u8])`
function, now pointed at `parse_v2` (an `RPHP_E0010` is rendered first so unsupported nodes
form their own report group).

```sh
# Two-way parity vs `php -n -l` over a vendor tree (verdicts cached by sha256 under target/corpus-cache/)
cargo xtask corpus --dir /path/to/vendor [--negative tests/negative] [--allow-failures]
RPHP_CORPUS_DIR=/path/to/vendor cargo xtask corpus --report json

# Parse-only sweep over php-src Zend/tests + tests/lang (--FILE-- vs --EXPECT*-- classification)
tools/php-src/checkout.sh        # once: sparse checkout of php-8.5.0 into vendor-php-src/
cargo xtask parse-sweep [--allow-failures] [--filter heredoc] [--stack-mb 256]

# Fuzzing (separate cargo-fuzz workspace, see fuzz/README.md)
cd fuzz && ./seed-corpus.sh && cargo fuzz run parse_bytes -- -timeout=10
```

The F2 exit criteria, in these jobs' terms: corpus false rejects = 0 on every `php -l`-clean
file (met), every negative-corpus file rejected (met), parse-sweep disagreements reduced to
the sweep's own classification artifacts and mago's two documented limits (met, see the
numbers below), and `parse_bytes` running clean overnight (the fuzz target still drives the M0
parser; F3 repoints it at `parse_v2`).

## Numbers today (mago adapter, 2026-09-17)

`cargo xtask corpus --dir /Users/drietsch/gartner/skeleton/vendor/symfony --negative crates/rphp-parser/tests/negative --allow-failures`
(Symfony 7.4 vendor subtree, `Tests/` skipped, plus the negative corpus):

| | count |
|---|---|
| files | 4957 positive + 421 negative |
| `php -l` ok / error | 4957 / 421 |
| rphp ok / error / panic | 4957 / 421 / 0 |
| false rejects | **0** |
| false accepts | **0** |
| unsupported nodes (`RPHP_E0010`) | **0** |

Whole vendor tree (`--dir /Users/drietsch/gartner/skeleton/vendor`, 22,041 files after skipping
310 `Tests/` dirs): rphp ok 22,041, false rejects **0**, false accepts **0**, panics 0
(44 s wall with cached verdicts; the adapter itself takes 1.7 s cumulative over 16 threads).

`cargo xtask parse-sweep --allow-failures` (php-8.5.0 `Zend/tests` + `tests/lang`, 5550 tests):

| | count |
|---|---|
| expected parses / parse error (sweep classification) | 5424 / 126 |
| rphp ok / error / panic | 5070 / 480 / 0 |
| agree | 5178 |
| false rejects (sweep) | 363 — **361** are compile-time fatals that `php -l` also reports (the sweep only counts `Parse error` heads as expected errors; both sides reject, 34 differ in wording), **2** are mago limitations: `$obj->__halt_compiler` (mago's lexer enters halt mode after `->`, `Zend/tests/grammar/semi_reserved_003.phpt`) and 10k-deep unary nesting beyond mago's 512-level cap (`stack_limit_013.phpt`) |
| false accepts (sweep) | 9 — all files `php -l` accepts: the expected parse errors come from `eval()`, included files or `parse_ini_string` (`bug38779*`, `bug69551`, `bug69640`, `bug70748`, `bug75252`, `flexible-nowdoc-error8`, `oss_fuzz_428983568`, `bug71897`) |

The 34 wording differences on files both sides reject are message-only (e.g. PHP prints
`Duplicate type Foo` where the adapter prints `Duplicate type self`, PHP expands `iterable` to
`Traversable|array` in type messages, `unset($GLOBALS[])` reports the append before the
`$GLOBALS` rule, mago's own parse error for `new Foo(...)` replaces PHP's "Cannot create Closure
for new expression").

Timing on a 16-thread laptop: a cached corpus run over the whole vendor tree takes 44 s wall
(sha256 of 30 MB of sources in the dev profile dominates; the parse itself is ~80 µs per file in
release). The parse sweep takes 0.1 s of parsing.

Fuzzing baseline (`cd fuzz && cargo fuzz run parse_bytes -- -max_len=4096 -timeout=10`): the
target still drives the M0 parser until F3 repoints it; `tests/negative.rs` and
`tests/adapter_snapshots.rs` parse every byte prefix of every negative and snapshot source
through `parse_v2` without panicking.
