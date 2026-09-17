//! Lower every parser snapshot source and every tier-a example: no panics,
//! no diagnostics beyond the expected ones, and a canonical result.

use std::path::{Path, PathBuf};

use rphp_hir::LowerOptions;
use rphp_intern::Interner;
use rphp_parser::{parse_v2, ParseOptions};
use rphp_source::SourceMap;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn php_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(php_files(&p));
            } else if p.extension().is_some_and(|x| x == "php") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// Diagnostics that a known input legitimately produces (php prints the
/// same notice).
fn expected(path: &Path, message: &str) -> bool {
    let _ = path;
    message.starts_with("The use statement with non-compound name")
}

#[test]
fn lowers_parser_snapshots_and_tier_a() {
    let root = repo_root();
    let mut files = php_files(&root.join("crates/rphp-parser/tests/snapshots"));
    files.extend(php_files(&root.join("examples/tier-a")));
    assert!(files.len() >= 20, "corpus too small: {}", files.len());
    let mut failures = Vec::new();
    for path in &files {
        let src = std::fs::read(path).unwrap();
        let mut sources = SourceMap::new();
        let fid = sources.add(path.display().to_string(), src.clone());
        let mut interner = Interner::new();
        let parsed = parse_v2(&src, ParseOptions::new(fid), &mut interner);
        if !parsed.diagnostics.is_empty() {
            // Not a php -l-clean file: not this test's concern.
            continue;
        }
        let line_of = |s: rphp_span::Span| sources.get(s.file).line_col(s.lo).0;
        let opts = LowerOptions::new(&line_of).with_file(path.clone());
        let (hir, diags) = rphp_hir::lower(parsed.program, &mut interner, &opts);
        for d in &diags {
            if !expected(path, &d.message) {
                failures.push(format!("{}: {} {}", path.display(), d.code, d.message));
            }
        }
        for v in hir.validate_canonical() {
            failures.push(format!("{}: non-canonical {}", path.display(), v.what));
        }
        // The printer must accept the result.
        let _ = rphp_hir::print(&hir, &interner);
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
