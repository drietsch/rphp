//! EXPECTF conversion and matching tables.

use rphp_test::phpt::expectf::{
    diff, expectf_to_regex, has_multiline_wildcard, matches, normalize_expected, normalize_output,
    passthrough, quote,
};
use rphp_test::phpt::parse::ExpectKind;

fn fmt(wanted: &str, actual: &str) -> bool {
    matches(ExpectKind::Format, wanted.as_bytes(), actual.as_bytes()).unwrap()
}

#[test]
fn conversion_table() {
    assert_eq!(expectf_to_regex(b"a%sb"), r"a[^\r\n]+b");
    assert_eq!(expectf_to_regex(b"%S"), r"[^\r\n]*");
    assert_eq!(expectf_to_regex(b"%a"), r".+?");
    assert_eq!(expectf_to_regex(b"%A"), r".*?");
    assert_eq!(expectf_to_regex(b"%w"), r"\s*");
    assert_eq!(expectf_to_regex(b"%i"), r"[+-]?\d+");
    assert_eq!(expectf_to_regex(b"%d"), r"\d+");
    assert_eq!(expectf_to_regex(b"%x"), r"[0-9a-fA-F]+");
    assert_eq!(expectf_to_regex(b"%c"), r".");
    assert_eq!(expectf_to_regex(b"%0"), r"\x00");
    assert_eq!(expectf_to_regex(b"%e"), "/");
    assert_eq!(
        expectf_to_regex(b"%f"),
        r"[+-]?(?:\d+(?:\.\d+)?|\.\d+)(?:[Ee][+-]?\d+)?"
    );
    // Literal text is quoted; % alone and unknown wildcards stay literal.
    assert_eq!(expectf_to_regex(b"a.b(c)%%%q"), r"a\.b\(c\)%%%q");
    // CRLF is normalised before quoting.
    assert_eq!(expectf_to_regex(b"a\r\nb"), "a\\x0Ab");
    // %r…%r passes raw regex through in a group; the rest is quoted.
    assert_eq!(expectf_to_regex(b"x %r\\d+|no%r y."), r"x (\d+|no) y\.");
    // Unbalanced %r is kept literally.
    assert_eq!(expectf_to_regex(b"a %r b"), "a %r b");
    // Non-ASCII bytes become \xNN escapes.
    assert_eq!(quote(&[0xE9, b'!']), r"\xE9!");
}

#[test]
fn wildcards_match() {
    assert!(fmt("int(%d)", "int(42)"));
    assert!(!fmt("int(%d)", "int(-42)"));
    assert!(fmt("int(%i)", "int(-42)"));
    assert!(fmt("float(%f)", "float(1.5)"));
    assert!(fmt("float(%f)", "float(-1.5E-10)"));
    assert!(fmt("float(%f)", "float(.5)"));
    assert!(fmt("float(%f)", "float(3)"));
    assert!(!fmt("float(%f)", "float(abc)"));
    assert!(fmt("0x%x", "0xDEADbeef"));
    assert!(fmt("in %s on line %d", "in /a/b.php on line 3"));
    assert!(!fmt("in %s on line %d", "in \nx on line 3"));
    assert!(fmt("[%S]", "[]"));
    assert!(!fmt("[%s]", "[]"));
    assert!(fmt("a%cb", "a\nb"), "%c is any byte incl. newline (s flag)");
    assert!(fmt("dir%efile", "dir/file"));
    assert!(fmt("nul:%0.", "nul:\0."));
    assert!(fmt("id: %r[a-f0-9]{4}%r!", "id: c0de!"));
    assert!(!fmt("id: %r[a-f0-9]{4}%r!", "id: zzzz!"));
}

#[test]
fn multiline_wildcards() {
    assert!(has_multiline_wildcard(b"x%Ay"));
    assert!(has_multiline_wildcard(b"%w"));
    assert!(has_multiline_wildcard(b"%r.%r"));
    assert!(has_multiline_wildcard(b"a%cb"));
    assert!(!has_multiline_wildcard(b"%s %d %S %i %x %f %e %0"));
    assert!(fmt("start\n%A\nend", "start\nmany\nlines\nend"));
    assert!(fmt("start%wend", "start \n\t end"));
    assert!(fmt("a%ab", "a\n\nb"));
    assert!(!fmt("a%ab", "ab"), "%a needs at least one byte");
    assert!(fmt("a%Ab", "ab"));
    // Whole-string anchoring: no partial matches.
    assert!(!fmt("%d", "12a"));
    assert!(!fmt("x%A", "yx"));
}

#[test]
fn line_count_and_exact() {
    assert!(!fmt("a\nb", "a\nb\nc"));
    assert!(!fmt("a\nb\nc", "a\nb"));
    assert!(fmt("a\n%s\nc", "a\nb\nc"));
    assert!(matches(ExpectKind::Exact, b"a\nb", b"a\nb").unwrap());
    assert!(!matches(ExpectKind::Exact, b"a\nb", b"a\nb ").unwrap());
}

#[test]
fn expectregex() {
    assert!(matches(ExpectKind::Regex, b"a\\d+b", b"a123b").unwrap());
    assert!(!matches(ExpectKind::Regex, b"a\\d+b", b"xa123b").unwrap());
    assert!(matches(ExpectKind::Regex, b"(?i)hello.*", b"HELLO\nworld").unwrap());
    // Broken user regexes surface as errors, not panics.
    assert!(matches(ExpectKind::Regex, b"(", b"(").is_err());
    // PCRE dialect: a bare `{` is a literal, `\0` is NUL, `{,n}` is `{0,n}`.
    assert!(matches(ExpectKind::Regex, b"array\\(2\\) {\n}", b"array(2) {\n}").unwrap());
    assert!(matches(ExpectKind::Regex, b"a{2}b{1,}c{,2}d{1,2}", b"aabbdd").unwrap());
    assert!(matches(ExpectKind::Regex, b"x[\\0a]y", b"x\0y").unwrap());
    assert!(matches(ExpectKind::Regex, b"[{}]+", b"{}{").unwrap());
    assert!(matches(ExpectKind::Regex, b"[]a]+", b"]a]").unwrap());
    assert!(fmt(
        "string(5) \"%r\\0%rAB%r\\0%rc\"",
        "string(5) \"\0AB\0c\""
    ));
    assert_eq!(passthrough(b"a{b}c{2}[{]\\{"), "a\\{b}c{2}[{]\\{");
}

#[test]
fn non_utf8_bytes() {
    let wanted = [b'x', 0xE9, b'\n', b'%', b's'];
    let actual = [b'x', 0xE9, b'\n', 0xFF, 0xFE];
    assert!(matches(ExpectKind::Format, &wanted, &actual).unwrap());
    assert!(matches(ExpectKind::Exact, &wanted[..3], &actual[..3]).unwrap());
    // %s must not stop at a non-UTF-8 byte.
    assert!(fmt("string(2) \"%s\"", "string(2) \"\u{e9}\""));
}

#[test]
fn normalisation() {
    assert_eq!(
        normalize_output(b" \t\r\nhello\r\nworld\n\0\x0B"),
        b"hello\nworld"
    );
    assert_eq!(normalize_expected(b"\nab\r\n\r\n"), b"ab");
}

#[test]
fn diff_reports_first_line() {
    let d = diff(ExpectKind::Format, b"a\nint(%d)\nc", b"a\nint(x)\nc");
    assert_eq!(d.line, 2);
    assert_eq!(d.expected, "int(%d)");
    assert_eq!(d.actual, "int(x)");
    assert!(d.text.contains("002- int(%d)"), "{}", d.text);
    assert!(d.text.contains("002+ int(x)"), "{}", d.text);
    assert!(d.text.contains("001  a"), "{}", d.text);

    let d = diff(ExpectKind::Exact, b"a\nb", b"a\nb\nc");
    assert_eq!(d.line, 3);
    assert_eq!(d.expected, "<end of expected>");
    assert_eq!(d.actual, "c");
    assert!(d.text.contains("003+ c"), "{}", d.text);

    let d = diff(ExpectKind::Exact, b"a\nb\nc\nd", b"a\nc\nd");
    assert_eq!(d.line, 2);
    assert!(d.text.contains("002- b"), "{}", d.text);
    assert!(
        !d.text.contains("+ c"),
        "LCS keeps the common tail: {}",
        d.text
    );
}
