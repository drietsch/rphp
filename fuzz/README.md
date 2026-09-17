# rphp fuzz targets

Coverage-guided fuzzing of the front end with [`cargo-fuzz`](https://github.com/rust-fuzz/cargo-fuzz)
(libFuzzer). This directory is its own cargo workspace (the root workspace
lists it under `exclude`), so it never affects `cargo build --workspace`.

| Target           | What it checks                                                                                   |
|------------------|--------------------------------------------------------------------------------------------------|
| `parse_bytes`    | lexer + parser never panic and always terminate on arbitrary bytes; every diagnostic renders     |
| `tokenize_bytes` | `rphp_tokenizer::tokenize` is lossless in both `short_open_tag` modes (tokens tile the input)    |

`tokenize_bytes` is gated behind the `tokenizer` cargo feature until F5 lands
the `tokenize(src, Options) -> Vec<RawToken { id, lo, hi, line }>` API; once it
exists, make the dependency unconditional in `Cargo.toml` and drop the
`required-features` line.

## Setup

```sh
cargo install cargo-fuzz        # once; needs the nightly from rust-toolchain.toml (picked up automatically)
cd fuzz
./seed-corpus.sh                # seeds corpus/<target>/ from examples/ and the tokenizer test corpus
```

## Run

```sh
cd fuzz
cargo fuzz build                                   # compile every target (sanity check, also what CI does)
cargo fuzz run parse_bytes -- -timeout=10          # fuzz until Ctrl-C; -timeout catches hangs
cargo fuzz run parse_bytes -- -max_total_time=300  # bounded run (CI's nightly smoke)
cargo fuzz run parse_bytes -j 8 -- -timeout=10     # parallel jobs
cargo fuzz build --features tokenizer              # once F5 lands
cargo fuzz run tokenize_bytes --features tokenizer -- -timeout=10
```

Crashes and timeouts land in `artifacts/<target>/`; reproduce and minimise with

```sh
cargo fuzz run parse_bytes artifacts/parse_bytes/crash-<sha>
cargo fuzz tmin parse_bytes artifacts/parse_bytes/crash-<sha>
cargo fuzz fmt parse_bytes artifacts/parse_bytes/crash-<sha>   # print the input as a Rust byte string
```

Turn every minimised crash into a regression test next to the parser
(`crates/rphp-parser/src/lib.rs` `mod tests`) before fixing it.

## Corpus

`corpus/`, `artifacts/` and `target/` are git-ignored. `seed-corpus.sh` copies
`examples/**/*.php` and `crates/rphp-tokenizer/tests/**/*.php` into every
target's corpus, content-addressed so reseeding is idempotent; pass extra
directories to pull in more (`./seed-corpus.sh /path/to/vendor/symfony/console`).
For a richer start, `cargo fuzz cmin parse_bytes` shrinks a big corpus to the
inputs that add coverage.

## Notes

- libFuzzer needs a sanitizer-capable nightly; the repo's pinned nightly is
  fine. `cargo fuzz` builds with `-Zsanitizer=address` by default; pass
  `-s none` to skip it (faster, still catches panics).
- Deeply nested input can overflow the recursive-descent parser's stack before
  the fuzzer reports anything useful; `-rss_limit_mb` and `-max_len=4096`
  keep runs focused on syntax rather than recursion depth.
