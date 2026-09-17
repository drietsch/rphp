//! `--emit=ast` snapshots: the v2 S-expression dump of two tier-a snippets,
//! pinned as golden files under `tests/snapshots/`. Regenerate with
//! `UPDATE_SNAPSHOTS=1 cargo test -p rphp-sapi-cli --test emit_ast` after an
//! intended printer or adapter change and review the diff.

use std::path::{Path, PathBuf};

use rphp_sapi_cli::{emit_ast_to_string, emit_tokens_to_string};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repo root")
}

fn check_snapshot(name: &str, actual: &str) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/snapshots/{name}"));
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
        std::fs::write(&path, actual).expect("write snapshot");
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "{}: {e} (run with UPDATE_SNAPSHOTS=1 to create)",
            path.display()
        )
    });
    assert!(
        expected == actual,
        "{} differs from the current output; run with UPDATE_SNAPSHOTS=1 to refresh after reviewing\n--- expected\n{expected}\n--- actual\n{actual}",
        path.display()
    );
}

fn ast_snapshot(rel: &str, snapshot: &str) {
    let path = repo_root().join("examples/tier-a").join(rel);
    let bytes = std::fs::read(&path).expect("tier-a snippet");
    let (tree, diags) = emit_ast_to_string(rel, &bytes);
    assert!(diags.is_empty(), "{rel}: {diags:?}");
    assert!(tree.starts_with("(program file=0\n"), "{tree}");
    check_snapshot(snapshot, &tree);
}

#[test]
fn emit_ast_objects_snapshot() {
    ast_snapshot("lang/objects.php", "lang_objects.ast");
}

#[test]
fn emit_ast_closures_snapshot() {
    ast_snapshot("lang/closures.php", "lang_closures.ast");
}

#[test]
fn emit_tokens_shape() {
    let out = emit_tokens_to_string(b"<?php\necho \"v=$x\\n\"; ?>\n");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], "T_OPEN_TAG(\"<?php\\n\") L1");
    assert_eq!(lines[1], "T_ECHO(\"echo\") L2");
    assert!(lines.contains(&"T_VARIABLE(\"$x\") L2"), "{out}");
    assert!(lines.contains(&"\";\" L2"), "{out}");
    assert_eq!(lines.last().copied(), Some("T_CLOSE_TAG(\"?>\\n\") L2"));
}
