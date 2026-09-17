//! Parser and section-semantics tests on inline `.phpt` strings.

use std::collections::BTreeMap;
use std::path::Path;

use rphp_test::phpt::parse::{ExpectKind, ParseError, TestFile};
use rphp_test::phpt::sections::{
    expand_ini_placeholders, parse_conflicts, parse_env_section, parse_extensions, parse_headers,
    shape_request, split_args, CaptureStdio, IniSettings, ShapeError,
};

fn parse(text: &str) -> Result<TestFile, ParseError> {
    TestFile::parse(Path::new("/tmp/inline/example.phpt"), text.as_bytes())
}

#[test]
fn basic_sections() {
    let t = parse("--TEST--\nhello world\n--FILE--\n<?php echo 1;\n--EXPECT--\n1\n").unwrap();
    assert_eq!(t.name(), "hello world");
    assert_eq!(t.file(), b"<?php echo 1;\n");
    assert_eq!(t.expected(), b"1\n");
    assert_eq!(t.expect_kind(), ExpectKind::Exact);
    assert!(!t.is_cgi());
    let names: Vec<&str> = t.section_names().collect();
    assert_eq!(names, vec!["EXPECT", "FILE", "TEST"]);
}

#[test]
fn header_line_trailing_text_is_ignored() {
    let t = parse("--TEST--n\nx\n--FILE--ignored\n<?php\n--EXPECT--\n").unwrap();
    assert_eq!(t.name(), "x");
    assert_eq!(t.file(), b"<?php\n");
    assert!(t.has("EXPECT"));
}

#[test]
fn dashes_only_lines_are_content() {
    let t = parse("--TEST--\nx\n--FILE--\n<?php\n--EXPECT--\n----\n--- a ---\n").unwrap();
    assert_eq!(t.expected(), b"----\n--- a ---\n");
}

#[test]
fn done_marker_ends_file_section() {
    let t = parse("--TEST--\nx\n--FILE--\n<?php\necho 1;\n===DONE===\n<?php exit(0);\nignored\n--EXPECT--\n1\n===DONE===\n")
        .unwrap();
    assert_eq!(t.file(), b"<?php\necho 1;\n===DONE===\n");
    // The marker does not terminate non-FILE sections.
    assert_eq!(t.expected(), b"1\n===DONE===\n");
}

#[test]
fn fileeof_strips_trailing_newlines() {
    let t = parse("--TEST--\nx\n--FILEEOF--\n<?php echo 1;\n\r\n\n--EXPECT--\n1\n").unwrap();
    assert!(!t.has("FILEEOF"));
    assert_eq!(t.file(), b"<?php echo 1;");
}

#[test]
fn expectf_and_expectregex_kinds() {
    let t = parse("--TEST--\nx\n--FILE--\n<?php\n--EXPECTF--\n%d\n").unwrap();
    assert_eq!(t.expect_kind(), ExpectKind::Format);
    let t = parse("--TEST--\nx\n--FILE--\n<?php\n--EXPECTREGEX--\n\\d+\n").unwrap();
    assert_eq!(t.expect_kind(), ExpectKind::Regex);
}

#[test]
fn errors() {
    assert_eq!(parse(""), Err(ParseError::Empty));
    assert_eq!(parse("<?php\n"), Err(ParseError::NoTestHeader));
    assert_eq!(
        parse("--TEST--\nx\n--BOGUS--\n--FILE--\n<?php\n--EXPECT--\n"),
        Err(ParseError::UnknownSection("BOGUS".into()))
    );
    assert_eq!(
        parse("--TEST--\nx\n--EXPECT--\n1\n"),
        Err(ParseError::MissingFile)
    );
    assert_eq!(
        parse("--TEST--\nx\n--FILE--\n<?php\n"),
        Err(ParseError::MissingExpect)
    );
    assert_eq!(
        parse("--TEST--\nx\n--FILE--\n<?php\n--EXPECT--\n1\n--EXPECTF--\n1\n"),
        Err(ParseError::MissingExpect)
    );
    assert_eq!(
        parse("--TEST--\nx\n--FILE--\n<?php\n--FILE--\n<?php\n--EXPECT--\n1\n"),
        Err(ParseError::DuplicateSection("FILE".into()))
    );
    // A second --TEST-- header duplicates the (non-empty) implicit one; the
    // duplicate check runs before the allowed-list check, as in run-tests.
    assert_eq!(
        parse("--TEST--\nx\n--TEST--\ny\n--FILE--\n<?php\n--EXPECT--\n1\n"),
        Err(ParseError::DuplicateSection("TEST".into()))
    );
    // An empty duplicate is tolerated (run-tests only rejects non-empty ones).
    assert!(
        parse("--TEST--\nx\n--INI--\n--INI--\nprecision=3\n--FILE--\n<?php\n--EXPECT--\n1\n")
            .is_ok()
    );
    let e =
        parse("--TEST--\nx\n--FILE_EXTERNAL--\n../../etc/nope.inc\n--EXPECT--\n1\n").unwrap_err();
    match e {
        ParseError::ExternalNotFound { section, path } => {
            assert_eq!(section, "FILE_EXTERNAL");
            // `..` stripped, then string-concatenated: cannot escape the dir.
            assert_eq!(path, Path::new("/tmp/inline///etc/nope.inc"));
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn external_sections_are_inlined() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("body.inc"), "<?php echo 'ext';\n").unwrap();
    std::fs::write(dir.path().join("want.txt"), "ext\n").unwrap();
    let path = dir.path().join("t.phpt");
    std::fs::write(
        &path,
        "--TEST--\nx\n--FILE_EXTERNAL--\nbody.inc\n--EXPECT_EXTERNAL--\nwant.txt\n",
    )
    .unwrap();
    let t = TestFile::load(&path).unwrap();
    assert_eq!(t.file(), b"<?php echo 'ext';\n");
    assert_eq!(t.expected(), b"ext\n");
    assert_eq!(t.expect_kind(), ExpectKind::Exact);
}

#[test]
fn redirect_and_phpdbg_do_not_need_file() {
    let t = parse("--TEST--\nx\n--REDIRECTTEST--\nreturn [];\n").unwrap();
    assert!(t.has("REDIRECTTEST"));
    let t = parse("--TEST--\nx\n--PHPDBG--\nrun\n--EXPECT--\nok\n").unwrap();
    assert_eq!(t.section("STDIN"), Some(&b"run\n\n"[..]));
}

#[test]
fn cgi_detection_and_not_empty() {
    let t = parse("--TEST--\nx\n--GET--\na=1\n--FILE--\n<?php\n--EXPECT--\n").unwrap();
    assert!(t.is_cgi());
    let t = parse("--TEST--\nx\n--GET--\n--FILE--\n<?php\n--EXPECT--\n").unwrap();
    assert!(!t.is_cgi(), "an empty GET section does not make a CGI test");
    let t = parse("--TEST--\nx\n--CGI--\n--FILE--\n<?php\n--EXPECT--\n").unwrap();
    assert!(t.is_cgi());
    let t = parse("--TEST--\nx\n--FILE--\n<?php\n--EXPECT--\n1\n--SKIPIF--\n0").unwrap();
    assert!(!t.section_not_empty("SKIPIF"), "PHP empty('0') is true");
    let t = parse("--TEST--\nx\n--SKIPIF--\n0\n--FILE--\n<?php\n--EXPECT--\n").unwrap();
    assert!(t.section_not_empty("SKIPIF"), "but \"0\\n\" is not empty");
}

#[test]
fn ini_settings_semantics() {
    let mut ini = IniSettings::new();
    ini.add_text("precision=14\r\nserialize_precision = -1\nextension=a.so\nprecision=3\nextension=b.so\nno_equals_here\n");
    assert_eq!(ini.get("precision"), Some("3"));
    assert_eq!(ini.get("serialize_precision"), Some("-1"));
    assert_eq!(
        ini.to_args(),
        vec![
            "-d",
            "precision=3",
            "-d",
            "serialize_precision=-1",
            "-d",
            "extension=a.so",
            "-d",
            "extension=b.so",
        ]
    );
    let dir = Path::new("/x/y");
    let out = expand_ini_placeholders(
        "sendmail_path={MAIL:m.out}\nauto_prepend_file={PWD}/p.inc\n",
        dir,
    )
    .unwrap();
    assert_eq!(
        out,
        "sendmail_path=tee m.out >/dev/null\nauto_prepend_file=/x/y/p.inc\n"
    );
    let err = expand_ini_placeholders("x={ENV:RPHP_SURELY_UNSET_VAR_42}", dir).unwrap_err();
    assert_eq!(
        err,
        "Environment variable RPHP_SURELY_UNSET_VAR_42 is not set"
    );
    std::env::set_var("RPHP_PHPT_TEST_VAR", "v");
    assert_eq!(
        expand_ini_placeholders("x={ENV:RPHP_PHPT_TEST_VAR}", dir).unwrap(),
        "x=v"
    );
}

#[test]
fn env_section() {
    let env = parse_env_section(
        "FOO=bar\n  BAZ = qux=1 \n\nnoequals\n=empty\nDIR={PWD}/f\n",
        Path::new("/d"),
    );
    assert_eq!(
        env,
        vec![
            ("FOO".into(), "bar".into()),
            ("BAZ ".into(), " qux=1".into()),
            ("DIR".into(), "/d/f".into()),
        ]
    );
}

#[test]
fn capture_stdio_and_args() {
    assert_eq!(CaptureStdio::parse(None), CaptureStdio::ALL);
    let c = CaptureStdio::parse(Some("STDOUT"));
    assert!(c.stdout && !c.stderr && !c.stdin);
    let c = CaptureStdio::parse(Some("stdin, stderr"));
    assert!(!c.stdout && c.stderr && c.stdin);

    assert_eq!(
        split_args("-a 'b c' \"d \\\"e\\\"\" f\\ g\n"),
        vec!["-a", "b c", "d \"e\"", "f g"]
    );
    assert!(split_args("  \n").is_empty());
}

#[test]
fn lists_and_headers() {
    assert_eq!(
        parse_extensions("\nctype\r\njson \n"),
        vec!["ctype", "json"]
    );
    assert_eq!(
        parse_conflicts("server # comment\n\nall\n"),
        vec!["server", "all"]
    );
    assert_eq!(
        parse_headers("Content-Type: text/html\r\nX-Foo:bar\nnocolon\n"),
        vec![
            ("Content-Type".into(), "text/html".into()),
            ("X-Foo".into(), "bar".into())
        ]
    );
}

#[test]
fn request_shaping() {
    let t = parse("--TEST--\nx\n--POST--\na=1&b=2\n--FILE--\n<?php\n--EXPECT--\n").unwrap();
    let mut env = BTreeMap::new();
    let body = shape_request(&t, &mut env).unwrap();
    assert_eq!(body.as_deref(), Some(&b"a=1&b=2"[..]));
    assert_eq!(env["REQUEST_METHOD"], "POST");
    assert_eq!(env["CONTENT_TYPE"], "application/x-www-form-urlencoded");
    assert_eq!(env["CONTENT_LENGTH"], "7");

    let t = parse("--TEST--\nx\n--POST_RAW--\nContent-Type: multipart/form-data; boundary=x\n--x\r\nbody\n--x--\n--FILE--\n<?php\n--EXPECT--\n").unwrap();
    let mut env = BTreeMap::new();
    let body = shape_request(&t, &mut env).unwrap().unwrap();
    assert_eq!(body, b"--x\r\nbody\n--x--");
    assert_eq!(env["CONTENT_TYPE"], "multipart/form-data; boundary=x");
    assert_eq!(env["CONTENT_LENGTH"], body.len().to_string());
    assert_eq!(env["REQUEST_METHOD"], "POST");

    let t = parse(
        "--TEST--\nx\n--PUT--\nContent-Type: text/plain\nhello\n--FILE--\n<?php\n--EXPECT--\n",
    )
    .unwrap();
    let mut env = BTreeMap::new();
    assert_eq!(shape_request(&t, &mut env).unwrap().unwrap(), b"hello");
    assert_eq!(env["REQUEST_METHOD"], "PUT");

    let t = parse("--TEST--\nx\n--PUT--\nContent-Type: text/plain\n--FILE--\n<?php\n--EXPECT--\n")
        .unwrap();
    let mut env = BTreeMap::new();
    assert_eq!(
        shape_request(&t, &mut env),
        Err(ShapeError::Bork("empty $request".into()))
    );

    let t = parse("--TEST--\nx\n--GZIP_POST--\nzz\n--FILE--\n<?php\n--EXPECT--\n").unwrap();
    let mut env = BTreeMap::new();
    assert!(matches!(
        shape_request(&t, &mut env),
        Err(ShapeError::Skip(_))
    ));

    let t = parse("--TEST--\nx\n--GET--\na=1\n--FILE--\n<?php\n--EXPECT--\n").unwrap();
    let mut env = BTreeMap::new();
    assert_eq!(shape_request(&t, &mut env).unwrap(), None);
    assert_eq!(env["REQUEST_METHOD"], "GET");
    assert_eq!(env["CONTENT_TYPE"], "");
}
