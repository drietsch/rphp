//! `rphp -l` / `--lint`: parse-only mode with php's exit codes (0 / 255).

use std::path::{Path, PathBuf};

use rphp_sapi_cli::{lint, run};

fn write_temp(name: &str, contents: &[u8]) -> PathBuf {
    let mut path = std::env::temp_dir();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    path.push(format!(
        "rphp_lint_{}_{}_{}.php",
        std::process::id(),
        nanos,
        name
    ));
    std::fs::write(&path, contents).expect("write temp file");
    path
}

fn run_with(flag: &str, path: &Path) -> i32 {
    run(vec![flag.to_string(), path.to_string_lossy().into_owned()])
}

#[test]
fn lint_flag_accepts_valid_file_with_exit_0() {
    let path = write_temp("ok", b"<?php echo 1 + 2;");
    let short = run_with("-l", &path);
    let long = run_with("--lint", &path);
    let _ = std::fs::remove_file(&path);
    assert_eq!(short, 0);
    assert_eq!(long, 0);
}

#[test]
fn lint_flag_rejects_syntax_error_with_php_exit_255() {
    let path = write_temp("bad", b"<?php echo 1 +;");
    let short = run_with("-l", &path);
    let long = run_with("--lint", &path);
    let _ = std::fs::remove_file(&path);
    assert_eq!(short, 255);
    assert_eq!(long, 255);
}

#[test]
fn lint_does_not_run_the_script() {
    // A runtime fault (undefined function) is not a syntax error: php -l
    // reports "No syntax errors detected" and exits 0.
    let path = write_temp("runtime", b"<?php undefined_function_xyz();");
    let code = run_with("-l", &path);
    let _ = std::fs::remove_file(&path);
    assert_eq!(code, 0);
}

#[test]
fn lint_library_entry_reports_rendered_diagnostics() {
    assert_eq!(lint("ok.php", b"<?php echo 1;"), Ok(()));
    let lines = lint("bad.php", b"<?php\necho 1 +;").unwrap_err();
    assert!(!lines.is_empty());
    assert!(lines[0].contains("RPHP_E"), "{}", lines[0]);
    assert!(lines[0].contains("bad.php:2:"), "{}", lines[0]);
}

#[test]
fn lint_cannot_be_combined_with_emit() {
    let path = write_temp("emit", b"<?php echo 1;");
    let code = run(vec![
        "-l".to_string(),
        "--emit=ast".to_string(),
        path.to_string_lossy().into_owned(),
    ]);
    let _ = std::fs::remove_file(&path);
    assert_eq!(code, 2);
}

#[test]
fn lint_missing_file_exits_1() {
    assert_eq!(
        run(vec![
            "-l".to_string(),
            "/nonexistent/rphp_lint_missing.php".to_string()
        ]),
        1
    );
}
