//! The negative corpus: every file under `tests/negative/` is rejected by
//! `php -l` (the corpus job's `--negative` mode checks that direction) and
//! must be rejected by the adapter too — with an error the front end
//! attributes to the file, never a panic.
//!
//! Most files are conformance cases (syntax mago 1.49 accepts and PHP 8.5.0
//! does not); the rest pin mago's own parse errors so a mago upgrade that
//! starts accepting them is noticed.

use std::path::Path;

use rphp_intern::Interner;
use rphp_parser::{parse_v2, ParseOptions};
use rphp_span::FileId;

fn files() -> Vec<std::path::PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/negative");
    let mut out: Vec<_> = std::fs::read_dir(&dir)
        .expect("tests/negative")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "php"))
        .collect();
    out.sort();
    out
}

#[test]
fn every_negative_file_is_rejected() {
    let files = files();
    assert!(files.len() > 300, "negative corpus looks truncated: {}", files.len());
    let mut accepted = Vec::new();
    for path in &files {
        let src = std::fs::read(path).unwrap();
        let mut interner = Interner::new();
        let parsed = parse_v2(&src, ParseOptions::new(FileId(7)), &mut interner);
        let errors: Vec<_> = parsed.diagnostics.iter().filter(|d| d.is_error()).collect();
        if errors.is_empty() {
            accepted.push(path.file_name().unwrap().to_string_lossy().into_owned());
            continue;
        }
        for d in errors {
            let label = d.primary.as_ref().expect("every error carries a span");
            assert_eq!(label.span.file, FileId(7), "{}", path.display());
            assert!(
                (label.span.hi as usize) <= src.len(),
                "{}: span {:?} outside the source",
                path.display(),
                label.span
            );
            assert!(!d.message.is_empty(), "{}", path.display());
        }
    }
    assert!(
        accepted.is_empty(),
        "accepted by the adapter but rejected by php -l: {accepted:?}"
    );
}

/// Every prefix of every negative file parses without panicking (mago's
/// recovery plus the adapter's placeholders must survive any cut).
#[test]
fn prefixes_never_panic() {
    for path in files() {
        let src = std::fs::read(&path).unwrap();
        for cut in 0..=src.len() {
            let mut interner = Interner::new();
            let _ = parse_v2(&src[..cut], ParseOptions::new(FileId(0)), &mut interner);
        }
    }
}
