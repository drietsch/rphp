//! The closed category set and each default normalizer.

use std::path::PathBuf;

use rphp_test::differential::normalize::replace_bytes;
use rphp_test::differential::{Category, NormalizeContext};

fn norm(cat: Category, input: &str) -> String {
    String::from_utf8(cat.normalize(input.as_bytes(), &NormalizeContext::new())).unwrap()
}

#[test]
fn category_set_is_closed_and_uniquely_named() {
    assert_eq!(Category::ALL.len(), 12);
    let mut names: Vec<&str> = Category::ALL.iter().map(|c| c.name()).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), 12, "kebab-case names must be unique");
    for c in Category::ALL {
        assert_eq!(Category::parse(c.name()), Some(c));
        assert_eq!(Category::parse(c.variant_name()), Some(c));
        assert_eq!(c.to_string(), c.name());
    }
    assert_eq!(Category::parse("OBJECT-ID"), None, "no case folding: the set is spelled one way");
    assert_eq!(Category::parse("wibble"), None);
    assert_eq!(Category::parse(""), None);
}

#[test]
fn categories_without_default_are_identity() {
    for c in [Category::Locale, Category::PlatformValue, Category::Pid] {
        assert!(!c.has_default(), "{c}");
        assert_eq!(norm(c, "pid 12345 12.5 ms"), "pid 12345 12.5 ms");
    }
    for c in Category::ALL {
        if !matches!(c, Category::Locale | Category::PlatformValue | Category::Pid) {
            assert!(c.has_default(), "{c}");
        }
    }
}

#[test]
fn float_format_collapses_decimal_and_exponent_literals() {
    assert_eq!(
        norm(Category::FloatFormat, "float(0.30000000000000004)\n3.5 -2.25 1.0E+25 1E-7 int(42)\n"),
        "float(%f)\n%f %f %f %f int(42)\n"
    );
}

#[test]
fn object_id_rewrites_var_dump_handles_only() {
    let input = "object(Foo)#3 (1) {\n  [\"a\"]=>\n  object(Closure)#12 (0) {\n  }\n}\n#0 {main}\n";
    assert_eq!(
        norm(Category::ObjectId, input),
        "object(Foo)#N (1) {\n  [\"a\"]=>\n  object(Closure)#N (0) {\n  }\n}\n#0 {main}\n"
    );
}

#[test]
fn resource_id_rewrites_both_spellings() {
    assert_eq!(
        norm(Category::ResourceId, "resource(5) of type (stream)\nResource id #12\n"),
        "resource(N) of type (stream)\nResource id #N\n"
    );
}

#[test]
fn timing_and_tzdb_placeholders() {
    assert_eq!(norm(Category::Timing, "took 12.5 ms, 0.003s, 7us, 3 seconds; 5 items"), "took %TIMING%, %TIMING%, %TIMING%, %TIMING%; 5 items");
    assert_eq!(norm(Category::TzdbVersion, "tzdb 2025.2 / 2025b / year 2025"), "tzdb %TZDB% / %TZDB% / year 2025");
}

#[test]
fn tempnam_paths_collapse() {
    assert_eq!(norm(Category::TempnamPath, "wrote /tmp/phpAbC123 and /private/var/folders/x1/T/phpZZ ok"), "wrote %TEMPNAM% and %TEMPNAM% ok");
    let sys = std::env::temp_dir().join("phpXYZ");
    let out = norm(Category::TempnamPath, &format!("f={}", sys.display()));
    assert_eq!(out, "f=%TEMPNAM%");
}

#[test]
fn path_replaces_longest_first() {
    let ctx = NormalizeContext::for_script(&PathBuf::from("/work/tier-a/lang/x.php"), &PathBuf::from("/tmp/run/php"));
    assert_eq!(ctx.paths, vec![PathBuf::from("/work/tier-a/lang/x.php"), PathBuf::from("/work/tier-a/lang"), PathBuf::from("/tmp/run/php")]);
    let out = Category::Path.normalize(b"in /work/tier-a/lang/x.php, dir /work/tier-a/lang, cwd /tmp/run/php, other /work/tier-a/other.php", &ctx);
    assert_eq!(String::from_utf8(out).unwrap(), "in %PATH%, dir %PATH%, cwd %PATH%, other /work/tier-a/other.php");
    // The empty context leaves everything alone; "/" is never a needle.
    let out = Category::Path.normalize(b"/a/b", &NormalizeContext::new().with_path("/"));
    assert_eq!(out, b"/a/b");
}

#[test]
fn hash_order_sorts_lines_keeping_trailing_newline_state() {
    assert_eq!(norm(Category::HashOrder, "b\nc\na\n"), "a\nb\nc\n");
    assert_eq!(norm(Category::HashOrder, "b\na"), "a\nb");
    assert_eq!(norm(Category::HashOrder, ""), "");
    assert_eq!(norm(Category::HashOrder, "\n"), "\n");
}

#[test]
fn error_wording_keeps_level_and_class_drops_prose_and_trace() {
    let display = "\nFatal error: Uncaught DivisionByZeroError: Modulo by zero in /x/y.php:3\nStack trace:\n#0 /x/y.php(3): f(1)\n#1 {main}\n  thrown in /x/y.php on line 3\n";
    assert_eq!(norm(Category::ErrorWording, display), "\nFatal error: Uncaught DivisionByZeroError: %MSG%\n");
    let log = "PHP Warning:  Undefined variable $x in /x/y.php on line 2\nPHP Fatal error:  Uncaught Error: boom in /x:1\n";
    assert_eq!(norm(Category::ErrorWording, log), "Warning: %MSG%\nFatal error: Uncaught Error: %MSG%\n");
    // Namespaced classes keep their name; ordinary lines are untouched.
    assert_eq!(norm(Category::ErrorWording, "Fatal error: Uncaught App\\Err: x in f:1\nhello\n"), "Fatal error: Uncaught App\\Err: %MSG%\nhello\n");
}

#[test]
fn replace_bytes_is_non_overlapping_and_binary_safe() {
    assert_eq!(replace_bytes(b"aaa", b"aa", b"X"), b"Xa");
    assert_eq!(replace_bytes(b"\xff-\xff", b"\xff", b"?"), b"?-?");
    assert_eq!(replace_bytes(b"abc", b"", b"X"), b"abc");
}
