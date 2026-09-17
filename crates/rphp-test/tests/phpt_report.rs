//! Baseline ratchet/gate semantics and report rendering.

use std::path::Path;
use std::time::Duration;

use rphp_test::phpt::report::{load_enabled, summary_table};
use rphp_test::phpt::{Baseline, Diff, Outcome, Ratchet, SliceReport, TestResult};

#[test]
fn baseline_roundtrip_ratchet_and_gate() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ctype.toml");
    std::fs::write(
        &path,
        "# comment\ndir = \"ext/ctype/tests\"\nenabled = true\nbaseline = 10\nnotes = \"n\"\n",
    )
    .unwrap();
    let mut b = Baseline::load(&path).unwrap();
    assert_eq!(b.dir, "ext/ctype/tests");
    assert_eq!(b.baseline, 10);
    assert!(b.enabled);
    assert_eq!(b.total, None);

    assert!(b.gate("ctype", 10).is_ok());
    assert!(b.gate("ctype", 11).is_ok());
    let err = b.gate("ctype", 9).unwrap_err();
    assert!(err.contains("below the baseline of 10"), "{err}");

    assert_eq!(b.ratchet(10, 49), Ratchet::Unchanged);
    assert_eq!(
        b.ratchet(7, 49),
        Ratchet::Refused {
            baseline: 10,
            actual: 7
        }
    );
    assert_eq!(b.baseline, 10, "refusal leaves the baseline alone");
    assert_eq!(b.ratchet(12, 49), Ratchet::Raised { from: 10, to: 12 });
    assert_eq!(b.total, Some(49));
    assert!(b.updated.as_deref().is_some_and(|d| d.len() == 10));

    b.save(&path).unwrap();
    let again = Baseline::load(&path).unwrap();
    assert_eq!(again, b);

    b.enabled = false;
    assert!(b.gate("ctype", 0).is_ok(), "disabled slices never gate");

    let all = load_enabled(dir.path()).unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].0, "ctype");
}

#[test]
fn slice_report_counts_and_renders() {
    let root = Path::new("/corpus");
    let mk = |name: &str, outcome: Outcome| TestResult {
        path: root.join("ext/x/tests").join(name),
        name: format!("test {name}"),
        outcome,
        duration_ms: 1,
        notes: vec![],
        cached: false,
    };
    let diff = Diff {
        line: 3,
        expected: "int(1)".into(),
        actual: "int(2)".into(),
        text: "003- int(1)\n003+ int(2)".into(),
    };
    let results = vec![
        mk("a.phpt", Outcome::Pass),
        mk("b.phpt", Outcome::Fail { diff: diff.clone() }),
        mk(
            "c.phpt",
            Outcome::Skip {
                reason: "no locale".into(),
            },
        ),
        mk(
            "d.phpt",
            Outcome::XFail {
                reason: "bug".into(),
                diff,
            },
        ),
        mk(
            "e.phpt",
            Outcome::Borked {
                reason: "missing section --FILE--".into(),
            },
        ),
        mk("f.phpt", Outcome::Timeout),
        mk(
            "g.phpt",
            Outcome::Crash {
                signal: Some(11),
                exit: None,
            },
        ),
        mk("h.phpt", Outcome::XPass),
    ];
    let r = SliceReport::from_results("x", &root.join("ext/x/tests"), "php", &results, root, 1234);
    assert_eq!(r.counts.total, 8);
    assert_eq!(r.counts.pass, 1);
    assert_eq!(r.counts.fail, 1);
    assert_eq!(r.counts.skip, 1);
    assert_eq!(r.counts.xfail, 1);
    assert_eq!(r.counts.bork, 1);
    assert_eq!(r.counts.timeout, 1);
    assert_eq!(r.counts.crash, 1);
    assert_eq!(r.counts.xpass, 1);
    assert_eq!(r.counts.ran(), 7);
    assert_eq!(r.failures.len(), 5, "FAIL, BORK, TIMEOUT, CRASH, XPASS");
    assert_eq!(r.xfails.len(), 1);
    assert_eq!(r.skips.len(), 1);
    assert_eq!(r.failures[0].path, "ext/x/tests/b.phpt");
    assert_eq!(r.failures[0].first_line, Some(3));
    assert!(r.failures[0].summary.contains("line 3"));

    let md = r.to_markdown();
    assert!(md.contains("| 8 | 7 | 1 |"), "{md}");
    assert!(md.contains("`ext/x/tests/g.phpt`"), "{md}");
    assert!(md.contains("killed by signal 11"), "{md}");
    let json: serde_json::Value = serde_json::from_str(&r.to_json()).unwrap();
    assert_eq!(json["counts"]["crash"], 1);
    let text = r.to_text(true);
    assert!(text.starts_with("x "), "{text}");
    assert!(text.contains("TIMEOUT ext/x/tests/f.phpt"), "{text}");
    let table = summary_table(&[r.clone(), r]);
    assert!(
        table.lines().last().unwrap().starts_with("TOTAL"),
        "{table}"
    );
    let _ = Duration::from_secs(0);
}
