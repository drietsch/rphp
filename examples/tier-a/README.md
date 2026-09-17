# Tier-A differential snippets

Small PHP programs that pin rphp to stock PHP 8.5 behaviour, one directory per
extension (or `lang/` for the language itself), one topic per file:

```
examples/tier-a/
  array/basics.php      array/byref.php
  string/basics.php     math/basics.php      math/fatal-intdiv-by-zero.php
  ctype/basics.php      json/basics.php      hash/basics.php      pcre/basics.php
  lang/objects.php      lang/inheritance.php lang/closures.php    lang/callables.php
  lang/dynamic-calls.php
  lang/fatal-undefined-function.php  lang/fatal-undefined-method.php  lang/fatal-modulo-by-zero.php
  divergences.toml
```

Every wave that adds functions adds a snippet exercising them (and snippets
that deliberately trigger warnings, exceptions and non-zero exit codes), so the
suite is the must-not-regress gate for the stdlib burn-down.

## How they are run

`cargo test -p rphp --test differential` (`tools/rphp/tests/differential.rs`)
walks `examples/tier-a/**/*.php` and, for each snippet, runs

* stock php with the **pinned oracle ini** —
  `php -n -d display_errors=1 -d log_errors=0 -d error_reporting=E_ALL
  -d html_errors=0 -d date.timezone=UTC -d precision=14 -d serialize_precision=-1
  -d memory_limit=-1 -d output_buffering=0 -d implicit_flush=1 -d short_open_tag=0
  -d zend.assertions=-1 <script>` — and
* the freshly built `rphp <script>`,

each as a real process in its own empty temporary working directory, with a
scrubbed environment (only `PATH`, `HOME`, `TMPDIR` are inherited; `LC_ALL=C`,
`TZ=UTC`), no stdin, and a timeout. `PHP_BIN` overrides which php is used;
when no php is found the differential test is **skipped**, not failed. A second,
php-independent test only checks that every snippet runs under rphp and exits
with the expected code.

The library behind it is `crates/rphp-test/src/differential/`.

## Comparison policy (ADR-008)

| channel | rule |
|---------|------|
| stdout  | byte-exact by default; against the `.expectf` template when one exists |
| stderr  | after stripping php's `PHP `-prefixed `log_errors` duplicates and after normalization |
| exit    | exact, always — never allowlistable |

Allowlist entries (below) normalize **both** sides of stdout and stderr before
the comparison, so the non-volatile part of the output is still checked. A
failure prints a unified diff around the first difference.

## Sidecars

* `<topic>.expectf` next to `<topic>.php`: a run-tests.php style template that
  **both** stdouts must match (`%d %i %f %x %s %S %a %A %w %e %c %r…%r`), used
  instead of byte equality. Both template and output are trimmed of
  surrounding whitespace first, as run-tests does.
* `<topic>.exit`: the exit code the snippet ends with (default 0), e.g. `255`
  for a snippet whose last statement throws. The differential test still
  compares php's and rphp's exit codes with each other; the sidecar only drives
  the php-independent smoke test.

## `divergences.toml`

Output that legitimately depends on the environment is not "matched away" by
hand; it is declared:

```toml
[[allow]]
snippet   = "math/basics.php"        # relative path or glob (*, **, ?)
category  = "float-format"           # from the closed set below
reason    = "shortest-round-trip dtoa differs between libc implementations"
normalize = ""                       # "" = category default,
                                     # "drop-lines:<regex>",
                                     # "regex:<pattern> => <replacement>"
```

The category set is **closed** (a name outside it fails loading, so the set can
only shrink deliberately):

| category | default normalizer |
|----------|--------------------|
| `float-format` | decimal/exponent literals → `%f` |
| `locale` | none — an explicit rule is required |
| `error-wording` | keeps the level and the thrown class, collapses the message/file/line to `%MSG%`, drops stack traces |
| `hash-order` | sorts output lines |
| `platform-value` | none — explicit rule required |
| `resource-id` | `resource(12)` → `resource(N)`, `Resource id #12` → `#N` |
| `object-id` | `object(Foo)#12` → `object(Foo)#N` |
| `tzdb-version` | `2025.2` / `2025b` → `%TZDB%` |
| `pid` | none — explicit rule required |
| `timing` | `12.5 ms`, `0.3s`, `7us` → `%TIMING%` |
| `tempnam-path` | paths under the system temp dir → `%TEMPNAM%` |
| `path` | the script's path, its directory and the cwd → `%PATH%` |

Every entry must cite a reason, an entry that matches no snippet fails the
suite, and the goal is to delete entries as rphp catches up — an entry is a
documented gap, never a hiding place.
