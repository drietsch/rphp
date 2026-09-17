//! Debug helper: print the token stream of one or more files, one token per
//! line as `id<TAB>line<TAB>len<TAB>name<TAB>text` (text escaped).
//!
//! * `--php-shape`: print exactly what `tests/dump_tokens.php` prints, so the
//!   two can be diffed directly:
//!   ```text
//!   cargo run -p rphp-tokenizer --example dump_tokens -- --php-shape f.php > a
//!   echo f.php | php -n -d short_open_tag=0 crates/rphp-tokenizer/tests/dump_tokens.php > b
//!   diff a b
//!   ```
//! * `--count`: only print `tokens<TAB>bytes<TAB>path` per file.
//! * `--bench`: read all files into memory, then report tokenize() throughput.
//! * `--short-open-tag`: enable `short_open_tag`.

use rphp_tokenizer::{token_name, tokenize, Options};
use std::io::{BufWriter, Write};

fn main() {
    let mut php_shape = false;
    let mut count = false;
    let mut bench = false;
    let mut opts = Options::default();
    let mut files = Vec::new();
    for a in std::env::args().skip(1) {
        match a.as_str() {
            "--php-shape" => php_shape = true,
            "--count" => count = true,
            "--bench" => bench = true,
            "--short-open-tag" => opts.short_open_tag = true,
            _ => files.push(a),
        }
    }
    if files.is_empty() {
        eprintln!("usage: dump_tokens [--php-shape|--count|--bench] [--short-open-tag] <file>...");
        std::process::exit(2);
    }
    if bench {
        // Read everything first, then time tokenize() alone over three passes.
        let srcs: Vec<Vec<u8>> = files
            .iter()
            .map(|f| std::fs::read(f).expect("read"))
            .collect();
        let bytes: usize = srcs.iter().map(Vec::len).sum();
        let mut tokens = 0usize;
        let t0 = std::time::Instant::now();
        for _ in 0..3 {
            tokens = srcs.iter().map(|s| tokenize(s, opts).len()).sum();
        }
        let secs = t0.elapsed().as_secs_f64() / 3.0;
        println!(
            "{} files, {} bytes, {tokens} tokens: {secs:.3} s per pass = {:.1} MB/s, {:.0} ns/token",
            srcs.len(),
            bytes,
            bytes as f64 / secs / 1e6,
            secs * 1e9 / tokens as f64
        );
        return;
    }
    let stdout = std::io::stdout();
    let mut out = BufWriter::new(stdout.lock());
    for f in files {
        let src = std::fs::read(&f).unwrap_or_else(|e| {
            eprintln!("{f}: {e}");
            std::process::exit(1);
        });
        let toks = tokenize(&src, opts);
        if count {
            writeln!(out, "{}\t{}\t{f}", toks.len(), src.len()).unwrap();
            continue;
        }
        if php_shape {
            writeln!(out, "== {f}").unwrap();
        }
        for t in toks {
            if php_shape {
                writeln!(out, "{}\t{}\t{}", t.id, t.line, t.len()).unwrap();
            } else {
                let name = token_name(t.id).map_or_else(
                    || format!("'{}'", (t.id as u8 as char).escape_default()),
                    str::to_string,
                );
                let text = String::from_utf8_lossy(t.text(&src))
                    .escape_debug()
                    .to_string();
                writeln!(out, "{}\t{}\t{}\t{}\t{}", t.id, t.line, t.len(), name, text).unwrap();
            }
        }
    }
    out.flush().unwrap();
}
