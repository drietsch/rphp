//! End-to-end tests of `cargo xtask ladder`: a `--list` smoke test over the
//! real fixtures (no php needed), and synthetic fixtures in a temp dir run with
//! `--only php` and with both sides being the stock php (`--rphp <php>`), which
//! must be `ok` — plus divergence, artifact, allowlist and setup-error paths.

use std::path::{Path, PathBuf};
use std::process::Command;

use rphp_test::differential::find_php;

fn xtask() -> Command {
    Command::new(env!("CARGO_BIN_EXE_xtask"))
}

/// Run `cargo xtask ladder <args>`; returns (exit code, stdout, stderr).
fn ladder(args: &[&str]) -> (i32, String, String) {
    let out = xtask().arg("ladder").args(args).output().expect("xtask runs");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn write(path: &Path, content: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

struct Sandbox {
    _tmp: tempfile::TempDir,
    fixtures: PathBuf,
    target: PathBuf,
}

impl Sandbox {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let fixtures = tmp.path().join("fixtures");
        let target = tmp.path().join("target");
        std::fs::create_dir_all(&fixtures).unwrap();
        std::fs::create_dir_all(&target).unwrap();
        Sandbox { _tmp: tmp, fixtures, target }
    }

    fn args<'a>(&'a self, extra: &[&'a str]) -> Vec<String> {
        let mut v = vec![
            "--fixtures-dir".to_string(),
            self.fixtures.display().to_string(),
            "--target-dir".to_string(),
            self.target.display().to_string(),
        ];
        v.extend(extra.iter().map(|s| s.to_string()));
        v
    }

    fn run(&self, extra: &[&str]) -> (i32, String, String) {
        let args = self.args(extra);
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        ladder(&refs)
    }

    fn side_dir(&self, fixture: &str, rung: &str, side: &str) -> PathBuf {
        self.target.join("ladder").join(fixture).join(rung).join(side)
    }
}

/// The synthetic fixture: one-line scripts covering every schema feature.
fn synthetic(sb: &Sandbox) -> PathBuf {
    let dir = sb.fixtures.join("synthetic");
    write(
        &dir.join("ladder.toml"),
        r#"
[fixture]
name = "synthetic"
allowlist = "divergences.toml"

[[rung]]
id = "R1"
title = "hello"
commands = ["hello.php", "hello.php world"]

[[rung]]
id = "R2"
title = "features"
env = { LADDER_X = "42" }
commands = [
  { run = "cat.php", stdin = "ping\n" },
  "env.php",
  { run = "exit3.php", expect_exit = 3 },
  "artifact.php",
  { run = "timing.php", allow = ["timing"] },
  "dropme.php",
]
artifacts = ["var/*.txt"]

[[rung]]
id = "R3"
title = "diverges"
commands = ["clock.php"]
artifacts = ["var/clock.txt"]

[[rung]]
id = "R4"
title = "pinned exit violated"
commands = [{ run = "hello.php", expect_exit = 7 }]

[[rung]]
id = "H"
title = "http"
http = true
docroot = "public"
requests = ["GET /", "POST /x?q=1 a=b", "GET /missing.txt"]
"#,
    );
    write(
        &dir.join("divergences.toml"),
        r#"
[[allow]]
snippet   = "R2/dropme.php"
category  = "platform-value"
reason    = "test: the volatile line is dropped on both sides"
normalize = "drop-lines:^volatile "
"#,
    );
    write(&dir.join("hello.php"), "<?php echo \"hi \", $argv[1] ?? \"-\", \"\\n\";\n");
    write(
        &dir.join("public/index.php"),
        "<?php header('X-Ladder: 1'); echo \"served \", $_SERVER['REQUEST_METHOD'], \" \", $_SERVER['REQUEST_URI'], \" \", $_POST['a'] ?? '-', \"\\n\";\n",
    );
    write(&dir.join("cat.php"), "<?php echo \"in:\", stream_get_contents(STDIN);\n");
    write(&dir.join("env.php"), "<?php echo getenv(\"LADDER_X\"), \" \", getenv(\"TZ\"), \"\\n\";\n");
    write(&dir.join("exit3.php"), "<?php echo \"bye\\n\"; exit(3);\n");
    write(
        &dir.join("artifact.php"),
        "<?php file_put_contents(\"var/out.txt\", getcwd() . \"\\n\" . __DIR__ . \"\\n\"); echo \"wrote\\n\";\n",
    );
    write(&dir.join("timing.php"), "<?php echo \"took \", hrtime(true) % 1000, \" ms\\n\";\n");
    write(&dir.join("dropme.php"), "<?php echo \"keep\\n\", \"volatile \", hrtime(true), \"\\n\";\n");
    write(
        &dir.join("clock.php"),
        "<?php $t = hrtime(true); echo $t, \"\\n\"; file_put_contents(\"var/clock.txt\", $t);\n",
    );
    // A pre-existing var/ must be replaced by an empty one in the working copy.
    write(&dir.join("var").join("stale.txt"), "stale");
    // node_modules is never copied.
    write(&dir.join("node_modules").join("x.js"), "x");
    dir
}

fn php_or_skip() -> Option<PathBuf> {
    let php = find_php();
    if php.is_none() {
        eprintln!("skipping: no php on PATH");
    }
    php
}

#[test]
fn list_real_fixtures() {
    let (code, out, err) = ladder(&["--list"]);
    assert_eq!(code, 0, "stderr: {err}");
    assert!(out.contains("L1-skeleton"), "{out}");
    assert!(out.contains("L1 "), "{out}");
    assert!(out.contains("L6a"), "{out}");
    assert!(out.contains("served by `php -S` vs `rphp -S`"), "{out}");
}

#[test]
fn help_prints_usage() {
    let (code, out, _) = ladder(&["--help"]);
    assert_eq!(code, 0);
    assert!(out.contains("--rung <id>"));
}

#[test]
fn list_synthetic_and_reject_invalid_toml() {
    let sb = Sandbox::new();
    synthetic(&sb);
    let (code, out, err) = sb.run(&["--list"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("synthetic"), "{out}");
    assert!(out.contains("R2 "), "{out}");
    assert!(out.contains("6 command(s), 1 artifact pattern(s), allowlist"), "{out}");
    assert!(out.contains("no composer.json"), "{out}");

    let bad = sb.fixtures.join("bad");
    write(&bad.join("ladder.toml"), "[fixture]\nname = \"bad\"\n[[rung]]\nid = \"X\"\ncommands = []\n");
    let (code, _, err) = sb.run(&["--list"]);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("rung `X` has no commands"), "{err}");
}

#[test]
fn unknown_rung_and_fixture_exit_2() {
    let sb = Sandbox::new();
    synthetic(&sb);
    let (code, _, err) = sb.run(&["--rung", "L99", "--only", "php"]);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("no rung `L99`"), "{err}");
    let (code, _, err) = sb.run(&["--fixture", "nope", "--only", "php"]);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("no fixture named `nope`"), "{err}");
}

#[test]
fn missing_vendor_exits_2_with_setup_hint() {
    let sb = Sandbox::new();
    let dir = sb.fixtures.join("composerish");
    write(&dir.join("ladder.toml"), "[fixture]\nname = \"composerish\"\n[[rung]]\nid = \"R\"\ncommands = [\"a.php\"]\n");
    write(&dir.join("composer.json"), "{}");
    write(&dir.join("a.php"), "<?php echo 1;");
    let (code, _, err) = sb.run(&["--only", "php"]);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("tools/fixtures/setup.sh"), "{err}");
    assert!(err.contains("has no vendor/"), "{err}");
}

#[test]
fn missing_rphp_exits_2_with_build_hint() {
    let sb = Sandbox::new();
    synthetic(&sb);
    let missing = sb.target.join("nope").join("rphp");
    let (code, _, err) = sb.run(&["--rung", "R1", "--rphp", missing.to_str().unwrap()]);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("cargo build -p rphp"), "{err}");
}

#[test]
fn only_php_dumps_outputs() {
    let Some(_php) = php_or_skip() else { return };
    let sb = Sandbox::new();
    synthetic(&sb);
    let (code, out, err) = sb.run(&["--rung", "R1", "--only", "php", "--keep"]);
    assert_eq!(code, 0, "stdout: {out}\nstderr: {err}");
    assert!(out.contains("[1] hello.php  ran"), "{out}");
    assert!(out.contains("[2] hello.php world  ran"), "{out}");
    let side = sb.side_dir("synthetic", "R1", "php");
    assert_eq!(std::fs::read_to_string(side.join("out/1.stdout")).unwrap(), "hi -\n");
    assert_eq!(std::fs::read_to_string(side.join("out/2.stdout")).unwrap(), "hi world\n");
    assert_eq!(std::fs::read_to_string(side.join("out/2.exit")).unwrap(), "0\n");
    assert_eq!(std::fs::read_to_string(side.join("out/1.stderr")).unwrap(), "");
    // --keep: the working copy is still there, with a fresh var/ and no node_modules.
    assert!(side.join("hello.php").is_file());
    assert!(side.join("var").is_dir());
    assert!(!side.join("var/stale.txt").exists(), "var/ starts empty");
    assert!(!side.join("node_modules").exists());
    assert!(out.contains("kept:"), "{out}");

    // Without --keep only out/ survives.
    let (code, _, err) = sb.run(&["--rung", "R1", "--only", "php"]);
    assert_eq!(code, 0, "{err}");
    assert!(side.join("out/1.stdout").is_file());
    assert!(!side.join("hello.php").exists());
    // The rphp side never ran.
    assert!(!sb.side_dir("synthetic", "R1", "rphp").exists());
}

#[test]
fn php_vs_php_matches_including_stdin_env_exit_artifacts_and_allowlists() {
    let Some(php) = php_or_skip() else { return };
    let sb = Sandbox::new();
    synthetic(&sb);
    let (code, out, err) = sb.run(&[
        "--rung", "R1", "--rung", "R2", "--rung", "H", "--rphp", php.to_str().unwrap(), "--keep",
    ]);
    assert_eq!(code, 0, "stdout: {out}\nstderr: {err}");
    assert!(out.contains("[1] hello.php  ok"), "{out}");
    assert!(out.contains("[1] cat.php  ok"), "{out}");
    assert!(out.contains("[3] exit3.php  ok"), "{out}");
    assert!(out.contains("[5] timing.php  ok"), "{out}");
    assert!(out.contains("[allow: timing]"), "{out}");
    assert!(out.contains("[6] dropme.php  ok"), "{out}");
    assert!(out.contains("[allow: platform-value]"), "{out}");
    assert!(out.contains("artifact var/out.txt (var/*.txt)  ok"), "{out}");
    assert!(out.contains("[1] GET /  ok"), "{out}");
    assert!(out.contains("[2] POST /x?q=1 a=b  ok"), "{out}");
    assert!(out.contains("[3] GET /missing.txt  ok"), "{out}");
    assert!(out.contains("-- R2: ok"), "{out}");
    assert!(out.contains("-- H: ok"), "{out}");
    assert!(out.contains("3 rung(s) — 3 ok, 0 FAIL, 0 skipped"), "{out}");
    // The responses are the raw wire bytes, with the port and the date
    // replaced; the HTTP status stands in for the exit code.
    let h_php = sb.side_dir("synthetic", "H", "php");
    let r1 = std::fs::read_to_string(h_php.join("out/1.stdout")).unwrap();
    assert!(r1.starts_with("HTTP/1.1 200 OK\r\nHost: 127.0.0.1:%PORT%\r\nDate: %DATE%\r\n"), "{r1}");
    assert!(r1.contains("X-Ladder: 1\r\n"), "{r1}");
    assert!(r1.ends_with("served GET / -\n"), "{r1}");
    assert!(std::fs::read_to_string(h_php.join("out/2.stdout")).unwrap().ends_with("served POST /x?q=1 b\n"));
    // php's server walks a missing path up to the document root's index.php.
    assert!(std::fs::read_to_string(h_php.join("out/3.stdout")).unwrap().ends_with("served GET /missing.txt -\n"));
    assert_eq!(std::fs::read_to_string(h_php.join("out/3.exit")).unwrap(), "200\n");

    // The dumps prove stdin, env and the exit code reached the processes.
    let php_side = sb.side_dir("synthetic", "R2", "php");
    let rphp_side = sb.side_dir("synthetic", "R2", "rphp");
    assert_eq!(std::fs::read_to_string(php_side.join("out/1.stdout")).unwrap(), "in:ping\n");
    assert_eq!(std::fs::read_to_string(rphp_side.join("out/1.stdout")).unwrap(), "in:ping\n");
    assert_eq!(std::fs::read_to_string(php_side.join("out/2.stdout")).unwrap(), "42 UTC\n");
    assert_eq!(std::fs::read_to_string(php_side.join("out/3.exit")).unwrap(), "3\n");
    // Both sides ran at the same path (…/work), so the artifact is identical
    // even before normalization.
    let a = std::fs::read_to_string(php_side.join("var/out.txt")).unwrap();
    let b = std::fs::read_to_string(rphp_side.join("var/out.txt")).unwrap();
    assert_eq!(a, b);
    assert!(a.contains("/R2/work"), "{a}");

    let report: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(sb.target.join("ladder-report.json")).unwrap()).unwrap();
    assert_eq!(report["summary"]["rungs"], 3);
    assert_eq!(report["summary"]["ok"], 3);
    assert_eq!(report["summary"]["skipped"], 0);
    assert_eq!(report["rungs"][1]["id"], "R2");
    assert_eq!(report["rungs"][1]["status"], "ok");
    assert_eq!(report["rungs"][1]["commands"][2]["php"]["exit"], 3);
}

#[test]
fn divergence_fails_unless_allowed_and_reports_first_divergence() {
    let Some(php) = php_or_skip() else { return };
    let sb = Sandbox::new();
    synthetic(&sb);
    let (code, out, err) = sb.run(&["--rung", "R3", "--rphp", php.to_str().unwrap()]);
    assert_eq!(code, 1, "stdout: {out}\nstderr: {err}");
    assert!(out.contains("[1] clock.php  FAIL stdout"), "{out}");
    assert!(out.contains("--- php\n"), "{out}");
    assert!(out.contains("+++ rphp\n"), "{out}");
    assert!(out.contains("artifact var/clock.txt (var/clock.txt)  FAIL"), "{out}");
    assert!(out.contains("-- R3: FAIL"), "{out}");
    assert!(out.contains("first divergence: synthetic / R3 [1] clock.php (stdout)"), "{out}");
    assert!(err.contains("1 rung(s) failed"), "{err}");

    let (code, out, _) = sb.run(&["--rung", "R3", "--rphp", php.to_str().unwrap(), "--allow-failures"]);
    assert_eq!(code, 0, "{out}");
    let report: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(sb.target.join("ladder-report.json")).unwrap()).unwrap();
    assert_eq!(report["rungs"][0]["status"], "fail");
    assert_eq!(report["rungs"][0]["commands"][0]["channel"], "stdout");
    assert_eq!(report["rungs"][0]["artifacts"][0]["status"], "fail");
    assert_eq!(report["summary"]["commands_failed"], 1);
    assert_eq!(report["summary"]["artifacts_failed"], 1);

    // --report json prints the same document on stdout.
    let (code, out, _) = sb.run(&["--rung", "R3", "--rphp", php.to_str().unwrap(), "--allow-failures", "--report", "json"]);
    assert_eq!(code, 0);
    let printed: serde_json::Value = serde_json::from_str(&out).expect("stdout is the JSON report");
    assert_eq!(printed["summary"]["failed"], 1);
}

#[test]
fn pinned_exit_code_is_checked_on_both_sides() {
    let Some(php) = php_or_skip() else { return };
    let sb = Sandbox::new();
    synthetic(&sb);
    let (code, out, _) = sb.run(&["--rung", "R4", "--rphp", php.to_str().unwrap()]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("FAIL exit"), "{out}");
    assert!(out.contains("php (fixture/oracle problem) exited 0, ladder.toml expects 7"), "{out}");
    // Also enforced with --only.
    let (code, out, _) = sb.run(&["--rung", "R4", "--only", "php"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("exited 0, ladder.toml expects 7"), "{out}");
}
