//! End-to-end tests driving the full lexer -> parser -> compiler -> runtime
//! pipeline through the CLI SAPI.

use std::path::PathBuf;

use rphp_sapi_cli::{eval_to_string, run};

/// Evaluate source, asserting it succeeds, and return its stdout.
fn eval_ok(src: &[u8]) -> String {
    eval_to_string(src).unwrap_or_else(|e| panic!("expected success, got error:\n{e}"))
}

#[test]
fn arithmetic_precedence() {
    assert_eq!(eval_ok(b"<?php echo 1 + 2 * 3;"), "7");
}

#[test]
fn pow_is_right_associative() {
    // 2 ** (3 ** 2) == 2 ** 9 == 512
    assert_eq!(eval_ok(b"<?php echo 2 ** 3 ** 2;"), "512");
}

#[test]
fn variables_and_assignment() {
    assert_eq!(eval_ok(b"<?php $x = 5; $y = 10; echo $x + $y;"), "15");
}

#[test]
fn if_else_branch() {
    // 3 <=> 2 == 1 (truthy) => the then-branch runs.
    assert_eq!(eval_ok(b"<?php if (3 <=> 2) echo 1; else echo 2;"), "1");
}

#[test]
fn while_loop_sums_one_to_five() {
    assert_eq!(
        eval_ok(b"<?php $i = 1; $s = 0; while ($i <= 5) { $s = $s + $i; $i = $i + 1; } echo $s;"),
        "15"
    );
}

#[test]
fn function_definition_and_call() {
    assert_eq!(
        eval_ok(b"<?php function add($a, $b) { return $a + $b; } echo add(3, 4);"),
        "7"
    );
}

#[test]
fn recursion_factorial() {
    assert_eq!(
        eval_ok(b"<?php function f($n) { if ($n <= 1) return 1; return $n * f($n - 1); } echo f(5);"),
        "120"
    );
}

#[test]
fn division_by_zero_is_an_error() {
    let err = eval_to_string(b"<?php echo 1 / 0;").unwrap_err();
    assert!(
        err.contains("Division by zero"),
        "expected a division-by-zero error, got: {err}"
    );
}

#[test]
fn syntax_error_reports_diagnostic() {
    // A missing right-hand side is a parse error; it must not panic and must
    // surface a rendered diagnostic rather than running.
    let err = eval_to_string(b"<?php $x = ;").unwrap_err();
    assert!(!err.is_empty(), "expected a rendered diagnostic");
}

#[test]
fn undefined_function_reports_diagnostic() {
    let err = eval_to_string(b"<?php echo nope();").unwrap_err();
    assert!(
        err.contains("undefined function"),
        "expected an undefined-function diagnostic, got: {err}"
    );
}

// ---- argument-handling tests through the public `run` entry point ----------

fn write_temp(name: &str, contents: &[u8]) -> PathBuf {
    let mut path = std::env::temp_dir();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    path.push(format!("rphp_test_{}_{}_{}.php", std::process::id(), nanos, name));
    std::fs::write(&path, contents).expect("write temp file");
    path
}

#[test]
fn run_subcommand_executes_file() {
    let path = write_temp("run", b"<?php echo 6 * 7;");
    let code = run(vec!["run".to_string(), path.to_string_lossy().into_owned()]);
    let _ = std::fs::remove_file(&path);
    assert_eq!(code, 0);
}

#[test]
fn bare_file_argument_executes() {
    let path = write_temp("bare", b"<?php echo 1;");
    let code = run(vec![path.to_string_lossy().into_owned()]);
    let _ = std::fs::remove_file(&path);
    assert_eq!(code, 0);
}

#[test]
fn help_flag_exits_zero() {
    assert_eq!(run(vec!["--help".to_string()]), 0);
    assert_eq!(run(vec!["-h".to_string()]), 0);
}

#[test]
fn no_args_exits_zero() {
    assert_eq!(run(vec![]), 0);
}

#[test]
fn unknown_flag_exits_two() {
    assert_eq!(run(vec!["--nope".to_string()]), 2);
}

#[test]
fn missing_file_exits_one() {
    let code = run(vec!["/this/path/does/not/exist_rphp.php".to_string()]);
    assert_eq!(code, 1);
}

#[test]
fn runtime_fault_exits_255() {
    let path = write_temp("divzero", b"<?php echo 1 / 0;");
    let code = run(vec![path.to_string_lossy().into_owned()]);
    let _ = std::fs::remove_file(&path);
    assert_eq!(code, 255);
}
