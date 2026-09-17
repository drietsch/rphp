//! The run-tests.php `EXPECTF` wildcard subset implemented for `.expectf`
//! sidecars.

use rphp_test::differential::{expectf_matches, expectf_regex};

fn m(template: &str, actual: &str) -> bool {
    expectf_matches(template.as_bytes(), actual.as_bytes())
}

#[test]
fn integer_and_float_wildcards() {
    assert!(m("int(%d)", "int(42)"));
    assert!(!m("int(%d)", "int(-42)"), "%d is unsigned");
    assert!(m("int(%i)", "int(-42)"));
    assert!(m("int(%i)", "int(+7)"));
    assert!(m("float(%f)", "float(3.5)"));
    assert!(m("float(%f)", "float(-0.25)"));
    assert!(m("float(%f)", "float(1.0E+25)"));
    assert!(m("float(%f)", "float(7)"));
    assert!(!m("float(%f)", "float(abc)"));
    assert!(m("0x%x", "0xDEADbeef"));
    assert!(!m("0x%x", "0xZZ"));
}

#[test]
fn string_and_any_wildcards() {
    assert!(m("hello %s!", "hello world!"));
    assert!(!m("hello %s!", "hello\nworld!"), "%s stops at a newline");
    assert!(!m("hello %s!", "hello !"), "%s needs at least one char");
    assert!(m("hello %S!", "hello !"));
    assert!(m("a%ab", "a\nx\ny\nb"), "%a crosses newlines");
    assert!(!m("a%ab", "ab"), "%a needs at least one char");
    assert!(m("a%Ab", "ab"));
    assert!(m("a%wb", "a \n\t b"));
    assert!(m("a%wb", "ab"));
    assert!(m("x%cy", "xzy"));
    assert!(!m("x%cy", "xy"));
    assert!(m("dir%esub", "dir/sub"));
}

#[test]
fn raw_regex_sections() {
    assert!(m("id=%r[a-f0-9]{4}%r!", "id=be3f!"));
    assert!(!m("id=%r[a-f0-9]{4}%r!", "id=zzzz!"));
    assert!(m("%r(foo|bar)%r-%d", "bar-7"));
    // An unterminated %r is a literal.
    assert!(m("100%r", "100%r"));
}

#[test]
fn literals_are_escaped_and_the_match_is_anchored() {
    assert!(m("a.b (c) [d] $e ^f *g +h ?i |j {k} \\l", "a.b (c) [d] $e ^f *g +h ?i |j {k} \\l"));
    assert!(!m("a.b", "axb"));
    assert!(!m("int(%d)", "xint(42)"));
    assert!(!m("int(%d)", "int(42)x"));
    assert!(m("100%", "100%"), "a trailing % is literal");
    assert!(m("100%z", "100%z"), "an unknown wildcard letter is literal");
}

#[test]
fn trimming_and_crlf_folding_follow_run_tests() {
    assert!(m("int(%d)\n", "\n int(5) \n\n"));
    assert!(m("a\nb", "a\r\nb\r\n"));
    assert!(m("  x  ", "x"));
}

#[test]
fn non_utf8_bytes_match_literally() {
    let template = b"bytes: \xff\x00A%s";
    assert!(expectf_matches(template, b"bytes: \xff\x00Az"));
    assert!(!expectf_matches(template, b"bytes: \xfe\x00Az"));
    // Wildcards match raw bytes too.
    assert!(expectf_matches(b"%a", b"\xff\xfe\n\x00"));
    assert!(expectf_matches(b"[%s]", b"[\xff\xfe]"));
}

#[test]
fn regex_translation_is_visible() {
    let re = expectf_regex(b"int(%d) %s").unwrap();
    assert_eq!(re.as_str(), r"(?s-u)^int\(\d+\) [^\r\n]+$");
    let re = expectf_regex(b"\xff").unwrap();
    assert_eq!(re.as_str(), r"(?s-u)^\xFF$");
}
