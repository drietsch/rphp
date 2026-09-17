//! A result cache so reruns only execute tests whose inputs changed.
//!
//! The key covers the test file, the support files in its directory
//! (`.inc`, external FILE/EXPECT files, …), the engine fingerprint (binary
//! contents + mtime, extra args, INI overwrites), the timeout and the working
//! directory. Files included from *other* directories are not tracked — use
//! `--no-cache` after touching those.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::run::{hex, TestResult};

/// On-disk cache rooted at a directory (`target/phpt-cache` by default).
#[derive(Debug, Clone)]
pub struct Cache {
    root: PathBuf,
}

impl Cache {
    /// Open (creating) a cache at `root`.
    pub fn new(root: impl Into<PathBuf>) -> io::Result<Self> {
        let root = root.into();
        fs::create_dir_all(&root)?;
        Ok(Cache { root })
    }

    /// `<workspace>/target/phpt-cache`.
    pub fn default_root() -> PathBuf {
        super::workspace_root().join("target").join("phpt-cache")
    }

    /// The cache directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Hash the given parts (NUL-separated) into a cache key.
    pub fn key(parts: &[&[u8]]) -> String {
        let mut h = Sha256::new();
        for p in parts {
            h.update(p);
            h.update(b"\0");
        }
        hex(&h.finalize())
    }

    fn path_for(&self, key: &str) -> PathBuf {
        self.root.join(&key[..2]).join(format!("{key}.json"))
    }

    /// A cached result for `key`, marked `cached = true`.
    pub fn get(&self, key: &str) -> Option<TestResult> {
        let bytes = fs::read(self.path_for(key)).ok()?;
        let mut r: TestResult = serde_json::from_slice(&bytes).ok()?;
        r.cached = true;
        r.duration_ms = 0;
        Some(r)
    }

    /// Store a result (atomically: write a temp file, then rename).
    pub fn put(&self, key: &str, result: &TestResult) -> io::Result<()> {
        let path = self.path_for(key);
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension(format!("tmp{}", std::process::id()));
        fs::write(&tmp, serde_json::to_vec(result)?)?;
        fs::rename(&tmp, &path)
    }

    /// Delete everything.
    pub fn clear(&self) -> io::Result<()> {
        if self.root.exists() {
            fs::remove_dir_all(&self.root)?;
        }
        fs::create_dir_all(&self.root)
    }
}
