//! Snapshot tests for the mago → AST v2 adapter, one source per grammar
//! family under `tests/snapshots/<family>.php` (each `php -l`-clean; the
//! corpus job can lint the directory) pinned through
//! `rphp_ast::v2::pretty::print` in the `.ast` file next to it.
//!
//! `UPDATE_SNAPSHOTS=1 cargo test -p rphp-parser --test adapter_snapshots`
//! rewrites the `.ast` files; review the diff.
//!
//! The precedence family is additionally validated against PHP itself: each
//! expression is evaluated with a tiny interpreter over the tree and compared
//! with `php -r 'var_export(...)'` (skipped when `php` is not installed).

use std::path::{Path, PathBuf};
use std::process::Command;

use rphp_ast::v2::pretty;
use rphp_ast::v2::{BinOp, CastKind, Expr, InterpPart, Program, Stmt, UnOp};
use rphp_diagnostics::codes;
use rphp_intern::Interner;
use rphp_parser::{parse_v2, ParseOptions, Parsed};
use rphp_span::FileId;

fn snapshot_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots")
}

fn parse(src: &[u8]) -> (Parsed, Interner) {
    let mut interner = Interner::new();
    let parsed = parse_v2(src, ParseOptions::new(FileId(0)), &mut interner);
    (parsed, interner)
}

fn parse_clean(src: &[u8]) -> (Program, Interner) {
    let (parsed, interner) = parse(src);
    assert!(
        parsed.diagnostics.is_empty(),
        "unexpected diagnostics: {:?}",
        parsed.diagnostics
    );
    (parsed.program, interner)
}

#[test]
fn snapshots_match() {
    let dir = snapshot_dir();
    let mut sources: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "php"))
        .collect();
    sources.sort();
    assert!(sources.len() >= 17, "snapshot sources missing: {}", sources.len());
    let update = std::env::var_os("UPDATE_SNAPSHOTS").is_some();
    let mut failures = Vec::new();
    for src_path in sources {
        let src = std::fs::read(&src_path).unwrap();
        let (parsed, interner) = parse(&src);
        let mut actual = String::new();
        for d in &parsed.diagnostics {
            let loc = d
                .primary
                .as_ref()
                .map(|l| format!("{}..{}", l.span.lo, l.span.hi))
                .unwrap_or_default();
            actual.push_str(&format!("DIAG[{}] {} @{loc}\n", d.code, d.message));
        }
        actual.push_str(&pretty::print(&parsed.program, &interner));
        actual.push('\n');
        let ast_path = src_path.with_extension("ast");
        if update {
            std::fs::write(&ast_path, &actual).unwrap();
            continue;
        }
        let expected = std::fs::read_to_string(&ast_path)
            .unwrap_or_else(|_| panic!("missing {} (run with UPDATE_SNAPSHOTS=1)", ast_path.display()));
        if expected != actual {
            failures.push(format!(
                "{}:\n--- expected\n{expected}\n--- actual\n{actual}",
                src_path.display()
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Every snapshot source is accepted without diagnostics (they are all
/// `php -l`-clean) and every byte prefix of it parses without panicking.
#[test]
fn snapshot_sources_are_clean_and_prefix_safe() {
    for entry in std::fs::read_dir(snapshot_dir()).unwrap().flatten() {
        let p = entry.path();
        if !p.extension().is_some_and(|e| e == "php") {
            continue;
        }
        let src = std::fs::read(&p).unwrap();
        let (parsed, _) = parse(&src);
        assert!(
            parsed.diagnostics.is_empty(),
            "{}: {:?}",
            p.display(),
            parsed.diagnostics
        );
        for cut in 0..=src.len() {
            let mut interner = Interner::new();
            let _ = parse_v2(&src[..cut], ParseOptions::new(FileId(0)), &mut interner);
        }
    }
}

// ---------------------------------------------------------------------------
// Precedence validated against php
// ---------------------------------------------------------------------------

/// A PHP value the mini evaluator understands.
#[derive(Clone, Debug, PartialEq)]
enum Val {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Arr(Vec<(Val, Val)>),
}

impl Val {
    fn truthy(&self) -> bool {
        match self {
            Val::Null => false,
            Val::Bool(b) => *b,
            Val::Int(i) => *i != 0,
            Val::Float(f) => *f != 0.0,
            Val::Str(s) => !(s.is_empty() || s == "0"),
            Val::Arr(a) => !a.is_empty(),
        }
    }
    fn num(&self) -> Val {
        match self {
            Val::Null => Val::Int(0),
            Val::Bool(b) => Val::Int(i64::from(*b)),
            Val::Int(_) | Val::Float(_) => self.clone(),
            Val::Str(s) => {
                if let Ok(i) = s.trim().parse::<i64>() {
                    Val::Int(i)
                } else if let Ok(f) = s.trim().parse::<f64>() {
                    Val::Float(f)
                } else {
                    Val::Int(0)
                }
            }
            Val::Arr(_) => panic!("array in arithmetic"),
        }
    }
    fn int(&self) -> i64 {
        match self.num() {
            Val::Int(i) => i,
            Val::Float(f) => f as i64,
            _ => unreachable!(),
        }
    }
    fn float(&self) -> f64 {
        match self.num() {
            Val::Int(i) => i as f64,
            Val::Float(f) => f,
            _ => unreachable!(),
        }
    }
    fn string(&self) -> String {
        match self {
            Val::Null => String::new(),
            Val::Bool(true) => "1".into(),
            Val::Bool(false) => String::new(),
            Val::Int(i) => i.to_string(),
            Val::Float(f) => {
                if f.fract() == 0.0 && f.abs() < 1e15 {
                    format!("{}", *f as i64)
                } else {
                    format!("{f}")
                }
            }
            Val::Str(s) => s.clone(),
            Val::Arr(_) => "Array".into(),
        }
    }
    /// `var_export` rendering.
    fn export(&self) -> String {
        match self {
            Val::Null => "NULL".into(),
            Val::Bool(b) => b.to_string(),
            Val::Int(i) => i.to_string(),
            Val::Float(f) => {
                if f.fract() == 0.0 && f.abs() < 1e15 {
                    format!("{}.0", *f as i64)
                } else if f.abs() >= 1e15 {
                    // PHP's `%.17G`-style: `9.223372036854776E+18`.
                    let e = format!("{f:E}");
                    let (m, exp) = e.split_once('E').unwrap();
                    let sign = if exp.starts_with('-') { "" } else { "+" };
                    format!("{m}E{sign}{exp}")
                } else {
                    format!("{f}")
                }
            }
            Val::Str(s) => format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'")),
            Val::Arr(_) => panic!("array export not supported"),
        }
    }
    fn is_float(&self) -> bool {
        matches!(self, Val::Float(_))
    }
}

fn arith(op: BinOp, a: &Val, b: &Val) -> Val {
    let (a, b) = (a.num(), b.num());
    let float = a.is_float() || b.is_float() || op == BinOp::Div;
    if float {
        let (x, y) = (a.float(), b.float());
        return Val::Float(match op {
            BinOp::Add => x + y,
            BinOp::Sub => x - y,
            BinOp::Mul => x * y,
            BinOp::Div => {
                if x % y == 0.0 && !a.is_float() && !b.is_float() {
                    return Val::Int((x / y) as i64);
                }
                x / y
            }
            BinOp::Pow => x.powf(y),
            _ => unreachable!(),
        });
    }
    let (x, y) = (a.int(), b.int());
    match op {
        BinOp::Add => x.checked_add(y).map_or(Val::Float(x as f64 + y as f64), Val::Int),
        BinOp::Sub => x.checked_sub(y).map_or(Val::Float(x as f64 - y as f64), Val::Int),
        BinOp::Mul => x.checked_mul(y).map_or(Val::Float(x as f64 * y as f64), Val::Int),
        BinOp::Pow => {
            if y < 0 {
                Val::Float((x as f64).powf(y as f64))
            } else {
                x.checked_pow(y as u32).map_or(Val::Float((x as f64).powf(y as f64)), Val::Int)
            }
        }
        _ => unreachable!(),
    }
}

fn compare(a: &Val, b: &Val) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    fn numeric(s: &str) -> bool {
        let t = s.trim();
        t.parse::<i64>().is_ok() || t.parse::<f64>().is_ok()
    }
    match (a, b) {
        (Val::Str(x), Val::Str(y)) => {
            if numeric(x) && numeric(y) {
                a.float().partial_cmp(&b.float()).unwrap_or(Ordering::Equal)
            } else {
                x.cmp(y)
            }
        }
        // PHP 8: a non-numeric string compares with a number as strings.
        (Val::Str(x), Val::Int(_) | Val::Float(_)) if !numeric(x) => x.cmp(&b.string()),
        (Val::Int(_) | Val::Float(_), Val::Str(y)) if !numeric(y) => a.string().cmp(y),
        (Val::Bool(_), _) | (_, Val::Bool(_)) | (Val::Null, _) | (_, Val::Null) => {
            a.truthy().cmp(&b.truthy())
        }
        _ => a.float().partial_cmp(&b.float()).unwrap_or(Ordering::Equal),
    }
}

fn eval(e: &Expr, interner: &Interner) -> Val {
    match e {
        Expr::Null(_) => Val::Null,
        Expr::Bool(b, _) => Val::Bool(*b),
        Expr::Int(i, _) => Val::Int(*i),
        Expr::Float(f, _) => Val::Float(*f),
        Expr::Str(s, _) => Val::Str(String::from_utf8_lossy(interner.resolve(*s)).into_owned()),
        Expr::Interp { parts, .. } => Val::Str(
            parts
                .iter()
                .map(|p| match p {
                    InterpPart::Lit(id, _) => String::from_utf8_lossy(interner.resolve(*id)).into_owned(),
                    InterpPart::Expr(e) => eval(e, interner).string(),
                })
                .collect(),
        ),
        Expr::Array { items, .. } => Val::Arr(
            items
                .iter()
                .enumerate()
                .map(|(i, it)| {
                    let k = it.key.as_ref().map_or(Val::Int(i as i64), |k| eval(k, interner));
                    (k, eval(it.value.as_ref().unwrap(), interner))
                })
                .collect(),
        ),
        Expr::Index { base, index, .. } => {
            let Val::Arr(items) = eval(base, interner) else { panic!("index on non-array") };
            let key = eval(index.as_ref().unwrap(), interner);
            items
                .into_iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v)
                .unwrap_or(Val::Null)
        }
        Expr::Unary { op, expr, .. } => {
            let v = eval(expr, interner);
            match op {
                UnOp::Neg => arith(BinOp::Mul, &v, &Val::Int(-1)),
                UnOp::Plus => v.num(),
                UnOp::Not => Val::Bool(!v.truthy()),
                UnOp::BitNot => Val::Int(!v.int()),
                UnOp::Cast(CastKind::Int) => Val::Int(v.int()),
                UnOp::Cast(CastKind::Float) => Val::Float(v.float()),
                UnOp::Cast(CastKind::String) => Val::Str(v.string()),
                UnOp::Cast(CastKind::Bool) => Val::Bool(v.truthy()),
                other => panic!("unsupported unary {other:?}"),
            }
        }
        Expr::Binary { op, lhs, rhs, .. } => {
            let a = eval(lhs, interner);
            match op {
                BinOp::And => return Val::Bool(a.truthy() && eval(rhs, interner).truthy()),
                BinOp::Or => return Val::Bool(a.truthy() || eval(rhs, interner).truthy()),
                BinOp::Coalesce => {
                    return if a == Val::Null { eval(rhs, interner) } else { a };
                }
                _ => {}
            }
            let b = eval(rhs, interner);
            match op {
                BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Pow => arith(*op, &a, &b),
                BinOp::Mod => Val::Int(a.int() % b.int()),
                BinOp::Concat => Val::Str(a.string() + &b.string()),
                BinOp::BitAnd => Val::Int(a.int() & b.int()),
                BinOp::BitOr => Val::Int(a.int() | b.int()),
                BinOp::BitXor => Val::Int(a.int() ^ b.int()),
                BinOp::Shl => Val::Int(a.int() << b.int()),
                BinOp::Shr => Val::Int(a.int() >> b.int()),
                BinOp::Eq => Val::Bool(compare(&a, &b).is_eq()),
                BinOp::Ne => Val::Bool(!compare(&a, &b).is_eq()),
                BinOp::Identical => Val::Bool(a == b),
                BinOp::NotIdentical => Val::Bool(a != b),
                BinOp::Lt => Val::Bool(compare(&a, &b).is_lt()),
                BinOp::Le => Val::Bool(compare(&a, &b).is_le()),
                BinOp::Gt => Val::Bool(compare(&a, &b).is_gt()),
                BinOp::Ge => Val::Bool(compare(&a, &b).is_ge()),
                BinOp::Spaceship => Val::Int(compare(&a, &b) as i64),
                BinOp::Xor => Val::Bool(a.truthy() ^ b.truthy()),
                other => panic!("unsupported binary {other:?}"),
            }
        }
        Expr::Ternary {
            cond, then, else_, ..
        } => {
            let c = eval(cond, interner);
            if c.truthy() {
                then.as_ref().map_or(c, |t| eval(t, interner))
            } else {
                eval(else_, interner)
            }
        }
        other => panic!("unsupported expression {other:?}"),
    }
}

fn php_available() -> bool {
    Command::new("php").arg("-v").output().is_ok_and(|o| o.status.success())
}

fn php_export(expr: &str) -> String {
    let out = Command::new("php")
        .args(["-n", "-r", &format!("var_export({expr});")])
        .output()
        .expect("php");
    assert!(out.status.success(), "php failed on {expr}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Expressions whose value depends on precedence and associativity; each is
/// parsed, evaluated over the tree, and compared with `php -r`.
const PRECEDENCE_CASES: &[&str] = &[
    "1 + 2 * 3 ** 2 . \"x\"",
    "-2 ** 2",
    "2 ** 3 ** 2",
    "1 - 2 - 3",
    "(int) [\"x\" => \"7\"][\"x\"] + 1",
    "(int) \"3.5\" ** 2",
    "1 . 2 + 3",
    "1 + 2 . 3 + 4",
    "null ?? 0 ?: 5",
    "null ?? 2 ?: 5",
    "1 << 2 + 1",
    "1 | 2 ^ 3 & 4",
    "!1 == 0",
    "-3 % 2",
    "2 ** -1",
    "7 <=> 3",
    "\"a\" . 1 + 2",
    "1 + 1 == 2 && 3 > 2",
    "0 || 2 and 0",
    "~5 & 12",
    "10 / 4 * 2",
    "5 % 3 * 2",
    "2 + 3 ?? 1",
    "1 < 2 == true",
    "-1 ** 2 + 1",
    "1 ? 2 : (0 ? 3 : 4)",
    "\"5\" + \"5\" . \"5\"",
    "true xor true or true",
    "!false && !true",
    "9223372036854775807 + 1",
    "\"abc\" == 0",
    "\"1e3\" == \"1000\"",
    "2.5 * 2",
    "7 % 3 ** 2",
    "\"a\" . 2 ** 2 . \"b\"",
];

#[test]
fn precedence_matches_php_evaluation() {
    if !php_available() {
        eprintln!("php not available: precedence evaluation check skipped");
        return;
    }
    for case in PRECEDENCE_CASES {
        let src = format!("<?php {case};");
        let (program, interner) = parse_clean(src.as_bytes());
        let Stmt::Expr { expr, .. } = &program.items[0] else { panic!("{case}") };
        let ours = eval(expr, &interner).export();
        let php = php_export(case);
        assert_eq!(ours, php, "precedence mismatch for `{case}`");
    }
}

// ---------------------------------------------------------------------------
// Entry-point contract
// ---------------------------------------------------------------------------

#[test]
fn ident_spans_cover_keyword_positions() {
    let src = b"<?php $o->list(); f(class: 1); class C { function new() {} const X = 1; } enum E { case A; } goto end; end: A::CLASS; T::m;";
    let (parsed, _) = parse(src);
    let spelled: Vec<&str> = parsed
        .ident_spans
        .iter()
        .map(|&(lo, hi)| std::str::from_utf8(&src[lo as usize..hi as usize]).unwrap())
        .collect();
    for expected in ["list", "class", "new", "X", "A", "end", "end", "CLASS", "m"] {
        assert!(spelled.contains(&expected), "{expected} missing from {spelled:?}");
    }
    assert!(!spelled.contains(&"C"), "class names are not reclassified");
}

#[test]
fn short_open_tag_option() {
    let src = b"<? echo 1; ?><?= 2 ?>";
    let (parsed, _) = parse(src);
    assert_eq!(parsed.program.items.len(), 2, "short tags on by default");
    let mut interner = Interner::new();
    let opts = ParseOptions {
        file: FileId(0),
        path: None,
        short_open_tag: false,
    };
    let parsed = parse_v2(src, opts, &mut interner);
    assert!(parsed.diagnostics.is_empty());
    assert!(matches!(parsed.program.items[0], Stmt::InlineHtml { .. }));
    assert!(matches!(parsed.program.items[1], Stmt::Echo { .. }));
}

#[test]
fn strict_types_and_halt_offset() {
    let (parsed, _) = parse(b"<?php\ndeclare(strict_types=1);\necho 1;\n__halt_compiler();DATA");
    assert!(parsed.diagnostics.is_empty());
    assert!(parsed.program.strict_types);
    assert_eq!(parsed.program.halt_offset, Some(57));
    let (parsed, _) = parse(b"<?php declare(strict_types=0);");
    assert!(!parsed.program.strict_types);
    let (parsed, _) = parse(b"<?php echo 1; declare(strict_types=1);");
    assert_eq!(parsed.diagnostics[0].code, codes::STRICT_TYPES);
    assert_eq!(
        parsed.diagnostics[0].message,
        "strict_types declaration must be the very first statement in the script"
    );
}

#[test]
fn mago_errors_map_to_codes() {
    let cases: &[(&[u8], &str)] = &[
        (b"<?php $a = ;", codes::UNEXPECTED_TOKEN),
        (b"<?php $a = 'x", codes::UNTERMINATED),
        (b"<?php function f( {", codes::UNEXPECTED_EOF),
        (b"<?php $d = 09;", codes::UNEXPECTED_CHAR),
        (b"<?php $s = \"\\u{ZZ}\";", codes::INVALID_LITERAL),
        (b"<?php f(?, 1);", codes::SUPERSET_REJECTED),
        (b"<?php $a?->b = 1;", codes::LVALUE_NULLSAFE),
        (b"<?php $this = 1;", codes::LVALUE_THIS),
        (b"<?php $GLOBALS = [];", codes::LVALUE_GLOBALS),
        (b"<?php echo $a[];", codes::LVALUE_APPEND),
        (b"<?php [$a, 'k' => $b] = $c;", codes::LVALUE_DESTRUCTURING),
        (b"<?php [f()] = $a;", codes::LVALUE_NOT_WRITABLE),
    ];
    for (src, code) in cases {
        let (parsed, _) = parse(src);
        let codes_seen: Vec<&str> = parsed.diagnostics.iter().map(|d| d.code).collect();
        assert!(
            codes_seen.contains(code),
            "{}: expected {code}, got {codes_seen:?}",
            String::from_utf8_lossy(src)
        );
    }
}

/// mago caps expression nesting at 512 levels (`RPHP_E0005`); the parse of a
/// 600-deep parenthesization needs a few MB of stack in debug builds, so it
/// runs on its own thread (xtask and the CLI give the front end a large
/// stack for the same reason).
#[test]
fn recursion_limit_is_reported() {
    let handle = std::thread::Builder::new()
        .stack_size(64 << 20)
        .spawn(|| {
            let deep = format!("<?php $x = {}1{};", "(".repeat(600), ")".repeat(600));
            let (parsed, _) = parse(deep.as_bytes());
            assert!(parsed.diagnostics.iter().any(|d| d.code == codes::RECURSION_LIMIT));
        })
        .unwrap();
    handle.join().unwrap();
}

/// Left-associative chains are folded iteratively: a 20000-term
/// concatenation parses on a 2 MB stack.
#[test]
fn long_left_associative_chains_do_not_recurse() {
    let handle = std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(|| {
            let src = format!("<?php $x = \"a\"{};", " . \"b\"".repeat(20000));
            let (program, _) = parse_clean(src.as_bytes());
            let Stmt::Expr { expr: Expr::Assign { value, .. }, .. } = &program.items[0] else { panic!() };
            let mut depth = 0;
            let mut cur: &Expr = value;
            while let Expr::Binary { lhs, .. } = cur {
                depth += 1;
                cur = lhs;
            }
            assert_eq!(depth, 20000);
            // Dropping a 20000-deep `Box<Expr>` chain recurses in the drop glue
            // (an `rphp-ast` concern, not the adapter's); leak it here.
            std::mem::forget(program);
        })
        .unwrap();
    handle.join().unwrap();
}

#[test]
fn conformance_messages_are_phps() {
    let cases: &[(&[u8], &str)] = &[
        (b"<?php (real) $a;", "The (real) cast has been removed, use (float) instead"),
        (b"<?php (unset) $a;", "The (unset) cast is no longer supported"),
        (b"<?php f(a: 1, 2);", "Cannot use positional argument after named argument"),
        (b"<?php $f = function() use ($this) {};", "Cannot use $this as lexical variable"),
        (b"<?php $a = [1, , 2];", "Cannot use empty array elements in arrays"),
        (b"<?php echo match(1) { default => 1, default => 2 };", "Match expressions may only contain one default arm"),
        (b"<?php $a ||= 1;", "Unexpected token `Equal`"),
        (b"<?php class C { function __construct(): void {} }", "Method C::__construct() cannot declare a return type"),
        (b"<?php function f(): void { return 1; }", "A void function must not return a value"),
        (b"<?php yield 1;", "The \"yield\" expression can only be used inside a function"),
        (b"<?php while(1) { break 2; }", "Cannot 'break' 2 levels"),
        (b"<?php namespace A; namespace B {}", "Cannot mix bracketed namespace declarations with unbracketed namespace declarations"),
        (b"<?php enum E { case A = 1; }", "Case A of non-backed enum E must not have a value"),
    ];
    for (src, message) in cases {
        let (parsed, _) = parse(src);
        assert!(
            parsed.diagnostics.iter().any(|d| d.message == *message),
            "{}: {:?}",
            String::from_utf8_lossy(src),
            parsed.diagnostics.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }
}

#[test]
fn dead_code_after_constant_false_is_not_validated() {
    // Zend folds `false && rhs` and never compiles rhs (assert() AST-printing tests rely on it).
    let (parsed, _) = parse(b"<?php assert(false && new class { public $p { get; } });");
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let (parsed, _) = parse(b"<?php assert(true && new class { public $p { get; } });");
    assert!(!parsed.diagnostics.is_empty());
}
