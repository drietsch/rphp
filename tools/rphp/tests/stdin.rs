//! The CLI's standard input: `STDIN`, `php://stdin` and the whole-file
//! readers over it, fed through a real pipe under php and under rphp and
//! required to match. The tier-a harness runs every snippet with no stdin
//! at all, so this is the only place fd 0 is exercised.
//!
//! **Skipped** (not failed) when no php is found (`PHP_BIN` or `php` on
//! `PATH`).

use std::io::Write;
use std::process::{Command, Stdio};

use rphp_test::differential::find_php;

/// The binary under test.
const RPHP: &str = env!("CARGO_BIN_EXE_rphp");

/// Run `code` under one engine with `input` on its stdin.
fn run(engine: &std::path::Path, code: &str, input: &[u8]) -> String {
    let mut child = Command::new(engine)
        .args(["-n", "-d", "display_errors=1", "-d", "log_errors=0", "-r", code])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("{} should run: {e}", engine.display()));
    // A writer thread, so a script that reads nothing cannot deadlock the
    // test on a full pipe.
    let mut stdin = child.stdin.take().expect("a stdin pipe");
    let input = input.to_vec();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&input);
    });
    let out = child.wait_with_output().expect("the engine exits");
    let _ = writer.join();
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    text.push_str(&format!("exit {:?}\n", out.status.code()));
    text
}

#[test]
fn stdin_matches_stock_php() {
    let Some(php) = find_php() else {
        eprintln!("no php found — skipping the stdin differential");
        return;
    };
    let many: String = (1..=20000).map(|i| format!("{i}\n")).collect();
    let cases: &[(&str, &[u8])] = &[
        (
            r#"var_dump(STDIN); while (($l = fgets(STDIN)) !== false) echo strtoupper($l); var_dump(feof(STDIN), fgets(STDIN));"#,
            b"one\ntwo\nthree",
        ),
        (r#"var_dump(fread(STDIN, 4), stream_get_contents(STDIN));"#, b"abcdefgh\nij\n"),
        (r#"var_dump(stream_get_meta_data(STDIN));"#, b""),
        (r#"var_dump(file_get_contents('php://stdin'));"#, b"all of it\n"),
        (r#"echo count(file('php://stdin')), "\n";"#, many.as_bytes()),
        (r#"var_dump(file('php://stdin', FILE_IGNORE_NEW_LINES));"#, b"a\nb\n"),
        (r#"var_dump(readfile('php://stdin'));"#, b"echoed\n"),
        (r#"$h = fopen('php://stdin', 'r'); var_dump($h, fgets($h), fgets($h));"#, b"x\n"),
        (r#"var_dump(fgets(STDIN));"#, b""),
    ];
    for (code, input) in cases {
        let from_php = run(&php, code, input);
        let from_rphp = run(std::path::Path::new(RPHP), code, input);
        assert_eq!(from_php, from_rphp, "\n{code}\n--- php ---\n{from_php}\n--- rphp ---\n{from_rphp}\n");
    }
}
