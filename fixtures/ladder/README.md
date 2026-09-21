# Ladder fixtures

Real-world oracle programs that gate the Symfony roadmap (plan track T4,
ADR-034). Each directory is a Composer project with `composer.json` +
`composer.lock` committed and `vendor/` + `var/` git-ignored;
`tools/fixtures/setup.sh` installs them with the stock php (`composer install`,
never `update`). `ladder.toml` in each fixture lists the rungs it serves and
the commands `cargo xtask ladder` runs under both `php` and `rphp`.

| Fixture | Rungs | Source |
|---|---|---|
| `L1-skeleton` | L1, L3, L6a, L7 | `composer create-project symfony/skeleton` (v8.1.99 → Symfony 8.1.7) |
| `L8-demo` | L8, L8http | `composer create-project symfony/symfony-demo` (Symfony 8.1, Doctrine ORM over the bundled SQLite `data/database.sqlite`, Twig, security, forms, translations, profiler); `.env.dev` carries the demo's placeholder `APP_SECRET`; `bin/console sass:build` once, its `var/sass` output rides along (`var_keep`) |

## Running

```text
tools/fixtures/setup.sh                       # once: composer install with stock php
cargo build -p rphp                           # xtask never builds rphp implicitly
cargo xtask ladder --list                     # fixtures, rungs, vendor state
cargo xtask ladder --rung L1                  # php vs rphp, exit 1 on divergence
cargo xtask ladder --rung L1 --only php --keep
cargo xtask ladder --fixture L1-skeleton --allow-failures --report json
```

Flags: `--rung <id>` (repeatable), `--fixture <name>`, `--only php|rphp`,
`--php`/`--rphp <path>` (defaults `$PHP_BIN`/`php` on PATH and
`$RPHP_BIN`/`target/debug/rphp`), `--timeout <s>` (120), `--jobs <n>` (rungs in
parallel; commands always sequential), `--keep`, `--allow-failures`,
`--report text|json`, `--fixtures-dir`, `--target-dir`. Exit status 0 = every
compared command and artifact agrees, 1 = divergence or runner error, 2 = setup
problem (no php/rphp, `vendor/` missing, invalid `ladder.toml`, unknown rung).
`target/ladder-report.json` is always written.

## Execution model (ADR-034)

For each selected rung and each side the runner builds a **fresh working
copy** of the fixture: every entry is copied except `vendor/` (hard-linked file
by file — PHP resolves `__DIR__` through symlinks, so a symlinked `vendor/`
would make Composer's `$baseDir` point back at the original fixture), `var/`
(created empty, but for the `[fixture].var_keep` entries), `node_modules/` and
`.git/` (skipped). Both sides run at the
**same path**, `target/ladder/<fixture>/<rung>/work/`, which is renamed to
`php/` resp. `rphp/` when the side is done, so path *lengths* inside serialized
data and padded console tables cannot differ between the engines. That path is
rendered as `%FIXTURE%` in diffs and before artifacts are compared.

Commands run in order with the working copy as cwd:

* php: `php -n -d <pinned oracle ini…> <script> <args>` (`rphp-test`'s
  `PHP_INI`: `display_errors=1`, `error_reporting=E_ALL`, `date.timezone=UTC`,
  `precision=14`, `serialize_precision=-1`, …);
* rphp: `rphp <script> <args>`;
* environment scrubbed to `PATH`, `HOME`, `TMPDIR` plus `LC_ALL=C`, `TZ=UTC`,
  plus the rung's / command's `env`; stdin is `/dev/null` unless `stdin` is set.

`bin/console` keeps its `#!/usr/bin/env php` shebang and is invoked as
`php bin/console …` / `rphp bin/console …` (both CLIs skip the shebang line).

Each side's raw outputs are dumped to `<side>/out/<n>.{stdout,stderr,exit}`
(`n` = 1-based command index) and survive cleanup; the rest of the working copy
is deleted unless `--keep`.

Comparison per command uses `rphp-test`'s `Policy`: stdout byte-exact, stderr
after stripping php's `log_errors` duplicates and normalizing, exit code exact;
a timeout on either side is an `exit` mismatch. Artifacts are compared
byte-for-byte after `%FIXTURE%` normalization (a pattern matching nothing on
the php side, a file missing under rphp, or a file only rphp produced all
fail). HTTP rungs (`http = true`) start `php -S` and `rphp -S` on the
working copy's `docroot` (ephemeral ports) and compare each request's raw
response — status line, headers, body — the same way, with the port and the
`Date` header as placeholders.

## `ladder.toml` schema

```toml
[fixture]
name      = "L1-skeleton"          # display name (normally the directory name)
setup     = "composer install …"   # informational; what tools/fixtures/setup.sh runs
php_min   = "8.4.1"                # refuse to run against an older stock php
allowlist = "ladder/divergences.toml"  # optional; rphp-test allowlist for every rung
var_keep  = ["sass"]               # optional; entries of var/ the working copy keeps
                                   # (build products no rung command makes)

[[rung]]
id        = "L6a"                  # unique word; selected with --rung
title     = "FrameworkBundle micro kernel"
env       = { COLUMNS = "120" }    # optional; for every command of the rung
stdin     = "…"                    # optional; for every command without its own
allow     = ["timing", "path"]     # optional; closed-set categories with a default
                                   # normalizer, applied to every command
allowlist = "ladder/other.toml"    # optional; overrides [fixture].allowlist
commands  = [
  "bin/console about --no-ansi",   # string form: script (fixture-relative) + args;
                                   # shell-style quoting ('…', "…", \ ) is honoured
  { run = "bin/console cache:clear --no-ansi",
    expect_exit = 0,               # both sides must exit with this (default: rphp
                                   # must match php)
    allow = ["timing"],            # extra categories for this command
    stdin = "y\n",                 # this command's stdin
    env = { APP_DEBUG = "0" } },   # on top of the rung's env
]
artifacts = ["var/cache/dev/*Container.php"]  # optional; globs, byte-identical after
                                              # all commands ran (vendor/ only searched
                                              # when the pattern starts with `vendor`)

[[rung]]                           # HTTP rung (L7, L8http): `php -S` vs `rphp -S`
id       = "L7"
http     = true
docroot  = "public"
requests = ["GET /", "GET /nonexistent"]
```

Unknown keys are rejected. Inline `allow` accepts only categories that have a
built-in normalizer (`float-format`, `error-wording`, `hash-order`,
`resource-id`, `object-id`, `tzdb-version`, `timing`, `tempnam-path`, `path`);
`locale`, `platform-value` and `pid` need an explicit `normalize` rule, so they
must live in the allowlist file.

## Allowlist files

The fixture allowlist reuses `rphp-test`'s format (`[[allow]]` entries with
`snippet`, `category`, `reason`, `normalize`; see
`crates/rphp-test/src/differential/allowlist.rs`). The **name** an entry
matches against is `<rung id>/<command line>` exactly as written in
`ladder.toml`, e.g. `L6a/bin/console about --no-ansi`; globs work
(`L6a/bin/console about*`, `*/bin/console *`). Normalizers run over both sides,
so a rule never hides the part of the output it does not touch.

### L3 / L6a: environment-dependent lines

`bin/console list`, `list --format=json` and `help cache:clear` (L3) are fully
deterministic once the terminal size is pinned (`COLUMNS`/`LINES` in the rung's
`env`; without it symfony/console shells out to `stty`). `cache:clear`,
`debug:container`, `debug:router`, `debug:autowiring` and `lint:container`
(L6a) print no host-dependent values with `--no-ansi`.

`bin/console about` (L6a) does; `L1-skeleton/ladder/divergences.toml`
normalizes exactly these spans (category `platform-value`, explicit rules):

| Line | Volatile span | Rule |
|---|---|---|
| `End of maintenance   01/2027 (in +136 days)` | the `(in +N days)` / `(expired)` tail | `(…)` → `(%WHEN%)` |
| `End of life          01/2027 (in +136 days)` | same | same |
| `Cache directory      ./var/cache/dev (563 KiB)` | the size (`.meta`/log files are not artifacts) | `(…)` → `(%SIZE%)` |
| `Build directory` / `Share directory` / `Log directory` | same | same |
| `Timezone             UTC (2026-09-17T04:23:57+00:00)` | the timestamp | `(…)` → `(%NOW%)` |
| `OPcache` / `APCu` / `Xdebug` / `Intl locale` | host php build's extensions | line dropped |

Kept under byte comparison on purpose: the Symfony `Version v8.1.7` line, the
whole Kernel section (`Type`, `Environment`, `Debug`, `Charset`, the directory
paths), `PHP > Version` (rphp must report `8.5.0`) and `Architecture 64 bits`.

L6a's artifact gate is the dumped container: `var/cache/dev/Container<hash>/*.php`
(the container class plus one file per service, 106 files; the hash is derived
from the container's content, so the directory name is stable) and the preload
file. The wrapper `var/cache/dev/App_KernelDevDebugContainer.php` is *not*
listed: it embeds `container.build_time` (a timestamp) and a `build_id` derived
from it, so it differs even between two stock-php runs. Everything else the
kernel writes (`.meta`, `.ser`, `url_*_routes.php`) is deterministic too.
`cargo xtask ladder --rung L3 --rung L6a --rphp "$(command -v php)"` (php vs
php) must be fully green — that is how a new rule or artifact is validated.

## Adding a fixture

1. `composer create-project …` (or hand-written `composer.json`) under
   `fixtures/ladder/L<n>-<name>/`; commit `composer.json` + `composer.lock`
   (never `vendor/`, `var/`).
2. Write `ladder.toml`; put helper scripts under `ladder/` inside the fixture.
   Every command must run cleanly and deterministically under stock php in a
   fresh copy (`cargo xtask ladder --fixture <name> --only php` shows the raw
   outputs under `target/ladder/<name>/<rung>/php/out/`).
3. Add environment-dependent spans to a fixture-local `ladder/divergences.toml`
   with one closed category and a reason per rule — never to hide engine bugs.
4. Run `tools/fixtures/setup.sh`, then `cargo xtask ladder --fixture <name>`.
