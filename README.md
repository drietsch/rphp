# rPHP

A clean-room **PHP 8.5 engine written in Rust** — lexer, parser, register-bytecode
compiler, and a tree-walking… no, a *register-bytecode interpreter*, wired together
behind a small CLI SAPI. The long-term design (multi-tier JIT, isolates, full stdlib
parity) lives under [`specs/`](specs/base/00-overview.md); this README describes
**what actually runs today**.

> Status: early but real. The full pipeline executes non-trivial PHP, and every
> language feature is gated by a **differential oracle** — output is compared
> byte-for-byte against stock PHP 8.5. Bleeding-edge and moving fast.

---

## Quick start

```sh
# Build the workspace
cargo build

# Run a script
cargo run -p rphp -- path/to/script.php

# Inspect any pipeline stage
cargo run -p rphp -- --emit=tokens   script.php
cargo run -p rphp -- --emit=ast      script.php
cargo run -p rphp -- --emit=bytecode script.php

# Test everything (unit + end-to-end + differential vs stock php)
cargo test --workspace
```

```php
<?php
class Counter {
    public $count = 0;
    function __construct($start) { $this->count = $start; }
    function inc() { $this->count = $this->count + 1; return $this; }
    function value() { return $this->count; }
}

$c = new Counter(10);
echo $c->inc()->inc()->value(), "\n";   // 12
echo json_encode($c), "\n";             // {"count":12}
```

---

## What works today

**Language** — PHP 8.5 as the differential oracle sees it: the full object
model (interfaces, traits, abstract/final, statics, class constants, enums,
readonly, property hooks and asymmetric visibility, lazy objects, magic
methods, `clone`), closures and first-class callables, generators with
`yield from`, fibers, named arguments (to user and native functions),
references everywhere php has them, exceptions with php's exact messages and
traces, attributes and Reflection, `include`/`require`/autoloading,
`declare(strict_types=1)`, `match`, `eval`.

**Standard library** — the burn-down is tracked per extension in
[`crates/rphp-stdlib/COVERAGE.md`](crates/rphp-stdlib/COVERAGE.md):
standard (strings, arrays, math, var, files/streams/dirs, output buffering,
http/url, info), ctype, json, pcre (over PCRE2), hash, random, date (own
timelib port), mbstring/iconv (pure Rust), spl, reflection, session,
filter, tokenizer, zlib, **pdo + pdo_sqlite**, **dom/libxml/simplexml**
(no libxml2; XPath 1.0, php 8.4's `Dom\*` API with an HTML5 parser and CSS
selectors), plus the engine classes (`Closure`, `Generator`, `Fiber`,
`WeakMap`, …).

**SAPIs** — the CLI (`rphp script.php`, `-r`, `-l`, `-d`), the built-in web
server (`rphp -S host:port -t docroot [router.php]`, byte-compared with
`php -S`), and a FastCGI server (`rphp -b host:port`, byte-compared with
`php-fpm`) for nginx/Caddy deployments. Each keeps php's `$_SERVER` layout,
response head, upload handling and error rendering for that SAPI.

**Applications** — the Symfony ladder (`fixtures/ladder/`) runs
`symfony/skeleton` and `symfony/demo` (Doctrine over SQLite, Twig, security,
forms, translations, the web profiler) on both engines and compares the
output of every console command and HTTP page byte for byte after a
declared set of normalizations: `bin/console about|debug:router|
lint:container`, the demo's blog, posts, search, feeds, login and error
pages are identical to php's.

```sh
rphp -S 127.0.0.1:8000 -t public          # like php -S
rphp -b 127.0.0.1:9000                    # FastCGI, like php-fpm (PHP_FCGI_CHILDREN=4 for workers)
```

**Performance** is not there yet: the interpreter runs php's own benchmarks
at 12–50× php's per-op cost (`examples/bench/`); symfony/demo's blog page
takes ~0.5 s against php's 75 ms. See the perf notes in COVERAGE.md.

---

## Pipeline

```
source bytes
  └─ lex ─▶ tokens ─ parse ─▶ typed AST ─ compile ─▶ register bytecode ─ run ─▶ output
            (rphp-lexer)      (rphp-ast)            (rphp-bytecode)     (rphp-runtime)
```

A three-address, register-based bytecode is interpreted by a portable
`loop { match op { … } }` Tier-0 VM. (The threaded-dispatch core and the JIT tiers
described in the specs are not built yet.)

### Crates

| Crate | Role |
|-------|------|
| `rphp-span`, `rphp-source` | byte spans, source map / line tables |
| `rphp-diagnostics` | error model, codes, renderer |
| `rphp-intern` | string interner (symbols compared as integers) |
| `rphp-value` | the `Value` cell + all scalar/heap operations (the single source of truth) |
| `rphp-lexer`, `rphp-parser`, `rphp-ast` | front end → owned typed AST |
| `rphp-bytecode` | register ISA, function/class tables, constant pools |
| `rphp-compiler` | AST → bytecode (register allocation, closures, classes) |
| `rphp-hir`, `rphp-tokenizer`, `rphp-pcre2` | resolution/validation pass, byte-exact `token_get_all`, the PCRE2 binding |
| `rphp-stdlib` | native-function registry, feature-organized per extension |
| `rphp-ext-pdo`, `rphp-ext-dom` | `ext/pdo` (+ sqlite) and `ext/dom` + libxml + simplexml, as extension crates |
| `rphp-runtime` | the Tier-0 interpreter, call ABI, object/method dispatch |
| `rphp-embed` | the engine a SAPI embeds: interpreters, compile hook, the compiled-unit cache, CGI request binding |
| `rphp-sapi-cli`, `rphp-sapi-server`, `rphp-sapi-fcgi` + `tools/rphp` | the CLI, `-S` and FastCGI SAPIs and the `rphp` binary |
| `rphp-test`, `xtask` | the differential oracle, the `.phpt` runner, the Symfony ladder |

Dependencies point strictly downward; `rphp-value` is the foundation everything
agrees with.

---

## Testing & the differential oracle

The correctness gate is **differential testing against stock PHP 8.5**: every snippet
in [`examples/tier-a/`](examples/tier-a/) is run through both rPHP and the system
`php` and required to match byte-for-byte (`tools/rphp/tests/differential.rs`).
The comparison is skipped (not failed) when no `php` is on `PATH`, so the suite stays
green in PHP-less CI while remaining a real oracle locally.

```sh
cargo test --workspace                 # unit + end-to-end + differential
cargo test -p rphp --test differential # the tier-a snippets against php
cargo test -p rphp --test http         # rphp -S against php -S
cargo test -p rphp --test fcgi         # rphp -b against php-fpm
cargo xtask ladder                     # the Symfony rungs (needs the fixtures set up)
```

---

## Design docs

The authoritative design — value model, heap types, memory/GC, bytecode ISA,
interpreter, JIT, stdlib plan, runtime/SAPIs, testing — lives in
[`specs/base/`](specs/base/00-overview.md). Start with `00-overview.md`. Note that
the specs describe the *target* engine; the code lands it incrementally.

---

## License

Licensed under either of **MIT** or **Apache-2.0** at your option.

## Contributors

- **Dietmar Rietsch** <dietmar.rietsch@andthenext.at>
