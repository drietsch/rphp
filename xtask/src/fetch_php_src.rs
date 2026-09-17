//! `cargo xtask fetch-php-src` — sparse, shallow checkout of the php-src test
//! corpus at the tag pinned in `tools/php-src/PIN` (the Rust twin of
//! `tools/php-src/checkout.sh`).
//!
//! Only `ext/*/tests`, `Zend/tests`, `tests`, `sapi/cli/tests` and
//! `run-tests.php` are materialised. Re-running is a no-op when the checkout
//! already sits on the pin, otherwise it fast-forwards to it.
//!
//! ```text
//! cargo xtask fetch-php-src [--dir <path>] [--url <git url>] [--pin <tag>]
//! ```

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::XtaskResult;

const USAGE: &str = "\
usage: cargo xtask fetch-php-src [--dir <path>] [--url <git url>] [--pin <tag>]

  --dir <path>   checkout directory (default: <repo>/vendor-php-src, or $PHP_SRC_DIR)
  --url <url>    git remote (default: https://github.com/php/php-src)
  --pin <tag>    override the tag in tools/php-src/PIN
";

/// Sparse-checkout patterns (non-cone mode, gitignore syntax).
pub const SPARSE_PATHS: &[&str] = &[
    "/ext/*/tests/",
    "/Zend/tests/",
    "/tests/",
    "/sapi/cli/tests/",
    "/run-tests.php",
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// The pinned tag from `tools/php-src/PIN`.
pub fn pinned_tag() -> Result<String, String> {
    let path = repo_root().join("tools/php-src/PIN");
    std::fs::read_to_string(&path)
        .map(|s| s.trim().to_string())
        .map_err(|e| format!("cannot read {}: {e}", path.display()))
}

/// The default checkout directory: `$PHP_SRC_DIR` or `<repo>/vendor-php-src`.
pub fn default_dir() -> PathBuf {
    std::env::var_os("PHP_SRC_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| repo_root().join("vendor-php-src"))
}

fn git(dir: Option<&Path>, args: &[&str]) -> Result<String, String> {
    let mut cmd = Command::new("git");
    if let Some(d) = dir {
        cmd.arg("-C").arg(d);
    }
    cmd.args(args);
    let out = cmd.output().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            "git is not installed or not on PATH; install git (https://git-scm.com) and re-run"
                .to_string()
        } else {
            format!("cannot run git: {e}")
        }
    })?;
    if !out.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Ensure `dir` holds the sparse php-src checkout at `pin`. Returns a
/// human-readable status line.
pub fn ensure_checkout(dir: &Path, url: &str, pin: &str) -> Result<String, String> {
    // A clear message before anything else when git is missing.
    git(None, &["--version"])?;

    if dir.join(".git").exists() {
        let current =
            git(Some(dir), &["describe", "--tags", "--exact-match", "HEAD"]).unwrap_or_default();
        if current == pin {
            return Ok(format!("php-src already at {pin} in {}", dir.display()));
        }
        eprintln!(
            "php-src: moving {} from '{}' to {pin}",
            dir.display(),
            if current.is_empty() { "?" } else { &current }
        );
        git(
            Some(dir),
            &[
                "fetch",
                "--depth",
                "1",
                "--filter=blob:none",
                "origin",
                &format!("refs/tags/{pin}:refs/tags/{pin}"),
            ],
        )?;
        let mut args = vec!["sparse-checkout", "set", "--no-cone"];
        args.extend(SPARSE_PATHS);
        git(Some(dir), &args)?;
        git(Some(dir), &["checkout", "--quiet", "--force", pin])?;
        return Ok(format!("php-src now at {pin} in {}", dir.display()));
    }

    eprintln!(
        "php-src: sparse shallow clone of {url} @ {pin} into {}",
        dir.display()
    );
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    git(
        None,
        &[
            "clone",
            "--quiet",
            "--filter=blob:none",
            "--no-checkout",
            "--depth",
            "1",
            "--branch",
            pin,
            url,
            &dir.display().to_string(),
        ],
    )?;
    let mut args = vec!["sparse-checkout", "set", "--no-cone"];
    args.extend(SPARSE_PATHS);
    git(Some(dir), &args)?;
    git(Some(dir), &["checkout", "--quiet", pin])?;
    Ok(format!("php-src now at {pin} in {}", dir.display()))
}

/// Entry point.
pub fn run(args: &[String]) -> XtaskResult {
    let mut pargs = pico_args::Arguments::from_vec(args.iter().map(Into::into).collect());
    if pargs.contains(["-h", "--help"]) {
        print!("{USAGE}");
        return Ok(());
    }
    let dir: PathBuf = pargs
        .opt_value_from_str("--dir")?
        .unwrap_or_else(default_dir);
    let url: String = pargs
        .opt_value_from_str("--url")?
        .unwrap_or_else(|| "https://github.com/php/php-src".to_string());
    let pin: String = match pargs.opt_value_from_str("--pin")? {
        Some(p) => p,
        None => pinned_tag()?,
    };
    let rest = pargs.finish();
    if !rest.is_empty() {
        eprint!("{USAGE}");
        return Err(format!("unexpected arguments: {rest:?}").into());
    }
    let msg = ensure_checkout(&dir, &url, &pin)?;
    println!("{msg}");
    Ok(())
}
