//! Robustness: every file of the parser's negative corpus (php rejects each
//! of them) lowers without panicking. The HIR pass may add diagnostics of
//! its own; nothing about the result is asserted beyond termination and a
//! printable tree.

use std::path::{Path, PathBuf};

use rphp_hir::LowerOptions;
use rphp_intern::Interner;
use rphp_parser::{parse_v2, ParseOptions};
use rphp_source::SourceMap;

#[test]
fn negative_corpus_does_not_panic() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../rphp-parser/tests/negative");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "php"))
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    assert!(
        !files.is_empty(),
        "negative corpus missing at {}",
        dir.display()
    );
    for path in &files {
        let src = std::fs::read(path).unwrap();
        let mut sources = SourceMap::new();
        let fid = sources.add(path.display().to_string(), src.clone());
        let mut interner = Interner::new();
        let parsed = parse_v2(&src, ParseOptions::new(fid), &mut interner);
        let line_of = |s: rphp_span::Span| sources.get(s.file).line_col(s.lo).0;
        let mut opts = LowerOptions::new(&line_of).with_file(path.clone());
        opts.check_canonical = false;
        let (hir, _diags) = rphp_hir::lower(parsed.program, &mut interner, &opts);
        let _ = rphp_hir::print(&hir, &interner);
    }
}
