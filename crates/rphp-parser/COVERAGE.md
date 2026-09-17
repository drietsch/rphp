# rphp-parser grammar coverage

Honest state of the front end per grammar family. Columns:

- **Parsed** — the construct produces an AST node (no diagnostic) in `rphp-parser`.
- **Resolved** — names/scopes/desugaring handled by `rphp-hir` (F4; the crate does not exist yet).
- **Lowered** — the compiler emits bytecode for it and the runtime executes it.
- **Differential** — covered by a snippet under `examples/tier-a/` that is compared byte-exact against stock `php` 8.5.

Legend: `yes` / `partial (what)` / `no`. Today the parser is the hand-written M0
recursive-descent parser (`src/lib.rs`, one file). Roadmap: **F2** replaces it with the
`mago-syntax` adapter and flips the *Parsed* column to `yes` across the board; **F4**
fills *Resolved*; **F3** plus the engine waves (A then B) fill *Lowered*; every wave adds
tier-a snippets that fill *Differential*. Re-generate the numbers below with the commands
in the last section whenever a column changes.

| Grammar family | Parsed | Resolved | Lowered | Differential |
|---|---|---|---|---|
| Operators: arithmetic `+ - * / % **`, concat `.`, comparison `== != === !== < <= > >= <=>`, logical `&& \|\| !`, unary `-` | yes | no | yes | yes (`math.php`, `string.php`) |
| Operators: bitwise, `and/or/xor`, ternary `?:`, `??`/`??=`, compound assign `op=`, `++`/`--`, casts, `@`, `instanceof` | partial (`instanceof` only) | no | partial (`instanceof`) | partial (`inheritance.php`) |
| Literals: decimal int/float (with `_` separators), `true/false/null`, single/double-quoted strings with `$var`/`{$var}` interpolation and escapes | partial (no hex/octal/binary, no `\u{}` edge cases vs php, overflow-to-float untested) | no | yes | yes (`string.php`) |
| Heredoc / nowdoc (incl. flexible indentation) | no | no | no | no |
| Variables / lvalues: `$x`, `$x[..]`, `$x[] =`, `$o->p =` | partial (no `$$x`, no static props, no nested write chains, no references `&`) | no | partial | yes (`array.php`, `byref.php`) |
| Control flow: `if/else`, `while`, `foreach` (`k => v`), `return` | partial (no `elseif`, `for`, `do/while`, `switch`, `break/continue N`, `goto`, alt syntax, `declare`) | no | partial (same subset) | yes |
| Functions: declarations, calls, dynamic calls `$f()`, closures with `use`, arrow functions `fn` | partial (untyped params only; no defaults, variadics, by-ref, named args, spread, static closures, first-class callables) | no | partial (same subset) | yes (`closures.php`, `callables.php`, `dynamic-calls.php`) |
| Classes: `class`, `extends`, properties with visibility, constructors, methods, `$this`, `new`, `::` static calls | partial (no interfaces, traits, enums, abstract/final, class constants, static props, readonly, anonymous classes, `parent::`/`self::`/`static::` semantics) | no | partial (same subset) | yes (`objects.php`, `inheritance.php`) |
| Property hooks, asymmetric visibility (8.4) | no | no | no | no |
| Attributes `#[...]` | no (lexer: unexpected character) | no | no | no |
| Namespaces / `use` / qualified names `\Foo\Bar` | no (lexer: unexpected character; the largest false-reject group) | no | no | no |
| Exceptions: `try/catch/finally`, `throw` (statement and expression) | no | no | no | no |
| Generators: `yield`, `yield from` | no | no | no | no |
| `match` | no | no | no | no |
| Destructuring: `list()`, `[$a, $b] =`, keyed/nested/by-ref, in `foreach` | no | no | no | no |
| `include`/`require`(`_once`), `eval` | no | no | no | no |
| Inline HTML, `<?=`, `?>` handling, `__halt_compiler` | no (bytes outside `<?php ... ?>` are dropped, not emitted as inline HTML) | no | no | no |
| Pipe operator `\|>`, `clone(...)` with-syntax (8.5) | no | no | no | no |
| Type declarations (params, returns, properties; nullable/union/intersection/DNF) | no (typed params are rejected) | no | no | no |
| Constants: `const`, `define()`, bare constant reads, magic constants, `::class` | no (bare constants rejected: "expected `(` after name") | no | no | no |
| `isset`/`empty`/`unset`, `global`/`static` vars, `print`/`exit`/`die` | no | no | no | no |

Diagnostics today: `RPHP_E0001` unexpected character (lexer), `RPHP_E0002` unexpected
token (parser, recoverable), `RPHP_E0003` unterminated string. F2 allocates
`E0001–E0006` for the mago error kinds and `E0010/E0011` for unsupported / superset-rejected
nodes; `E0300` marks "parsed but not yet lowered".

## Parity jobs (the F2 gate)

Both jobs live in `xtask/` and share one report shape (false rejects grouped by the first
diagnostic's code + message prefix with three `file:line` examples each, false accepts,
panics, timing; JSON copy under `target/`). They exit non-zero on any disagreement unless
`--allow-failures`. Each calls the front end through one tiny `our_verdict(&[u8])`
function; F2 repoints that at the adapter.

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
file, every negative-corpus file rejected, parse-sweep false rejects = 0 and false accepts
= 0 (modulo `Zend/tests/bug64660.phpt`, whose expected "memory exhausted" needs a nesting
limit — an F8 hardening item), and `parse_bytes` running clean overnight.

## Numbers today (M0 parser, 2026-09-17)

`cargo xtask corpus --dir /Users/drietsch/gartner/skeleton/vendor/symfony --allow-failures`
(Symfony 7.4 vendor subtree, `Tests/` skipped):

| | count |
|---|---|
| files | 4957 |
| `php -l` ok / error | 4957 / 0 |
| rphp ok / error / panic | 1327 / 3630 / 0 |
| false rejects | **3630** in 3 groups: 3625 × `RPHP_E0001: unexpected character` (namespace `\`, `?`, `\|`, `#[`, …), 4 × `expected expression` (inline-HTML templates), 1 × bare constant |
| false accepts | 0 |

Whole vendor tree (`--dir /Users/drietsch/gartner/skeleton/vendor`, 22,041 files): rphp ok
2024, false rejects 20,017 in 4 groups (19,165 unexpected character, 846 bare constants,
4 expected expression, 2 class-member syntax), false accepts 0.

`cargo xtask parse-sweep --allow-failures` (php-8.5.0 `Zend/tests` + `tests/lang`,
5550 tests, 0 skipped):

| | count |
|---|---|
| expected parses / parse error | 5424 / 126 |
| rphp ok / error / panic | 538 / 5012 / 0 |
| agree | 614 |
| false rejects | **4911** in 25 groups: 2060 unexpected character, 1563 bare constants, 555 class-member syntax, 189 parameter features (types/defaults/variadics), 147 `expected ;`, 147 `expected expression`, 86 `expected )`, 35 anonymous classes, 35 `::` members, 27 `->` members, 24 assignment targets, 13 attributes before class body, … |
| false accepts | **25**: numeric-literal-separator edge cases (`1_`, `1__0`, `1_e2`), `new` without parentheses followed by `[`/`->`, `exit`/`die` declared as functions, alternative encaps offset syntax, unterminated comment, invalid octal, byte 0x7F, variable attribute names, `list()` misuse, LSB syntax, flexible nowdoc error |

Timing on a 16-thread laptop (debug build): the first corpus run spawns one `php -n -l`
per file (~45 ms each on this machine, not the 1.2 ms the roadmap assumed; 38 s wall for
the Symfony subtree, 3.5 min for the whole vendor tree); a fully cached rerun takes 1.9 s
wall (the sha256 of 30 MB of sources in the unoptimized dev profile dominates — a
`[profile.dev.package.sha2] opt-level = 3` in the root manifest would remove most of it).
The parse sweep takes 2.0 s wall; the parse itself is ~65 µs per test.

Fuzzing baseline (`cd fuzz && cargo fuzz run parse_bytes -- -max_len=4096 -timeout=10`):
31k executions in 3 minutes from a 21-file seed corpus, coverage 2209 edges, no crashes,
no timeouts. `tokenize_bytes` is gated behind `--features tokenizer` until F5 lands.

Gaps to close before the numbers mean anything (owned by F2/F5): namespaces and
qualified names, constants, typed declarations, attributes, the full statement set,
heredoc/nowdoc, inline HTML. The grouping above is exactly the work list.
