//! `cargo xtask phpt` — run php-src `.phpt` slices under rphp (or under stock
//! php to validate the runner itself), report, and gate on the per-slice
//! baselines in `tests/phpt/enabled/*.toml`.
//!
//! ```text
//! cargo xtask phpt --ext ctype --ext json            # scored slices
//! cargo xtask phpt --dir vendor-php-src/ext/spl/tests --filter 'array_*'
//! cargo xtask phpt --engine php --ext ctype          # runner self-check
//! cargo xtask phpt --gate                            # every enabled slice vs its baseline
//! cargo xtask phpt --ext json --update-baseline      # ratchet up (never down)
//! ```

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use rphp_test::phpt::{
    self, Baseline, Cache, Engine, Outcome, Ratchet, RunOptions, Runner, SliceReport, TestResult,
};

use crate::XtaskResult;

const USAGE: &str = "\
usage: cargo xtask phpt [options]

Selection (default: every enabled slice in tests/phpt/enabled/):
  --ext <name>          slice to run (repeatable): an extension (`ctype` -> ext/ctype/tests),
                        a sub-directory (`standard-strings` -> ext/standard/tests/strings),
                        or one of zend (Zend/tests), lang (tests/lang), basic (tests/basic),
                        cli (sapi/cli/tests); a matching tests/phpt/enabled/<name>.toml wins
  --dir <path>          an arbitrary directory of .phpt files (repeatable)
  --filter <pat>        glob (`array_*`) or substring on the test path
  --corpus <path>       php-src checkout (default: $PHP_SRC_DIR or <repo>/vendor-php-src)

Engine:
  --engine rphp|php     default rphp
  --rphp <path>         rphp binary (default: $RPHP_BIN or target/debug/rphp)
  --php <path>          php binary for --engine php (default: `php` on PATH)
  --jobs <n>            worker threads (default: all cores)
  --timeout <s>         per-test timeout (default 60)
  --no-cache            ignore and do not write target/phpt-cache
  --keep                keep <test>.php beside failing tests

Reporting / gating:
  --report text|md|json print this format to stdout (default text); md and json are
                        always written to target/phpt-report/<slice>.{md,json}
  --list-failures       print every non-passing test with its first differing line
  --verbose             print every result as it finishes
  --gate                exit 1 if an enabled slice passes fewer tests than its baseline
  --update-baseline     raise baselines to the observed pass counts (never lowers)
";

/// A directory to score.
struct Slice {
    name: String,
    dir: PathBuf,
    baseline_path: PathBuf,
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Map an `--ext` name to a directory below the corpus.
fn map_ext(name: &str) -> String {
    match name {
        "zend" => "Zend/tests".to_string(),
        "lang" => "tests/lang".to_string(),
        "basic" => "tests/basic".to_string(),
        "cli" => "sapi/cli/tests".to_string(),
        "tests" => "tests".to_string(),
        other => match other.split_once('-') {
            Some((ext, sub)) => format!("ext/{ext}/tests/{}", sub.replace('-', "/")),
            None => format!("ext/{other}/tests"),
        },
    }
}

fn corpus_missing(corpus: &Path) -> ! {
    eprintln!(
        "xtask phpt: php-src corpus not found at {}\n\
         fetch it with:\n\
         \n    cargo xtask fetch-php-src\n\
         \n(or tools/php-src/checkout.sh; set --corpus/PHP_SRC_DIR for another location)",
        corpus.display()
    );
    std::process::exit(2);
}

/// Entry point.
pub fn run(args: &[String]) -> XtaskResult {
    let mut pargs = pico_args::Arguments::from_vec(args.iter().map(Into::into).collect());
    if pargs.contains(["-h", "--help"]) {
        print!("{USAGE}");
        return Ok(());
    }
    let exts: Vec<String> = pargs.values_from_str("--ext")?;
    let dirs: Vec<PathBuf> = pargs.values_from_str("--dir")?;
    let filter: String = pargs.opt_value_from_str("--filter")?.unwrap_or_default();
    let corpus: PathBuf = pargs
        .opt_value_from_str("--corpus")?
        .or_else(|| std::env::var_os("PHP_SRC_DIR").map(PathBuf::from))
        .unwrap_or_else(|| repo_root().join("vendor-php-src"));
    let engine_name: String = pargs
        .opt_value_from_str("--engine")?
        .unwrap_or_else(|| "rphp".to_string());
    let rphp_bin: Option<PathBuf> = pargs.opt_value_from_str("--rphp")?;
    let php_bin: Option<PathBuf> = pargs.opt_value_from_str("--php")?;
    let jobs: Option<usize> = pargs.opt_value_from_str("--jobs")?;
    let timeout: u64 = pargs.opt_value_from_str("--timeout")?.unwrap_or(60);
    let no_cache = pargs.contains("--no-cache");
    let keep = pargs.contains("--keep");
    let report_fmt: String = pargs
        .opt_value_from_str("--report")?
        .unwrap_or_else(|| "text".to_string());
    let list_failures = pargs.contains("--list-failures");
    let verbose = pargs.contains("--verbose");
    let gate = pargs.contains("--gate");
    let update_baseline = pargs.contains("--update-baseline");
    let rest = pargs.finish();
    if !rest.is_empty() {
        eprint!("{USAGE}");
        return Err(format!("unexpected arguments: {rest:?}").into());
    }
    if !matches!(report_fmt.as_str(), "text" | "md" | "json") {
        return Err(format!("--report must be text, md or json (got {report_fmt})").into());
    }

    // Engine.
    let engine = match engine_name.as_str() {
        "rphp" => {
            let bin = rphp_bin.unwrap_or_else(Engine::default_rphp_binary);
            if !bin.is_file() {
                return Err(format!(
                    "rphp binary not found at {} — build it with `cargo build -p rphp` or pass --rphp/RPHP_BIN",
                    bin.display()
                )
                .into());
            }
            Engine::rphp(bin)
        }
        "php" => {
            let bin = match php_bin {
                Some(p) => p,
                None => Engine::find_in_path("php")
                    .ok_or("`php` not found on PATH; pass --php <path>")?,
            };
            Engine::php(bin)
        }
        other => return Err(format!("--engine must be rphp or php (got {other})").into()),
    };

    // Slices.
    let enabled_dir = repo_root().join("tests/phpt/enabled");
    let mut slices: Vec<Slice> = Vec::new();
    let need_corpus = !exts.is_empty() || dirs.is_empty();
    if need_corpus && !corpus.join("run-tests.php").is_file() {
        corpus_missing(&corpus);
    }
    for name in &exts {
        let baseline_path = enabled_dir.join(format!("{name}.toml"));
        let rel = match Baseline::load(&baseline_path) {
            Ok(b) => b.dir,
            Err(_) => map_ext(name),
        };
        slices.push(Slice {
            name: name.clone(),
            dir: corpus.join(rel),
            baseline_path,
        });
    }
    for d in &dirs {
        let name = d
            .display()
            .to_string()
            .trim_end_matches('/')
            .replace(['/', '\\'], "-")
            .trim_start_matches('-')
            .to_string();
        slices.push(Slice {
            baseline_path: enabled_dir.join(format!("{name}.toml")),
            name,
            dir: d.clone(),
        });
    }
    if slices.is_empty() {
        for (name, b) in phpt::report::load_enabled(&enabled_dir)? {
            if !b.enabled {
                continue;
            }
            slices.push(Slice {
                name: name.clone(),
                dir: corpus.join(&b.dir),
                baseline_path: enabled_dir.join(format!("{name}.toml")),
            });
        }
        if slices.is_empty() {
            return Err(format!("no slices enabled in {}", enabled_dir.display()).into());
        }
    }
    for s in &slices {
        if !s.dir.is_dir() {
            return Err(format!("{}: directory not found: {}", s.name, s.dir.display()).into());
        }
    }

    // Runner.
    let cache = if no_cache {
        None
    } else {
        Some(Cache::new(Cache::default_root())?)
    };
    let options = RunOptions {
        timeout: Duration::from_secs(timeout),
        cwd: Some(if corpus.is_dir() {
            corpus.clone()
        } else {
            std::env::current_dir()?
        }),
        keep_files: keep,
        retry_flaky: true,
    };
    let runner = Runner {
        engine: &engine,
        options,
        cache,
    };
    let pool = {
        let mut b = rayon::ThreadPoolBuilder::new();
        if let Some(j) = jobs {
            b = b.num_threads(j.max(1));
        }
        b.build()?
    };

    let report_dir = repo_root().join("target/phpt-report");
    std::fs::create_dir_all(&report_dir)?;
    eprintln!("engine: {}", engine.label());

    let mut reports: Vec<SliceReport> = Vec::new();
    let mut gate_errors: Vec<String> = Vec::new();
    for slice in &slices {
        let all = phpt::discover(&slice.dir);
        let tests = phpt::filter_tests(all, &corpus, &filter);
        eprintln!(
            "{}: {} tests in {}",
            slice.name,
            tests.len(),
            slice.dir.display()
        );
        let started = Instant::now();
        let done = AtomicUsize::new(0);
        let stderr_lock = Mutex::new(());
        let total = tests.len();
        let results: Vec<TestResult> = pool.install(|| {
            runner.run_all(&tests, |r| {
                let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                if verbose {
                    let _g = stderr_lock.lock();
                    let rel = r.path.strip_prefix(&corpus).unwrap_or(&r.path);
                    let extra = match &r.outcome {
                        Outcome::Skip { reason } => format!(" ({reason})"),
                        Outcome::Fail { diff } => format!(" (line {})", diff.line),
                        Outcome::Borked { reason } => format!(" ({reason})"),
                        _ => String::new(),
                    };
                    eprintln!(
                        "[{n}/{total}] {:<7} {}{extra}{}",
                        r.outcome.label(),
                        rel.display(),
                        if r.cached { " [cached]" } else { "" }
                    );
                }
            })
        });
        let report = SliceReport::from_results(
            &slice.name,
            &slice.dir,
            &engine.label(),
            &results,
            &corpus,
            started.elapsed().as_millis() as u64,
        );
        std::fs::write(
            report_dir.join(format!("{}.md", slice.name)),
            report.to_markdown(),
        )?;
        std::fs::write(
            report_dir.join(format!("{}.json", slice.name)),
            report.to_json(),
        )?;

        // Baselines.
        let pass = report.counts.pass;
        if update_baseline {
            let mut b = Baseline::load(&slice.baseline_path).unwrap_or_else(|_| Baseline {
                dir: slice
                    .dir
                    .strip_prefix(&corpus)
                    .unwrap_or(&slice.dir)
                    .display()
                    .to_string(),
                enabled: true,
                baseline: 0,
                total: None,
                updated: None,
                notes: String::new(),
            });
            match b.ratchet(pass, report.counts.total) {
                Ratchet::Raised { from, to } => {
                    b.save(&slice.baseline_path)?;
                    eprintln!(
                        "{}: baseline {from} -> {to} ({})",
                        slice.name,
                        slice.baseline_path.display()
                    );
                }
                Ratchet::Unchanged => {
                    if !slice.baseline_path.is_file() {
                        b.save(&slice.baseline_path)?;
                    }
                    eprintln!("{}: baseline unchanged at {pass}", slice.name);
                }
                Ratchet::Refused { baseline, actual } => {
                    eprintln!(
                        "{}: refusing to lower the baseline from {baseline} to {actual}",
                        slice.name
                    );
                    gate_errors.push(format!(
                        "{}: {actual} passing tests is below the baseline of {baseline}",
                        slice.name
                    ));
                }
            }
        } else if gate {
            if let Ok(b) = Baseline::load(&slice.baseline_path) {
                if let Err(e) = b.gate(&slice.name, pass) {
                    gate_errors.push(e);
                }
            }
        }

        match report_fmt.as_str() {
            "text" => print!("{}", report.to_text(list_failures)),
            "md" => println!("{}", report.to_markdown()),
            _ => println!("{}", report.to_json()),
        }
        reports.push(report);
    }

    if report_fmt == "text" && reports.len() > 1 {
        println!(
            "{}",
            phpt::report::summary_table(&reports)
                .lines()
                .last()
                .unwrap_or("")
        );
    }
    if report_fmt == "text" {
        eprintln!("reports: {}/<slice>.{{md,json}}", report_dir.display());
    }
    if !gate_errors.is_empty() {
        return Err(format!("baseline gate failed:\n  {}", gate_errors.join("\n  ")).into());
    }
    Ok(())
}
