//! Development aid until the CLI's `--emit=hir` is wired: parse a file,
//! lower it and print the HIR (or the AST with `--ast`) plus diagnostics.
//!
//! `cargo run -p rphp-hir --example hir_dump -- [--ast] [--spans] file.php`

use std::path::PathBuf;

use rphp_ast::v2::pretty;
use rphp_intern::Interner;
use rphp_parser::{parse_v2, ParseOptions};
use rphp_source::SourceMap;
use rphp_span::FileId;

fn main() {
    let mut ast_only = false;
    let mut spans = false;
    let mut path: Option<PathBuf> = None;
    for a in std::env::args().skip(1) {
        match a.as_str() {
            "--ast" => ast_only = true,
            "--spans" => spans = true,
            other => path = Some(PathBuf::from(other)),
        }
    }
    let path = path.expect("usage: hir_dump [--ast] [--spans] file.php");
    let src = std::fs::read(&path).expect("read source");
    let abs = std::fs::canonicalize(&path).unwrap_or(path.clone());
    let mut sm = SourceMap::new();
    let file = sm.add(abs.display().to_string(), src.clone());
    let mut interner = Interner::new();
    let opts = ParseOptions {
        file,
        path: Some(&abs),
        short_open_tag: true,
    };
    let parsed = parse_v2(&src, opts, &mut interner);
    let popts = pretty::Options { spans };
    for d in &parsed.diagnostics {
        println!("{}", d.render(&sm));
    }
    if ast_only {
        println!("{}", pretty::print_with(&parsed.program, &interner, popts));
        return;
    }
    let line_of = |s: rphp_span::Span| sm.get(FileId(s.file.0)).line_col(s.lo).0;
    let lopts = rphp_hir::LowerOptions::new(&line_of).with_file(abs.clone());
    let (hir, diags) = rphp_hir::lower(parsed.program, &mut interner, &lopts);
    for d in &diags {
        println!("{}", d.render(&sm));
    }
    println!("{}", rphp_hir::print_with(&hir, &interner, popts));
}
