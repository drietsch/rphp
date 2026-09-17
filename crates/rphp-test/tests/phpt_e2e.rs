//! End-to-end: tiny `.phpt` files run under stock `php` (skipped when php is
//! not on PATH). This is the runner validating itself, the same way
//! `cargo xtask phpt --engine php` does on the real corpus.

use std::path::{Path, PathBuf};
use std::time::Duration;

use rphp_test::phpt::{discover, Cache, Engine, Outcome, RunOptions, Runner};

fn php_engine() -> Option<Engine> {
    let php = Engine::find_in_path("php")?;
    Some(Engine::php(php))
}

fn write(dir: &Path, name: &str, body: &str) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, body).unwrap();
    p
}

fn run(engine: &Engine, path: &Path) -> Outcome {
    let opts = RunOptions {
        timeout: Duration::from_secs(20),
        ..RunOptions::default()
    };
    rphp_test::phpt::run::run_test(engine, &opts, path).outcome
}

#[test]
fn passes_skips_fails_under_stock_php() {
    let Some(engine) = php_engine() else {
        eprintln!("php not on PATH; skipping");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();

    let pass = write(
        d,
        "pass.phpt",
        "--TEST--\npass\n--FILE--\n<?php\necho \"hello\\n\";\n?>\n--EXPECT--\nhello\n",
    );
    assert_eq!(run(&engine, &pass), Outcome::Pass);
    assert!(!d.join("pass.php").exists(), "generated script is removed");

    let expectf = write(
        d,
        "expectf.phpt",
        "--TEST--\nexpectf\n--FILE--\n<?php\nvar_dump(1.5, \"x\", 42);\nechoNope();\n--EXPECTF--\nfloat(%f)\nstring(%d) \"%s\"\nint(%i)\n\nFatal error: Uncaught Error: Call to undefined function echoNope() in %s:%d\nStack trace:\n#0 {main}\n  thrown in %sexpectf.php on line %d\n",
    );
    assert_eq!(run(&engine, &expectf), Outcome::Pass);

    let skipped = write(
        d,
        "skip.phpt",
        "--TEST--\nskip\n--SKIPIF--\n<?php if (PHP_INT_SIZE > 0) die('skip always skipped here'); ?>\n--FILE--\n<?php echo 1;\n--EXPECT--\n1\n",
    );
    assert_eq!(
        run(&engine, &skipped),
        Outcome::Skip {
            reason: "always skipped here".into()
        }
    );
    assert!(!d.join("skip.skip.php").exists());

    let not_skipped = write(
        d,
        "noskip.phpt",
        "--TEST--\nnoskip\n--SKIPIF--\n<?php if (false) die('skip'); ?>\n--FILE--\n<?php echo 1;\n--EXPECT--\n1\n",
    );
    assert_eq!(run(&engine, &not_skipped), Outcome::Pass);

    let failing = write(
        d,
        "fail.phpt",
        "--TEST--\nfail\n--FILE--\n<?php\necho \"a\\nb\\nc\\n\";\n--EXPECT--\na\nB\nc\n",
    );
    match run(&engine, &failing) {
        Outcome::Fail { diff } => {
            assert_eq!(diff.line, 2);
            assert_eq!(diff.expected, "B");
            assert_eq!(diff.actual, "b");
        }
        other => panic!("expected Fail, got {other:?}"),
    }

    let xfail = write(
        d,
        "xfail.phpt",
        "--TEST--\nxfail\n--XFAIL--\nknown bad\n--FILE--\n<?php echo 2;\n--EXPECT--\n1\n",
    );
    assert!(matches!(run(&engine, &xfail), Outcome::XFail { reason, .. } if reason == "known bad"));
    let xpass = write(
        d,
        "xpass.phpt",
        "--TEST--\nxpass\n--XFAIL--\nknown bad\n--FILE--\n<?php echo 1;\n--EXPECT--\n1\n",
    );
    assert_eq!(run(&engine, &xpass), Outcome::XPass);

    let bork = write(d, "bork.phpt", "--TEST--\nbork\n--FILE--\n<?php echo 1;\n");
    assert!(matches!(run(&engine, &bork), Outcome::Borked { .. }));

    let skip_garbage = write(
        d,
        "skipgarbage.phpt",
        "--TEST--\nbork2\n--SKIPIF--\n<?php echo 'unexpected';\n--FILE--\n<?php echo 1;\n--EXPECT--\n1\n",
    );
    assert!(
        matches!(run(&engine, &skip_garbage), Outcome::Borked { reason } if reason.contains("invalid output from SKIPIF"))
    );
}

#[test]
fn ini_env_args_stdin_clean_under_stock_php() {
    let Some(engine) = php_engine() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();

    let ini = write(
        d,
        "ini.phpt",
        "--TEST--\nini\n--INI--\nprecision=3\n--FILE--\n<?php echo M_PI, \"\\n\";\n--EXPECT--\n3.14\n",
    );
    assert_eq!(run(&engine, &ini), Outcome::Pass);

    let env = write(
        d,
        "env.phpt",
        "--TEST--\nenv\n--ENV--\nRPHP_PHPT_E2E=yes\nTZ=Europe/Vienna\n--FILE--\n<?php echo getenv('RPHP_PHPT_E2E'), ' ', getenv('TZ'), \"\\n\";\n--EXPECT--\nyes Europe/Vienna\n",
    );
    assert_eq!(run(&engine, &env), Outcome::Pass);

    let args = write(
        d,
        "args.phpt",
        "--TEST--\nargs\n--ARGS--\n--foo 'bar baz'\n--FILE--\n<?php echo $argc, ':', implode('|', array_slice($argv, 1)), \"\\n\";\n--EXPECT--\n3:--foo|bar baz\n",
    );
    assert_eq!(run(&engine, &args), Outcome::Pass);

    let stdin = write(
        d,
        "stdin.phpt",
        "--TEST--\nstdin\n--STDIN--\nline one\nline two\n--FILE--\n<?php echo strtoupper(stream_get_contents(STDIN));\n--EXPECT--\nLINE ONE\nLINE TWO\n",
    );
    assert_eq!(run(&engine, &stdin), Outcome::Pass);

    let clean = write(
        d,
        "clean.phpt",
        "--TEST--\nclean\n--FILE--\n<?php file_put_contents(__DIR__.'/clean.tmp', 'x'); echo 'ok';\n--CLEAN--\n<?php unlink(__DIR__.'/clean.tmp');\n--EXPECT--\nok\n",
    );
    assert_eq!(run(&engine, &clean), Outcome::Pass);
    assert!(!d.join("clean.tmp").exists(), "CLEAN ran");

    let clean_noise = write(
        d,
        "cleannoise.phpt",
        "--TEST--\nclean noise\n--FILE--\n<?php echo 'ok';\n--CLEAN--\n<?php echo 'oops';\n--EXPECT--\nok\n",
    );
    assert!(
        matches!(run(&engine, &clean_noise), Outcome::Borked { reason } if reason.contains("CLEAN"))
    );

    let stderr = write(
        d,
        "stderr.phpt",
        "--TEST--\nstderr merged\n--FILE--\n<?php fwrite(STDERR, \"err\\n\"); echo \"out\\n\";\n--EXPECT--\nerr\nout\n",
    );
    assert_eq!(run(&engine, &stderr), Outcome::Pass);

    let only_stdout = write(
        d,
        "capture.phpt",
        "--TEST--\nstdout only\n--CAPTURE_STDIO--\nSTDOUT\n--FILE--\n<?php fwrite(STDERR, \"err\\n\"); echo \"out\\n\";\n--EXPECT--\nout\n",
    );
    assert_eq!(run(&engine, &only_stdout), Outcome::Pass);

    let exit_code = write(
        d,
        "exit.phpt",
        "--TEST--\nnon-zero exit is not a failure\n--FILE--\n<?php echo 'bye'; exit(3);\n--EXPECT--\nbye\n",
    );
    assert_eq!(run(&engine, &exit_code), Outcome::Pass);
}

#[test]
fn cgi_get_post_cookie_under_php_cgi() {
    let Some(engine) = php_engine() else {
        return;
    };
    if engine.cgi_binary.is_none() {
        eprintln!("php-cgi not next to php; skipping");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let get = write(
        d,
        "get.phpt",
        "--TEST--\nget\n--GET--\na=1&b=two\n--COOKIE--\nc=3\n--FILE--\n<?php var_dump($_GET['a'], $_GET['b'], $_COOKIE['c'], $_SERVER['REQUEST_METHOD']);\n--EXPECT--\nstring(1) \"1\"\nstring(3) \"two\"\nstring(1) \"3\"\nstring(3) \"GET\"\n",
    );
    assert_eq!(run(&engine, &get), Outcome::Pass);
    let post = write(
        d,
        "post.phpt",
        "--TEST--\npost\n--POST--\nx=hello&y[]=1&y[]=2\n--FILE--\n<?php var_dump($_POST['x'], count($_POST['y']), $_SERVER['REQUEST_METHOD']);\n--EXPECT--\nstring(5) \"hello\"\nint(2)\nstring(4) \"POST\"\n",
    );
    assert_eq!(run(&engine, &post), Outcome::Pass);
    let headers = write(
        d,
        "headers.phpt",
        "--TEST--\nheaders\n--CGI--\n--FILE--\n<?php header('X-Test: yes'); echo 'body';\n--EXPECTHEADERS--\nX-Test: yes\n--EXPECT--\nbody\n",
    );
    assert_eq!(run(&engine, &headers), Outcome::Pass);
}

#[test]
fn timeout_is_reported() {
    let Some(engine) = php_engine() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let slow = write(
        dir.path(),
        "slow.phpt",
        "--TEST--\nslow\n--FILE--\n<?php sleep(30);\n--EXPECT--\n",
    );
    let opts = RunOptions {
        timeout: Duration::from_millis(500),
        retry_flaky: false,
        ..RunOptions::default()
    };
    let r = rphp_test::phpt::run::run_test(&engine, &opts, &slow);
    assert_eq!(r.outcome, Outcome::Timeout);
    assert!(r.duration_ms < 10_000);
}

#[test]
fn runner_caches_and_discovers() {
    let Some(engine) = php_engine() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    std::fs::create_dir_all(d.join("sub")).unwrap();
    write(
        d,
        "a.phpt",
        "--TEST--\na\n--FILE--\n<?php echo 'a';\n--EXPECT--\na\n",
    );
    write(
        d,
        "sub/b.phpt",
        "--TEST--\nb\n--FILE--\n<?php echo 'b';\n--EXPECT--\nB\n",
    );
    write(d, "notatest.php", "<?php\n");
    let tests = discover(d);
    assert_eq!(tests.len(), 2);

    let cache_dir = tempfile::tempdir().unwrap();
    let runner = Runner {
        engine: &engine,
        options: RunOptions::default(),
        cache: Some(Cache::new(cache_dir.path()).unwrap()),
    };
    let first = runner.run_all(&tests, |_| {});
    assert!(matches!(first[0].outcome, Outcome::Pass));
    assert!(matches!(first[1].outcome, Outcome::Fail { .. }));
    assert!(first.iter().all(|r| !r.cached));

    let second = runner.run_all(&tests, |_| {});
    assert!(
        second.iter().all(|r| r.cached),
        "second run is served from the cache"
    );
    assert_eq!(second[1].outcome, first[1].outcome);

    // Editing the test invalidates its entry only.
    write(
        d,
        "sub/b.phpt",
        "--TEST--\nb\n--FILE--\n<?php echo 'b';\n--EXPECT--\nb\n",
    );
    let third = runner.run_all(&tests, |_| {});
    assert!(third[0].cached);
    assert!(!third[1].cached);
    assert_eq!(third[1].outcome, Outcome::Pass);
}
