# php-src `.phpt` baselines

`cargo xtask phpt` runs slices of the php-src test corpus (a sparse checkout of
`php/php-src` at the tag in `tools/php-src/PIN`, fetched into the git-ignored
`vendor-php-src/` by `cargo xtask fetch-php-src` or `tools/php-src/checkout.sh`)
under `rphp` and scores them the way php-src's own `run-tests.php` would.

## `enabled/<slice>.toml`

One file per scored directory. The file name is the slice name used on the
command line (`--ext <slice>`).

| key        | type   | meaning |
|------------|--------|---------|
| `dir`      | string | directory relative to the php-src checkout, e.g. `ext/ctype/tests`, `ext/standard/tests/strings`, `Zend/tests` |
| `enabled`  | bool   | `false` keeps the file but excludes the slice from `--gate` and from the default run |
| `baseline` | int    | the PASS count the slice must reach; `--gate` fails when the observed count is lower, `--update-baseline` raises it and refuses to lower it |
| `total`    | int    | (written by `--update-baseline`) number of tests when the baseline was recorded, for context |
| `updated`  | string | (written by `--update-baseline`) date of the last ratchet |
| `notes`    | string | free text: what blocks the slice, what is expected to skip |

Only `PASS` counts toward the baseline. `XFAIL` (an `--XFAIL--` section or a
SKIPIF that prints `xfail …`) and `SKIP` are neutral; `FAIL`, `BORK`,
`TIMEOUT`, `CRASH` and `XPASS` are listed in the report.

## Commands

```sh
cargo xtask fetch-php-src                       # once; idempotent
cargo xtask phpt --ext ctype --ext json         # run two slices under target/debug/rphp
cargo xtask phpt --engine php --ext ctype       # validate the runner against stock php
cargo xtask phpt --dir vendor-php-src/ext/spl/tests --filter 'array_*' --list-failures
cargo xtask phpt --gate                         # every enabled slice vs its baseline (CI)
cargo xtask phpt --ext json --update-baseline   # ratchet after a stdlib wave
```

Reports land in `target/phpt-report/<slice>.md` and `.json`; results are cached
in `target/phpt-cache/` keyed by test content, support files in the test's
directory, and the engine binary (`--no-cache` to bypass).

## What is faithful to run-tests.php

Section parsing (all sections of 8.5.0, `===DONE===`, FILEEOF, `*_EXTERNAL`),
`--INI--` (`{PWD}`/`{TMP}`/`{ENV:X}`/`{MAIL:x}`), `--ENV--`, `--ARGS--`,
`--STDIN--`, `--EXTENSIONS--`, `--SKIPIF--` (`skip`/`info`/`warn`/`xfail`/
`xleak`/`flaky`/`nocache` prefixes, invalid output = BORK), `--CLEAN--` (output
= BORK), `--XFAIL--`, `--FLAKY--` (one retry), `--CAPTURE_STDIO--`, CGI shaping
(`REDIRECT_STATUS`, `QUERY_STRING`, `REQUEST_METHOD`, `CONTENT_TYPE`,
`CONTENT_LENGTH`, `HTTP_COOKIE`, `SCRIPT_FILENAME`, body on stdin; header
stripping and `--EXPECTHEADERS--` when a `php-cgi` sits next to `php`), the
EXPECTF wildcard table, output trimming and CRLF normalisation, `Termsig`
crashes, the 60 s timeout, the flaky-output retry list.

Not supported (reported as SKIP): `--REDIRECTTEST--`, `--PHPDBG--`,
`--GZIP_POST--`/`--DEFLATE_POST--`. `--CONFLICTS--` tests run sequentially
after the parallel batch instead of per conflict key. Under `--engine rphp`
the `-d` flags from `--INI--` are passed although the CLI does not accept them
yet; CGI tests run with the CLI binary and the shaped environment.
