//! The `NodeKind` coverage contract (ADR-025): every node kind of the pinned
//! `mago-syntax` is listed in `rphp_parser::adapter::COVERED`, and every
//! listed kind exists.
//!
//! `NodeKind` derives strum's `EnumIter`, but the `IntoEnumIterator` trait is
//! not re-exported by mago and `strum` is not a dependency of this crate, so
//! the enumeration comes from the crate's own source: the registry copy of
//! `src/cst/node.rs` for the version pinned in the workspace manifest (which
//! must match [`rphp_parser::adapter::MAGO_SYNTAX_VERSION`]). Each name is
//! then checked against the compiled enum through `FromStr` / `Debug`, so a
//! stale table fails even if the source lookup does not.

use std::path::{Path, PathBuf};
use std::str::FromStr;

use mago_syntax::cst::NodeKind;
use rphp_parser::adapter::{coverage_of, Coverage, COVERED, MAGO_SYNTAX_VERSION};

/// The `mago-syntax` version pinned in the workspace manifest.
fn pinned_version() -> String {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml");
    let text = std::fs::read_to_string(&manifest).expect("workspace Cargo.toml");
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("mago-syntax") {
            let rest = rest.trim_start();
            if let Some(rest) = rest.strip_prefix('=') {
                let v = rest.trim().trim_matches('"').trim_start_matches('=');
                return v.to_string();
            }
        }
    }
    panic!("mago-syntax pin not found in {}", manifest.display());
}

/// The registry source of `src/cst/node.rs` for `version`, if extracted.
fn registry_node_rs(version: &str) -> Option<PathBuf> {
    let home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cargo")))?;
    let src = home.join("registry").join("src");
    for index in std::fs::read_dir(src).ok()?.flatten() {
        let candidate = index
            .path()
            .join(format!("mago-syntax-{version}"))
            .join("src/cst/node.rs");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// The variant names of `pub enum NodeKind { ... }` in `node.rs`.
fn variants_from_source(text: &str) -> Vec<String> {
    let start = text
        .find("pub enum NodeKind {")
        .expect("NodeKind enum in node.rs");
    let body = &text[start..];
    let end = body.find("\n}").expect("enum end");
    body[..end]
        .lines()
        .skip(1)
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("//") && !l.starts_with('#'))
        .map(|l| l.trim_end_matches(',').to_string())
        .collect()
}

#[test]
fn pinned_version_matches_adapter_constant() {
    assert_eq!(
        pinned_version(),
        MAGO_SYNTAX_VERSION,
        "bump adapter::MAGO_SYNTAX_VERSION together with the workspace pin (ADR-028)"
    );
}

#[test]
fn covered_names_exist_and_are_unique() {
    let mut seen = std::collections::BTreeSet::new();
    for (name, _) in COVERED {
        let kind = NodeKind::from_str(name)
            .unwrap_or_else(|_| panic!("COVERED lists `{name}`, which is not a NodeKind"));
        assert_eq!(format!("{kind:?}"), *name);
        assert!(seen.insert(*name), "`{name}` listed twice");
    }
    assert_eq!(coverage_of("Program"), Some(Coverage::Mapped));
    assert_eq!(coverage_of("PlaceholderArgument"), Some(Coverage::Rejected));
    assert_eq!(coverage_of("Error"), Some(Coverage::Recovery));
    assert_eq!(coverage_of("NoSuchKind"), None);
}

#[test]
fn every_node_kind_is_covered() {
    let version = pinned_version();
    let Some(path) = registry_node_rs(&version) else {
        eprintln!(
            "note: mago-syntax {version} source not found under $CARGO_HOME/registry; \
             only the name round-trip was checked"
        );
        return;
    };
    let text = std::fs::read_to_string(&path).expect("read node.rs");
    let variants = variants_from_source(&text);
    assert!(variants.len() > 200, "suspiciously short variant list: {}", variants.len());
    let missing: Vec<&String> = variants
        .iter()
        .filter(|v| coverage_of(v).is_none())
        .collect();
    assert!(
        missing.is_empty(),
        "NodeKind variants not in adapter::COVERED (map or reject them): {missing:?}"
    );
    let stale: Vec<&&str> = COVERED
        .iter()
        .map(|(n, _)| n)
        .filter(|n| !variants.iter().any(|v| v == *n))
        .collect();
    assert!(stale.is_empty(), "adapter::COVERED lists removed kinds: {stale:?}");
    assert_eq!(variants.len(), COVERED.len());
    for v in &variants {
        let kind = NodeKind::from_str(v).expect("source variant compiles");
        assert_eq!(format!("{kind:?}"), *v);
    }
}
