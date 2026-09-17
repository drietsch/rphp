//! `divergences.toml` parsing, validation and glob matching.

use rphp_test::differential::{glob_match, Allowlist, AllowlistError, Category, NormalizeContext, Rule};

const VALID: &str = r#"
# comment
[[allow]]
snippet   = "**/fatal-*.php"
category  = "error-wording"
reason    = "M0 CLI renders uncaught errors differently"
normalize = "drop-lines:^(PHP )?Fatal error: "

[[allow]]
snippet   = "spl/ids.php"
category  = "ObjectId"
reason    = "handles depend on allocation order"

[[allow]]
snippet   = "misc/pid.php"
category  = "pid"
reason    = "getmypid() is a bare integer"
normalize = "regex:pid=(\\d+) => pid=%PID%"
"#;

#[test]
fn parses_valid_entries_in_order() {
    let list = Allowlist::parse(VALID).unwrap();
    assert_eq!(list.entries().len(), 3);
    assert_eq!(list.entries()[0].category, Category::ErrorWording);
    assert!(matches!(list.entries()[0].rule, Rule::DropLines(_)));
    assert_eq!(list.entries()[1].category, Category::ObjectId);
    assert!(matches!(list.entries()[1].rule, Rule::CategoryDefault));
    assert_eq!(list.entries()[2].category, Category::Pid);
    assert!(matches!(list.entries()[2].rule, Rule::Replace { .. }));
    assert!(list.root().is_none());
}

#[test]
fn empty_and_comment_only_files_are_valid() {
    assert!(Allowlist::parse("").unwrap().entries().is_empty());
    assert!(Allowlist::parse("# nothing yet\n").unwrap().entries().is_empty());
    assert!(Allowlist::empty().entries().is_empty());
}

#[test]
fn unknown_category_is_rejected_with_the_closed_set() {
    let src = "[[allow]]\nsnippet = \"a.php\"\ncategory = \"clock-skew\"\nreason = \"x\"\n";
    let err = Allowlist::parse(src).unwrap_err();
    assert!(matches!(err, AllowlistError::UnknownCategory { index: 0, ref category, .. } if category == "clock-skew"), "{err:?}");
    let msg = err.to_string();
    assert!(msg.contains("clock-skew") && msg.contains("float-format") && msg.contains("tempnam-path"), "{msg}");
    // Case matters: the set is spelled one way.
    let src = "[[allow]]\nsnippet = \"a.php\"\ncategory = \"Object-Id\"\nreason = \"x\"\n";
    assert!(matches!(Allowlist::parse(src), Err(AllowlistError::UnknownCategory { .. })));
}

#[test]
fn every_entry_must_cite_a_reason_and_a_snippet() {
    let src = "[[allow]]\nsnippet = \"a.php\"\ncategory = \"timing\"\nreason = \"  \"\n";
    assert!(matches!(Allowlist::parse(src), Err(AllowlistError::EmptyReason { index: 0, .. })));
    let src = "[[allow]]\nsnippet = \"\"\ncategory = \"timing\"\nreason = \"x\"\n";
    assert!(matches!(Allowlist::parse(src), Err(AllowlistError::EmptySnippet { index: 0 })));
    let src = "[[allow]]\ncategory = \"timing\"\nreason = \"x\"\n";
    assert!(matches!(Allowlist::parse(src), Err(AllowlistError::Toml(_))), "missing snippet");
}

#[test]
fn unknown_keys_and_bad_toml_are_rejected() {
    let src = "[[allow]]\nsnippet = \"a.php\"\ncategroy = \"timing\"\nreason = \"x\"\n";
    let err = Allowlist::parse(src).unwrap_err();
    assert!(matches!(err, AllowlistError::Toml(_)), "{err}");
    assert!(matches!(Allowlist::parse("[[allow"), Err(AllowlistError::Toml(_))));
    assert!(matches!(Allowlist::parse("[[allow]]\nsnippet = 1\ncategory = \"timing\"\nreason = \"x\"\n"), Err(AllowlistError::Toml(_))));
}

#[test]
fn normalize_rules_are_validated() {
    let mk = |rule: &str| format!("[[allow]]\nsnippet = \"a.php\"\ncategory = \"timing\"\nreason = \"x\"\nnormalize = '{rule}'\n");
    assert!(matches!(Allowlist::parse(&mk("frobnicate:x")), Err(AllowlistError::BadRule { index: 0, .. })));
    assert!(matches!(Allowlist::parse(&mk("drop-lines:(")), Err(AllowlistError::BadRule { .. })));
    assert!(matches!(Allowlist::parse(&mk("regex:a b")), Err(AllowlistError::BadRule { .. })), "missing ` => `");
    assert!(matches!(Allowlist::parse(&mk("regex:( => x")), Err(AllowlistError::BadRule { .. })));
    assert!(Allowlist::parse(&mk("regex:a => ")).is_ok(), "empty replacement is fine");
    assert!(Allowlist::parse(&mk("")).is_ok());
}

#[test]
fn categories_without_default_need_an_explicit_rule() {
    for cat in ["locale", "platform-value", "pid"] {
        let src = format!("[[allow]]\nsnippet = \"a.php\"\ncategory = \"{cat}\"\nreason = \"x\"\n");
        let err = Allowlist::parse(&src).unwrap_err();
        assert!(matches!(err, AllowlistError::NoDefaultNormalizer { index: 0, .. }), "{cat}: {err}");
        let src = format!("[[allow]]\nsnippet = \"a.php\"\ncategory = \"{cat}\"\nreason = \"x\"\nnormalize = \"drop-lines:^x\"\n");
        assert!(Allowlist::parse(&src).is_ok(), "{cat} with explicit rule");
    }
}

#[test]
fn entries_apply_by_glob_and_in_file_order() {
    let list = Allowlist::parse(VALID).unwrap();
    assert_eq!(list.entries_for("lang/fatal-undefined-function.php").len(), 1);
    assert_eq!(list.entries_for("fatal-x.php").len(), 1, "`**/` also matches zero directories");
    assert_eq!(list.entries_for("lang/basics.php").len(), 0);
    assert_eq!(list.categories_for("spl/ids.php"), vec![Category::ObjectId]);
    assert_eq!(list.categories_for("misc/pid.php"), vec![Category::Pid]);

    let ctx = NormalizeContext::new();
    let out = list.normalize("lang/fatal-a.php", b"x\nPHP Fatal error:  boom\nFatal error: boom\ny\n", &ctx);
    assert_eq!(out, b"x\ny\n");
    let out = list.normalize("misc/pid.php", b"pid=4242 pid=7\n", &ctx);
    assert_eq!(out, b"pid=%PID% pid=%PID%\n");
    let out = list.normalize("spl/ids.php", b"object(A)#7 (0) {\n}\n", &ctx);
    assert_eq!(out, b"object(A)#N (0) {\n}\n");
    // No matching entry: untouched.
    assert_eq!(list.normalize("lang/basics.php", b"object(A)#7", &ctx), b"object(A)#7");
}

#[test]
fn stale_entries_are_reported() {
    let list = Allowlist::parse(VALID).unwrap();
    let names = vec!["lang/fatal-x.php".to_string(), "spl/ids.php".to_string()];
    let stale = list.unmatched(&names);
    assert_eq!(stale.len(), 1);
    assert_eq!(stale[0].snippet, "misc/pid.php");
}

#[test]
fn load_sets_the_root_and_relative_names_follow_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("divergences.toml");
    std::fs::write(&path, VALID).unwrap();
    let list = Allowlist::load(&path).unwrap();
    assert_eq!(list.root(), Some(dir.path()));
    assert_eq!(list.relative_name(&dir.path().join("lang").join("x.php")), "lang/x.php");
    assert_eq!(list.relative_name(std::path::Path::new("/elsewhere/y.php")), "y.php");
    let missing = Allowlist::load(&dir.path().join("nope.toml")).unwrap_err();
    assert!(matches!(missing, AllowlistError::Io { .. }));
    assert!(missing.to_string().contains("nope.toml"));
}

#[test]
fn glob_semantics() {
    assert!(glob_match("a/b.php", "a/b.php"));
    assert!(!glob_match("a/b.php", "a/b.phpx"));
    assert!(glob_match("*/b.php", "a/b.php"));
    assert!(!glob_match("*/b.php", "a/c/b.php"), "`*` stays within one segment");
    assert!(glob_match("**/b.php", "a/c/b.php"));
    assert!(glob_match("**/b.php", "b.php"));
    assert!(glob_match("lang/*.php", "lang/x.php"));
    assert!(!glob_match("lang/*.php", "math/x.php"));
    assert!(glob_match("lang/?.php", "lang/x.php"));
    assert!(!glob_match("lang/?.php", "lang/xy.php"));
    assert!(glob_match("**", "anything/at/all"));
    assert!(!glob_match("", "x"));
    assert!(glob_match("", ""));
}
