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

**Language**
- Scalars (`null`, `bool`, `int` full `i64`, `float` `f64`) with PHP 8 semantics —
  arithmetic with int→float overflow promotion, the PHP 8 comparison rules, lenient
  numeric-string coercion, and `%.14G` float formatting verified against stock PHP.
- Byte-safe **strings** (binary-safe `echo`, concatenation, C-style + `$var`/`{$var}`
  interpolation) and copy-on-write **arrays** (ordered int/string keys, literals,
  indexing, append, `foreach`).
- Control flow: `if`/`else`/`else if`, `while`, `foreach` (with optional key),
  `return`; short-circuiting `&&`/`||`.
- **Functions** — declarations (forward references resolve), recursion, by-reference
  builtin parameters.
- **Closures & arrow functions** — first-class, by-value capture (`use (...)` /
  auto-capture), direct dynamic calls `$f(...)`, higher-order builtins.
- **Objects & classes** *(newest slice)* — class declarations, properties with
  constant defaults, constructors, methods, `$this`, method chaining, **reference
  semantics**, identity (`===`), and `json_encode` over public properties.
  (Inheritance, visibility enforcement, statics, `::`, magic methods, and
  `instanceof` are not in yet — see [the coverage dashboard](crates/rphp-stdlib/COVERAGE.md).)

**Standard library** — ~180 registry entries across `ctype`, `math`, `string`,
`array` (incl. by-reference sort/mutators and higher-order `array_map`/`usort`/…),
`json`, `hash`, and `pcre` (over PCRE2). Each is differentially tested against PHP
8.5. The per-extension burn-down and known divergences are tracked in
[`crates/rphp-stdlib/COVERAGE.md`](crates/rphp-stdlib/COVERAGE.md).

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
| `rphp-stdlib` | native-function registry, feature-organized per extension |
| `rphp-runtime` | the Tier-0 interpreter, call ABI, object/method dispatch |
| `rphp-sapi-cli` + `tools/rphp` | the CLI SAPI and the `rphp` binary |

Dependencies point strictly downward; `rphp-value` is the foundation everything
agrees with.

---

## Testing & the differential oracle

The correctness gate is **differential testing against stock PHP 8.5**: every snippet
in [`examples/tier-a/`](examples/tier-a/) is run through both rPHP and the system
`php` and required to match byte-for-byte (`crates/rphp-sapi-cli/tests/differential.rs`).
The comparison is skipped (not failed) when no `php` is on `PATH`, so the suite stays
green in PHP-less CI while remaining a real oracle locally.

```sh
cargo test --workspace          # unit + end-to-end + differential
cargo test -p rphp-sapi-cli --test differential
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
