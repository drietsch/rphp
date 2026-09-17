//! The comparison policy on synthetic `RunResult`s, the diff renderer, the
//! log-duplicate stripper, and the process runner against a real php when one
//! is installed.

use std::path::Path;
use std::time::Duration;

use rphp_test::differential::{
    collect_snippets, compare, escape_bytes, expected_exit, find_php, php_args, run_command,
    run_snippet, strip_log_duplicates, unified_diff, Allowlist, Channel, NormalizeContext,
    Policy, RunResult, Verdict,
};

fn res(out: &str, err: &str, status: i32) -> RunResult {
    RunResult::new(out.as_bytes().to_vec(), err.as_bytes().to_vec(), status)
}

fn channel(v: &Verdict) -> Option<Channel> {
    match v {
        Verdict::Match => None,
        Verdict::Mismatch { channel, .. } => Some(*channel),
    }
}

#[test]
fn identical_results_match() {
    let list = Allowlist::empty();
    let policy = Policy::new("x.php", &list);
    let v = compare(&res("hi\n", "", 0), &res("hi\n", "", 0), &policy);
    assert!(v.is_match());
    let v = compare(&res("", "warn\n", 255), &res("", "warn\n", 255), &policy);
    assert!(v.is_match());
}

#[test]
fn stdout_is_byte_exact_by_default() {
    let list = Allowlist::empty();
    let policy = Policy::new("x.php", &list);
    let v = compare(&res("a\nb\nc\n", "", 0), &res("a\nB\nc\n", "", 0), &policy);
    assert_eq!(channel(&v), Some(Channel::Stdout));
    let Verdict::Mismatch { first_diff, .. } = v else { unreachable!() };
    assert!(first_diff.contains("--- php\n+++ rphp\n"), "{first_diff}");
    assert!(first_diff.contains("first difference at line 2"), "{first_diff}");
    assert!(first_diff.contains(" a\n-b\n+B\n c\n"), "{first_diff}");
    // Trailing newline differences count.
    let v = compare(&res("a\n", "", 0), &res("a", "", 0), &policy);
    assert_eq!(channel(&v), Some(Channel::Stdout));
    let Verdict::Mismatch { first_diff, .. } = v else { unreachable!() };
    assert!(first_diff.contains("\\ No newline at end of file"), "{first_diff}");
}

#[test]
fn stderr_then_exit_are_checked_after_stdout() {
    let list = Allowlist::empty();
    let policy = Policy::new("x.php", &list);
    let v = compare(&res("ok\n", "php says\n", 0), &res("ok\n", "", 0), &policy);
    assert_eq!(channel(&v), Some(Channel::Stderr));
    let v = compare(&res("ok\n", "", 0), &res("ok\n", "", 255), &policy);
    assert_eq!(channel(&v), Some(Channel::Exit));
    let Verdict::Mismatch { first_diff, .. } = v else { unreachable!() };
    assert_eq!(first_diff, "exit code differs: php 0 vs rphp 255");
    // A stdout difference is reported before an exit difference.
    let v = compare(&res("a\n", "", 0), &res("b\n", "", 1), &policy);
    assert_eq!(channel(&v), Some(Channel::Stdout));
}

#[test]
fn timeouts_are_exit_mismatches_even_when_output_agrees() {
    let list = Allowlist::empty();
    let policy = Policy::new("x.php", &list);
    let mut slow = res("", "", -1);
    slow.timed_out = true;
    let v = compare(&res("", "", -1), &slow, &policy);
    assert_eq!(v, Verdict::Mismatch { channel: Channel::Exit, first_diff: "rphp timed out".to_string() });
}

#[test]
fn php_log_duplicates_are_stripped_from_stderr() {
    let php = res(
        "\nFatal error: Uncaught Error: boom in /f.php:3\nStack trace:\n#0 {main}\n  thrown in /f.php on line 3\n",
        "PHP Fatal error:  Uncaught Error: boom in /f.php:3\nStack trace:\n#0 {main}\n  thrown in /f.php on line 3\n",
        255,
    );
    let mut rphp = php.clone();
    rphp.stderr.clear();
    let list = Allowlist::empty();
    assert!(compare(&php, &rphp, &Policy::new("x.php", &list)).is_match());

    // Lines that are not duplicates survive, including after a stripped block.
    let out = strip_log_duplicates(b"PHP Warning:  w in f on line 1\nreal stderr\nPHP Notice:  other\n", b"\nWarning: w in f on line 1\n");
    assert_eq!(out, b"real stderr\nPHP Notice:  other\n");
    assert_eq!(strip_log_duplicates(b"", b"x"), b"");
}

#[test]
fn allowlist_normalizers_apply_to_both_sides() {
    let php = res("object(A)#1 (0) {\n}\n", "", 0);
    let rphp = res("object(A)#2 (0) {\n}\n", "", 0);
    let none = Allowlist::empty();
    assert_eq!(channel(&compare(&php, &rphp, &Policy::new("spl/ids.php", &none))), Some(Channel::Stdout));
    let list = Allowlist::parse("[[allow]]\nsnippet = \"spl/*.php\"\ncategory = \"object-id\"\nreason = \"handles\"\n").unwrap();
    assert!(compare(&php, &rphp, &Policy::new("spl/ids.php", &list)).is_match());
    // The entry does not apply to other snippets.
    assert_eq!(channel(&compare(&php, &rphp, &Policy::new("lang/ids.php", &list))), Some(Channel::Stdout));
    // Non-volatile output is still compared under the entry.
    let rphp2 = res("object(B)#2 (0) {\n}\n", "", 0);
    assert_eq!(channel(&compare(&php, &rphp2, &Policy::new("spl/ids.php", &list))), Some(Channel::Stdout));
}

#[test]
fn allowlist_normalizers_apply_to_stderr_with_per_side_contexts() {
    let list = Allowlist::parse("[[allow]]\nsnippet = \"x.php\"\ncategory = \"path\"\nreason = \"cwd differs per side\"\n").unwrap();
    let mut policy = Policy::new("x.php", &list);
    policy.php_ctx = NormalizeContext::new().with_path("/tmp/run/php");
    policy.rphp_ctx = NormalizeContext::new().with_path("/tmp/run/rphp");
    let php = res("cwd=/tmp/run/php\n", "err in /tmp/run/php\n", 0);
    let rphp = res("cwd=/tmp/run/rphp\n", "err in /tmp/run/rphp\n", 0);
    assert!(compare(&php, &rphp, &policy).is_match());
}

#[test]
fn expectf_template_replaces_byte_equality() {
    let list = Allowlist::empty();
    let policy = Policy::new("x.php", &list).with_expectf(b"int(%d)\n");
    assert!(compare(&res("int(5)\n", "", 0), &res("int(7)\n", "", 0), &policy).is_match());
    let v = compare(&res("int(5)\n", "", 0), &res("int(x)\n", "", 0), &policy);
    let Verdict::Mismatch { channel, first_diff } = v else { panic!("expected mismatch") };
    assert_eq!(channel, Channel::Stdout);
    assert!(first_diff.starts_with("rphp stdout does not match the .expectf template"), "{first_diff}");
    let v = compare(&res("int(x)\n", "", 0), &res("int(7)\n", "", 0), &policy);
    let Verdict::Mismatch { first_diff, .. } = v else { panic!("expected mismatch") };
    assert!(first_diff.starts_with("stock php stdout does not match"), "{first_diff}");
}

#[test]
fn unified_diff_is_bounded_and_escapes_bytes() {
    let left: String = (1..=40).map(|i| format!("line {i}\n")).collect();
    let right = left.replace("line 20\n", "LINE 20\nextra\n");
    let d = unified_diff(left.as_bytes(), right.as_bytes(), "php", "rphp");
    assert!(d.lines().count() <= 20, "{d}");
    assert!(d.contains(" line 17\n line 18\n line 19\n-line 20\n+LINE 20\n+extra\n line 21\n"), "{d}");

    let d = unified_diff(b"a\n\xff\xfe\n", b"a\n\x01\n", "php", "rphp");
    assert!(d.contains("-\\xFF\\xFE\n+\\x01\n"), "{d}");

    let many: String = (1..=30).map(|i| format!("r{i}\n")).collect();
    let d = unified_diff(b"x\n", many.as_bytes(), "php", "rphp");
    assert!(d.contains("more lines)"), "{d}");
    assert!(d.lines().count() <= 20, "{d}");
    assert_eq!(unified_diff(b"same", b"same", "a", "b"), "(identical)\n");
    assert_eq!(escape_bytes(b"ok\t\xc3\xa9\r\xff"), "ok\t\u{e9}\\x0D\\xFF");
}

#[test]
fn oracle_argument_vector_is_pinned() {
    let args = php_args(Path::new("/s.php"), &["a".to_string()]);
    let args: Vec<String> = args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
    assert_eq!(args[0], "-n");
    assert!(args.windows(2).any(|w| w[0] == "-d" && w[1] == "display_errors=1"));
    assert!(args.windows(2).any(|w| w[0] == "-d" && w[1] == "serialize_precision=-1"));
    assert!(args.windows(2).any(|w| w[0] == "-d" && w[1] == "zend.assertions=-1"));
    assert_eq!(&args[args.len() - 2..], ["/s.php", "a"]);
}

#[test]
fn sidecars_and_snippet_collection() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("b")).unwrap();
    std::fs::create_dir_all(root.join("a")).unwrap();
    std::fs::write(root.join("b/x.php"), "<?php\n").unwrap();
    std::fs::write(root.join("a/y.php"), "<?php\n").unwrap();
    std::fs::write(root.join("a/y.exit"), " 255 \n").unwrap();
    std::fs::write(root.join("a/notes.md"), "").unwrap();
    let found = collect_snippets(root);
    assert_eq!(found, vec![root.join("a/y.php"), root.join("b/x.php")]);
    assert_eq!(expected_exit(&root.join("a/y.php")).unwrap(), 255);
    assert_eq!(expected_exit(&root.join("b/x.php")).unwrap(), 0);
    std::fs::write(root.join("b/x.exit"), "lots").unwrap();
    assert!(expected_exit(&root.join("b/x.php")).is_err());
}

#[cfg(unix)]
#[test]
fn run_command_enforces_the_timeout() {
    let mut cmd = std::process::Command::new("sleep");
    cmd.arg("5");
    let r = run_command(cmd, Duration::from_millis(100)).unwrap();
    assert!(r.timed_out);
    let mut cmd = std::process::Command::new("sh");
    cmd.args(["-c", "printf out; printf err >&2; exit 3"]);
    let r = run_command(cmd, Duration::from_secs(5)).unwrap();
    assert_eq!(r, RunResult::new("out", "err", 3));
}

/// End-to-end through real processes: php compared against itself must match,
/// and the scrubbed environment/pinned ini are observable. Skipped without php.
#[test]
fn run_snippet_against_php_itself_matches() {
    let Some(php) = find_php() else {
        eprintln!("skipping: no php");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let snippet = dir.path().join("env.php");
    std::fs::write(&snippet, "<?php\necho getenv('LC_ALL'), ' ', getenv('TZ'), ' ', ini_get('precision'), ' ', ini_get('display_errors'), \"\\n\";\nvar_dump(getenv('RPHP_TEST_MARKER'));\n").unwrap();
    std::env::set_var("RPHP_TEST_MARKER", "leak");
    let list = Allowlist::empty();
    assert_eq!(run_snippet(&php, &php, &snippet, &list), Verdict::Match);
    let r = rphp_test::differential::run_php(&php, &snippet, &[], dir.path(), Duration::from_secs(30)).unwrap();
    assert_eq!(r.stdout_lossy(), "C UTC 14 1\nbool(false)\n");
    assert_eq!(r.status, 0);

    // A faulting script: the exit code channel is compared exactly.
    let fatal = dir.path().join("fatal.php");
    std::fs::write(&fatal, "<?php\n$f = 'nope';\n$f();\n").unwrap();
    assert_eq!(run_snippet(&php, &php, &fatal, &list), Verdict::Match);
    let r = rphp_test::differential::run_php(&php, &fatal, &[], dir.path(), Duration::from_secs(30)).unwrap();
    assert_eq!(r.status, 255);
    assert!(r.stdout_lossy().contains("Fatal error: Uncaught Error: Call to undefined function nope()"));
    assert!(r.stderr.is_empty(), "log_errors=0 keeps stderr clean");

    // A missing binary is a harness error, reported as an Exit mismatch.
    let v = run_snippet(Path::new("/nonexistent/php"), &php, &snippet, &list);
    assert_eq!(channel(&v), Some(Channel::Exit));
}
