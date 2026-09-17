//! Differential test against the installed `php` (the oracle): token id, line
//! and length must match byte for byte, in both `short_open_tag` modes.
//!
//! * `mini_corpus_matches_php`: the in-repo corpus `tests/corpus/*.php`, one
//!   file per scanner quirk. Must be 100 %.
//! * `external_corpus_matches_php`: only with `RPHP_TOKENIZER_CORPUS=<dir>`
//!   (e.g. a Composer `vendor/` tree). Walks the tree, runs the oracle in
//!   parallel batches of 200 files, prints pass/fail counts, a histogram of
//!   mismatch classes and the first 10 mismatches in detail, then asserts
//!   100 %. `RPHP_TOKENIZER_JOBS` caps the thread count.
//!
//! `RPHP_PHP` points at a specific php binary; without any php the tests are
//! skipped with a message rather than failed.

use rphp_tokenizer::{token_name, tokenize, Options};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

const DUMP_SCRIPT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/dump_tokens.php");
const BATCH: usize = 200;
const SHOW: usize = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Expected {
    id: u16,
    line: u32,
    len: u32,
}

fn php_binary() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("RPHP_PHP") {
        return Some(PathBuf::from(p));
    }
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|d| d.join("php"))
        .find(|p| p.is_file())
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            walk(&p, out);
        } else if p.extension().is_some_and(|e| e == "php") {
            out.push(p);
        }
    }
}

/// Run the oracle over `files`; returns `(path, tokens)` per file, `None` for
/// files php could not read.
fn php_dump(
    php: &Path,
    files: &[PathBuf],
    short_open_tag: bool,
) -> Vec<(PathBuf, Option<Vec<Expected>>)> {
    let mut child = Command::new(php)
        .arg("-n")
        .arg("-d")
        .arg(format!("short_open_tag={}", u8::from(short_open_tag)))
        .arg("-d")
        .arg("error_reporting=0")
        .arg("-d")
        .arg("display_errors=0")
        .arg("-d")
        .arg("memory_limit=-1")
        .arg(DUMP_SCRIPT)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn php");
    let mut stdin = child.stdin.take().unwrap();
    let paths: Vec<String> = files.iter().map(|f| f.display().to_string()).collect();
    let writer = std::thread::spawn(move || {
        for p in paths {
            let _ = writeln!(stdin, "{p}");
        }
    });
    let out = child.wait_with_output().expect("php output");
    writer.join().unwrap();
    assert!(out.status.success(), "php exited with {}", out.status);

    let text = String::from_utf8_lossy(&out.stdout);
    let mut result: Vec<(PathBuf, Option<Vec<Expected>>)> = Vec::with_capacity(files.len());
    for line in text.lines() {
        if let Some(p) = line.strip_prefix("== ") {
            result.push((PathBuf::from(p), Some(Vec::new())));
        } else if line == "!! unreadable" {
            if let Some(last) = result.last_mut() {
                last.1 = None;
            }
        } else if let Some((_, Some(toks))) = result.last_mut() {
            let mut f = line.split('\t');
            let (Some(id), Some(ln), Some(len)) = (f.next(), f.next(), f.next()) else {
                panic!("bad dump line {line:?}");
            };
            toks.push(Expected {
                id: id.parse().unwrap(),
                line: ln.parse().unwrap(),
                len: len.parse().unwrap(),
            });
        }
    }
    assert_eq!(
        result.len(),
        files.len(),
        "oracle dumped {} of {} files",
        result.len(),
        files.len()
    );
    result
}

fn name(id: u16) -> String {
    match token_name(id) {
        Some(n) => n.to_string(),
        None => format!("'{}'", (id as u8 as char).escape_default()),
    }
}

fn excerpt(src: &[u8], at: usize) -> String {
    let lo = at.saturating_sub(40);
    let hi = (at + 40).min(src.len());
    let show = |b: &[u8]| String::from_utf8_lossy(b).escape_debug().to_string();
    format!(
        "…{}⟦HERE⟧{}…",
        show(&src[lo..at.min(src.len())]),
        show(&src[at.min(src.len())..hi])
    )
}

/// A one-line class for the mismatch histogram, plus the detailed report.
struct Mismatch {
    class: String,
    report: String,
}

fn compare(path: &Path, src: &[u8], expected: &[Expected], opts: Options) -> Option<Mismatch> {
    let actual = tokenize(src, opts);
    let n = expected.len().max(actual.len());
    let mut off_php = 0usize;
    for i in 0..n {
        let e = expected.get(i).copied();
        let a = actual.get(i).copied();
        let same = matches!((e, a), (Some(e), Some(a)) if e.id == a.id && e.line == a.line && e.len == a.hi - a.lo);
        if same {
            off_php += e.unwrap().len as usize;
            continue;
        }
        let at = a.map_or(off_php, |a| a.lo as usize);
        let class = match (e, a) {
            (Some(e), Some(a)) if e.id != a.id => {
                format!("id: php {} vs rphp {}", name(e.id), name(a.id))
            }
            (Some(e), Some(a)) if e.len != a.hi - a.lo => format!("len of {}", name(e.id)),
            (Some(e), Some(_)) => format!("line of {}", name(e.id)),
            (Some(e), None) => format!("rphp stops early (php has {})", name(e.id)),
            (None, Some(a)) => format!("rphp has extra {}", name(a.id)),
            (None, None) => unreachable!(),
        };
        let mut r = format!(
            "== {} (short_open_tag={}) — token #{i} at byte {at}: {class}\n",
            path.display(),
            u8::from(opts.short_open_tag)
        );
        let lo = i.saturating_sub(3);
        let hi = (i + 4).min(n);
        r.push_str("     #     php: id line len            | rphp: id line len\n");
        for j in lo..hi {
            let mark = if j == i { ">>" } else { "  " };
            let l = expected.get(j).map_or("-".to_string(), |e| {
                format!("{:<24} L{:<4} {:>5}", name(e.id), e.line, e.len)
            });
            let rr = actual.get(j).map_or("-".to_string(), |a| {
                format!("{:<24} L{:<4} {:>5}", name(a.id), a.line, a.hi - a.lo)
            });
            r.push_str(&format!("  {mark} {j:<5} {l} | {rr}\n"));
        }
        r.push_str(&format!("  source: {}\n", excerpt(src, at)));
        return Some(Mismatch { class, report: r });
    }
    None
}

fn run_corpus(
    php: &Path,
    files: &[PathBuf],
    opts: Options,
    jobs: usize,
) -> (usize, usize, usize, Vec<Mismatch>) {
    let next = AtomicUsize::new(0);
    let batches: Vec<&[PathBuf]> = files.chunks(BATCH).collect();
    let pass = AtomicUsize::new(0);
    let unreadable = AtomicUsize::new(0);
    let mismatches: Mutex<Vec<Mismatch>> = Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for _ in 0..jobs.max(1) {
            s.spawn(|| loop {
                let b = next.fetch_add(1, Ordering::Relaxed);
                let Some(batch) = batches.get(b) else { break };
                for (path, expected) in php_dump(php, batch, opts.short_open_tag) {
                    let Some(expected) = expected else {
                        unreadable.fetch_add(1, Ordering::Relaxed);
                        continue;
                    };
                    let src = std::fs::read(&path).expect("read corpus file");
                    match compare(&path, &src, &expected, opts) {
                        None => {
                            pass.fetch_add(1, Ordering::Relaxed);
                        }
                        Some(m) => mismatches.lock().unwrap().push(m),
                    }
                }
            });
        }
    });
    let mut mm = mismatches.into_inner().unwrap();
    mm.sort_by(|a, b| a.report.cmp(&b.report));
    let fail = mm.len();
    (pass.into_inner(), fail, unreadable.into_inner(), mm)
}

fn summarize(
    label: &str,
    opts: Options,
    pass: usize,
    fail: usize,
    unreadable: usize,
    mm: &[Mismatch],
    secs: f64,
) -> String {
    let mut s = format!(
        "{label} short_open_tag={}: {pass} pass, {fail} fail, {unreadable} unreadable ({:.1} s)\n",
        u8::from(opts.short_open_tag),
        secs
    );
    if fail > 0 {
        let mut hist: BTreeMap<&str, usize> = BTreeMap::new();
        for m in mm {
            *hist.entry(m.class.as_str()).or_default() += 1;
        }
        let mut classes: Vec<_> = hist.into_iter().collect();
        classes.sort_by_key(|c| std::cmp::Reverse(c.1));
        s.push_str("mismatch classes:\n");
        for (c, n) in classes {
            s.push_str(&format!("  {n:>6}  {c}\n"));
        }
        for m in mm.iter().take(SHOW) {
            s.push_str(&m.report);
        }
    }
    s
}

const MODES: [Options; 2] = [
    Options {
        short_open_tag: false,
    },
    Options {
        short_open_tag: true,
    },
];

#[test]
fn mini_corpus_matches_php() {
    let Some(php) = php_binary() else {
        eprintln!("differential: no `php` on PATH (set RPHP_PHP); skipping");
        return;
    };
    let mut files = Vec::new();
    walk(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/corpus"),
        &mut files,
    );
    assert!(
        files.len() >= 40,
        "mini corpus has only {} files",
        files.len()
    );
    let mut failures = String::new();
    for opts in MODES {
        let t0 = std::time::Instant::now();
        let (pass, fail, unreadable, mm) = run_corpus(&php, &files, opts, 1);
        let s = summarize(
            "mini corpus",
            opts,
            pass,
            fail,
            unreadable,
            &mm,
            t0.elapsed().as_secs_f64(),
        );
        eprintln!("{s}");
        assert_eq!(unreadable, 0);
        if fail > 0 {
            failures.push_str(&s);
            for m in mm.iter().skip(SHOW) {
                failures.push_str(&m.report);
            }
        }
    }
    assert!(failures.is_empty(), "\n{failures}");
}

#[test]
fn external_corpus_matches_php() {
    let Ok(dir) = std::env::var("RPHP_TOKENIZER_CORPUS") else {
        eprintln!("differential: RPHP_TOKENIZER_CORPUS not set; skipping the external corpus");
        return;
    };
    let Some(php) = php_binary() else {
        eprintln!("differential: no `php` on PATH (set RPHP_PHP); skipping");
        return;
    };
    let mut files = Vec::new();
    walk(Path::new(&dir), &mut files);
    assert!(!files.is_empty(), "no .php files under {dir}");
    let jobs = std::env::var("RPHP_TOKENIZER_JOBS")
        .ok()
        .and_then(|j| j.parse().ok())
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(4, |n| n.get().min(8)));
    eprintln!(
        "differential: {} files under {dir}, {jobs} jobs",
        files.len()
    );
    let mut total_fail = 0;
    for opts in MODES {
        let t0 = std::time::Instant::now();
        let (pass, fail, unreadable, mm) = run_corpus(&php, &files, opts, jobs);
        eprintln!(
            "{}",
            summarize(
                &dir,
                opts,
                pass,
                fail,
                unreadable,
                &mm,
                t0.elapsed().as_secs_f64()
            )
        );
        total_fail += fail;
    }
    assert_eq!(
        total_fail, 0,
        "{total_fail} files differ from php (see output above)"
    );
}
