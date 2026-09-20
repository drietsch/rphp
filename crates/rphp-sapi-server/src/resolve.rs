//! What a request path names under the document root — php's
//! `normalize_vpath` and `php_cli_server_request_translate_vpath`, ported
//! step for step so `SCRIPT_NAME`, `PATH_INFO` and the not-found cases come
//! out the same.

use std::path::{Path, PathBuf};

/// The decoded, dot-segment-free request path (`normalize_vpath`): raw
/// url-decoded, runs of `/` collapsed, `.` and `..` segments resolved
/// without ever leaving the root.
pub fn normalize_vpath(raw: &str) -> String {
    let decoded = raw_url_decode(raw.as_bytes());
    let mut segments: Vec<&[u8]> = Vec::new();
    let leading = decoded.first() == Some(&b'/');
    for seg in decoded.split(|b| *b == b'/') {
        match seg {
            b"" | b"." => {}
            b".." => {
                segments.pop();
            }
            s => segments.push(s),
        }
    }
    let mut out = Vec::new();
    if leading {
        out.push(b'/');
    }
    for (i, s) in segments.iter().enumerate() {
        if i > 0 {
            out.push(b'/');
        }
        out.extend_from_slice(s);
    }
    // php keeps a trailing slash when the last segment ended with one.
    if decoded.len() > 1 && decoded.ends_with(b"/") && !out.ends_with(b"/") {
        out.push(b'/');
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `%XX` decoding, `+` left alone.
fn raw_url_decode(s: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        if s[i] == b'%' && i + 2 < s.len() {
            let hex = |b: u8| (b as char).to_digit(16);
            if let (Some(h), Some(l)) = (hex(s[i + 1]), hex(s[i + 2])) {
                out.push(((h << 4) | l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(s[i]);
        i += 1;
    }
    out
}

/// Where a request path lands.
#[derive(Debug, PartialEq, Eq)]
pub struct Translated {
    /// The file on disk.
    pub path: PathBuf,
    /// `SCRIPT_NAME`: the part of the request path that named the file
    /// (with `/index.php` appended when a directory was asked for).
    pub script_name: String,
    /// `PATH_INFO`: what followed the file in the request path, if anything.
    pub path_info: Option<String>,
    /// Whether the file's extension is `php` — the only thing php runs.
    pub is_php: bool,
}

/// php's `php_cli_server_request_translate_vpath`: walk the request path
/// back one segment at a time until something exists under `docroot`; a
/// directory is served by its `index.php` or `index.html`, and a directory
/// without either is not found.
pub fn translate(docroot: &Path, vpath: &str) -> Option<Translated> {
    let mut rel = vpath.to_string();
    let mut path_info: Option<String> = None;
    loop {
        let full = join(docroot, &rel);
        match std::fs::metadata(&full) {
            Ok(md) if md.is_dir() => {
                let mut dir_rel = rel.clone();
                if !dir_rel.ends_with('/') {
                    dir_rel.push('/');
                }
                for index in ["index.php", "index.html"] {
                    let candidate = join(docroot, &format!("{dir_rel}{index}"));
                    if std::fs::metadata(&candidate).is_ok_and(|m| m.is_file()) {
                        let script_name = format!("{dir_rel}{index}");
                        return Some(Translated {
                            is_php: is_php(&script_name),
                            path: candidate,
                            script_name,
                            path_info,
                        });
                    }
                }
                return None;
            }
            Ok(_) => {
                return Some(Translated {
                    is_php: is_php(&rel),
                    path: full,
                    script_name: rel,
                    path_info,
                });
            }
            Err(_) => {}
        }
        // Strip the last segment; it and everything already stripped is
        // the PATH_INFO of whatever turns up above.
        let Some(cut) = rel.rfind('/') else {
            return None;
        };
        if cut == 0 && rel.len() == 1 {
            return None;
        }
        rel.truncate(cut);
        path_info = Some(vpath[rel.len()..].to_string());
        if rel.is_empty() {
            // Back at the root itself.
            let full = docroot.to_path_buf();
            if std::fs::metadata(&full).is_ok_and(|m| m.is_dir()) {
                for index in ["index.php", "index.html"] {
                    let candidate = full.join(index);
                    if std::fs::metadata(&candidate).is_ok_and(|m| m.is_file()) {
                        let script_name = format!("/{index}");
                        return Some(Translated {
                            is_php: is_php(&script_name),
                            path: candidate,
                            script_name,
                            path_info,
                        });
                    }
                }
            }
            return None;
        }
    }
}

fn join(docroot: &Path, rel: &str) -> PathBuf {
    let mut p = docroot.as_os_str().to_os_string();
    if !rel.starts_with('/') {
        p.push("/");
    }
    p.push(rel);
    PathBuf::from(p)
}

/// The extension of a request path (`ext` in php's request struct).
pub fn extension(script_name: &str) -> &str {
    let last = script_name.rsplit('/').next().unwrap_or("");
    match last.rfind('.') {
        Some(i) => &last[i + 1..],
        None => "",
    }
}

fn is_php(script_name: &str) -> bool {
    extension(script_name).eq_ignore_ascii_case("php")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes() {
        assert_eq!(normalize_vpath("/"), "/");
        assert_eq!(normalize_vpath("/a/../b"), "/b");
        assert_eq!(normalize_vpath("/../../x"), "/x");
        assert_eq!(normalize_vpath("//a//b/"), "/a/b/");
        assert_eq!(normalize_vpath("/a/./b"), "/a/b");
        assert_eq!(normalize_vpath("/a%20b/%2e%2e/c"), "/c");
        assert_eq!(normalize_vpath("/sub/page.php/extra/path"), "/sub/page.php/extra/path");
    }

    #[test]
    fn translates_like_php() {
        let dir = std::env::temp_dir().join(format!("rphp-resolve-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::create_dir_all(dir.join("dir")).unwrap();
        std::fs::create_dir_all(dir.join("dir3")).unwrap();
        std::fs::write(dir.join("index.php"), "").unwrap();
        std::fs::write(dir.join("sub/page.php"), "").unwrap();
        std::fs::write(dir.join("static.txt"), "").unwrap();
        std::fs::write(dir.join("dir/index.html"), "").unwrap();
        std::fs::write(dir.join("dir3/index.php"), "").unwrap();
        let t = |v: &str| translate(&dir, v).map(|t| (t.script_name, t.path_info, t.is_php));
        assert_eq!(t("/"), Some(("/index.php".into(), None, true)));
        assert_eq!(t("/nonexistent"), Some(("/index.php".into(), Some("/nonexistent".into()), true)));
        assert_eq!(
            t("/nonexistent/deeper/x.php"),
            Some(("/index.php".into(), Some("/nonexistent/deeper/x.php".into()), true))
        );
        assert_eq!(t("/sub/page.php"), Some(("/sub/page.php".into(), None, true)));
        assert_eq!(
            t("/sub/page.php/extra/path"),
            Some(("/sub/page.php".into(), Some("/extra/path".into()), true))
        );
        assert_eq!(t("/index.php/foo"), Some(("/index.php".into(), Some("/foo".into()), true)));
        assert_eq!(t("/static.txt"), Some(("/static.txt".into(), None, false)));
        assert_eq!(t("/dir"), Some(("/dir/index.html".into(), None, false)));
        assert_eq!(t("/dir/"), Some(("/dir/index.html".into(), None, false)));
        assert_eq!(t("/dir3"), Some(("/dir3/index.php".into(), None, true)));
        assert_eq!(t("/sub"), None);
        assert_eq!(t("/sub/"), None);
        assert_eq!(t("/nonexistent/"), Some(("/index.php".into(), Some("/nonexistent/".into()), true)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
