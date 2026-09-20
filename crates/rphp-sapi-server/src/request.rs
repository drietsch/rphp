//! Binding one HTTP request to an interpreter: `$_SERVER` exactly as
//! `php -S` lays it out, `$_GET`/`$_POST`/`$_COOKIE`/`$_FILES`/`$_REQUEST`
//! by php's request-variable grammar, the raw body for `php://input`, the
//! header list for `getallheaders()`, and the uploads of a
//! `multipart/form-data` body (RFC 1867) as temporary files.

use std::path::{Path, PathBuf};

use rphp_runtime::Interp;
use rphp_stdlib::{parse_query, register_variable};
use rphp_value::{Array, ArrayKey, PhpRef, Value};

use crate::http::Request;

/// What the dispatcher decided about the request, for `$_SERVER`.
pub struct Binding<'a> {
    pub request: &'a Request,
    pub docroot: &'a Path,
    /// `SERVER_NAME` / `SERVER_PORT`: the address the server listens on.
    pub host: &'a str,
    pub port: u16,
    /// The normalized request path (`SCRIPT_NAME` when no file was found,
    /// as under a router).
    pub vpath: &'a str,
    /// The file the request named, if any.
    pub script_name: Option<&'a str>,
    pub script_filename: Option<&'a Path>,
    pub path_info: Option<&'a str>,
    /// The router script, which stands in for `SCRIPT_FILENAME` when there
    /// is no file.
    pub router: Option<&'a Path>,
    pub request_time: f64,
}

/// Seed the interpreter's superglobals and request state.
pub fn bind(it: &mut Interp, b: &Binding<'_>) {
    let req = b.request;
    it.request_time = Some(b.request_time);
    it.request_headers = req.headers.clone();
    // php's multipart handler consumes the body: `php://input` is empty for
    // a `multipart/form-data` POST.
    let content_type = req.header("Content-Type").unwrap_or("");
    let mime = content_type.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    let consumed = req.method == "POST" && mime == "multipart/form-data";
    it.request_body = Some(std::sync::Arc::from(if consumed { &[][..] } else { req.body.as_slice() }));

    // $_GET
    let max_vars = it.ini.int("max_input_vars");
    let get = match &req.query {
        Some(q) => parse_query(q.as_bytes(), b"&", max_vars).0,
        None => Array::new(),
    };
    // $_POST and $_FILES: only a POST body is read into them.
    let (post, files) = if req.method == "POST" {
        if mime == "application/x-www-form-urlencoded" {
            (parse_query(&req.body, b"&", max_vars).0, Array::new())
        } else if mime == "multipart/form-data" {
            let boundary = boundary_of(content_type);
            match boundary {
                Some(bnd) => {
                    let mut post = Array::new();
                    let mut files = Array::new();
                    let tmp_dir = upload_dir(it);
                    for part in crate::multipart::parse(&req.body, bnd.as_bytes()) {
                        match part.filename {
                            None => register_variable(&mut post, &part.name, Value::string(&part.body)),
                            Some(filename) => {
                                let (tmp, error) = match write_upload(&tmp_dir, &part.body) {
                                    Some(p) => (p, 0),
                                    None => (PathBuf::new(), 3),
                                };
                                if !tmp.as_os_str().is_empty() {
                                    it.uploaded_files.push(tmp.clone());
                                }
                                register_file(
                                    &mut files,
                                    &part.name,
                                    &filename,
                                    part.content_type.as_deref().unwrap_or(""),
                                    &tmp,
                                    error,
                                    part.body.len(),
                                );
                            }
                        }
                    }
                    (post, files)
                }
                None => (Array::new(), Array::new()),
            }
        } else {
            (Array::new(), Array::new())
        }
    } else {
        (Array::new(), Array::new())
    };
    // $_COOKIE: `;`-separated; the name is taken as sent, the value is
    // raw-url-decoded (`+` stays), and a name already registered (looked up
    // as sent, brackets and all) keeps its first value.
    let mut cookie = Array::new();
    if let Some(raw) = req.header("Cookie") {
        for pair in raw.split(';') {
            let pair = pair.trim_start();
            if pair.is_empty() {
                continue;
            }
            let (name, value) = match pair.split_once('=') {
                Some((n, v)) => (n, v),
                None => (pair, ""),
            };
            if cookie.get(&ArrayKey::str(name.as_bytes())).is_some() {
                continue;
            }
            let value = raw_url_decode(value.as_bytes());
            register_variable(&mut cookie, name.as_bytes(), Value::string(&value));
        }
    }
    // $_REQUEST per `request_order`, or `variables_order` when that is
    // empty; later sources override earlier ones.
    let order = it
        .ini
        .get("request_order")
        .filter(|s| !s.is_empty())
        .or_else(|| it.ini.get("variables_order"))
        .unwrap_or("EGPCS")
        .to_string();
    let mut request = Array::new();
    for c in order.chars() {
        let src = match c.to_ascii_uppercase() {
            'G' => &get,
            'P' => &post,
            'C' => &cookie,
            _ => continue,
        };
        for (k, v) in src.iter() {
            request.set(k.clone(), v.clone());
        }
    }

    let server = server_array(b);

    it.globals.insert(b"_GET", PhpRef::new(Value::Array(get)));
    it.globals.insert(b"_POST", PhpRef::new(Value::Array(post)));
    it.globals.insert(b"_COOKIE", PhpRef::new(Value::Array(cookie)));
    it.globals.insert(b"_FILES", PhpRef::new(Value::Array(files)));
    it.globals.insert(b"_REQUEST", PhpRef::new(Value::Array(request)));
    it.globals.insert(b"_SERVER", PhpRef::new(Value::Array(server)));
}

/// `$_SERVER` in php's order: the environment, then the SAPI's keys, then
/// the request times.
fn server_array(b: &Binding<'_>) -> Array {
    let req = b.request;
    let mut s = Array::new();
    let mut set = |k: &str, v: &str| {
        s.set(ArrayKey::str(k.as_bytes()), Value::string(v.as_bytes()));
    };
    for (k, v) in std::env::vars_os() {
        set(&k.to_string_lossy(), &v.to_string_lossy());
    }
    set("DOCUMENT_ROOT", &b.docroot.to_string_lossy());
    set("REMOTE_ADDR", &req.remote.ip().to_string());
    set("REMOTE_PORT", &req.remote.port().to_string());
    set("SERVER_SOFTWARE", &format!("PHP/{} (Development Server)", crate::PHP_VERSION));
    set("SERVER_PROTOCOL", &req.protocol());
    set("SERVER_NAME", b.host);
    set("SERVER_PORT", &b.port.to_string());
    set("REQUEST_URI", &req.uri);
    set("REQUEST_METHOD", &req.method);
    let script_name = b.script_name.unwrap_or(b.vpath);
    set("SCRIPT_NAME", script_name);
    if let Some(f) = b.script_filename {
        set("SCRIPT_FILENAME", &f.to_string_lossy());
    } else if let Some(r) = b.router {
        set("SCRIPT_FILENAME", &r.to_string_lossy());
    }
    if let Some(pi) = b.path_info {
        set("PATH_INFO", pi);
        set("PHP_SELF", &format!("{script_name}{pi}"));
    } else {
        set("PHP_SELF", script_name);
    }
    if let Some(q) = &req.query {
        set("QUERY_STRING", q);
    }
    for (name, value) in &req.headers {
        let key: String = name
            .chars()
            .map(|c| if c == '-' { '_' } else { c.to_ascii_uppercase() })
            .collect();
        if key == "CONTENT_TYPE" || key == "CONTENT_LENGTH" {
            set(&key, value);
        }
        set(&format!("HTTP_{key}"), value);
    }
    if let Some(auth) = req.header("Authorization") {
        if let Some(rest) = strip_prefix_ci(auth, "basic ") {
            if let Some(decoded) = base64_decode(rest.trim()) {
                let text = String::from_utf8_lossy(&decoded).into_owned();
                let (user, pw) = text.split_once(':').unwrap_or((&text, ""));
                set("PHP_AUTH_USER", user);
                set("PHP_AUTH_PW", pw);
            }
        } else if let Some(rest) = strip_prefix_ci(auth, "digest ") {
            set("PHP_AUTH_DIGEST", rest);
        }
    }
    s.set(ArrayKey::str(b"REQUEST_TIME_FLOAT"), Value::Float(b.request_time));
    s.set(ArrayKey::str(b"REQUEST_TIME"), Value::Int(b.request_time as i64));
    s
}

fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    if s.len() >= prefix.len() && s[..prefix.len()].eq_ignore_ascii_case(prefix) {
        Some(&s[prefix.len()..])
    } else {
        None
    }
}

/// Standard base64, lenient about padding.
fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut acc: u32 = 0;
    let mut bits = 0;
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            b' ' | b'\r' | b'\n' | b'\t' => continue,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 0xff) as u8);
        }
    }
    Some(out)
}

/// `%XX` decoding, `+` left alone (php's `php_raw_url_decode`).
fn raw_url_decode(s: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        match s[i] {
            b'%' if i + 2 < s.len() => {
                let hex = |b: u8| (b as char).to_digit(16);
                if let (Some(h), Some(l)) = (hex(s[i + 1]), hex(s[i + 2])) {
                    out.push(((h << 4) | l) as u8);
                    i += 3;
                    continue;
                }
                out.push(b'%');
            }
            b => out.push(b),
        }
        i += 1;
    }
    out
}

/// The `boundary=` parameter of a multipart content type.
fn boundary_of(content_type: &str) -> Option<String> {
    for param in content_type.split(';').skip(1) {
        let param = param.trim();
        if let Some(v) = strip_prefix_ci(param, "boundary=") {
            let v = v.trim().trim_matches('"');
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

/// Where uploads land: `upload_tmp_dir`, else the system temporary
/// directory.
fn upload_dir(it: &Interp) -> PathBuf {
    let dir = match it.ini.get("upload_tmp_dir").filter(|d| !d.is_empty()) {
        Some(d) => PathBuf::from(d),
        None => std::env::temp_dir(),
    };
    // php names the file by the directory's real path.
    dir.canonicalize().unwrap_or(dir)
}

/// Write an upload to a fresh `php……` file in `dir`, as php's
/// `php_open_temporary_file` names them.
fn write_upload(dir: &Path, body: &[u8]) -> Option<PathBuf> {
    for attempt in 0..64 {
        let suffix = random_suffix(attempt);
        let path = dir.join(format!("php{suffix}"));
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut f) => {
                use std::io::Write;
                if f.write_all(body).is_err() {
                    let _ = std::fs::remove_file(&path);
                    return None;
                }
                return Some(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return None,
        }
    }
    None
}

/// Six characters that differ per call.
fn random_suffix(salt: u32) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    std::time::SystemTime::now().hash(&mut h);
    std::process::id().hash(&mut h);
    salt.hash(&mut h);
    std::thread::current().id().hash(&mut h);
    let mut n = h.finish();
    let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut s = String::new();
    for _ in 0..6 {
        s.push(alphabet[(n % alphabet.len() as u64) as usize] as char);
        n /= alphabet.len() as u64;
    }
    s
}

/// One upload into `$_FILES`: `name`, `full_path`, `type`, `tmp_name`,
/// `error`, `size`, each placed under the field's name so `f[]` and
/// `f[k]` build the per-attribute arrays php does.
fn register_file(
    files: &mut Array,
    field: &[u8],
    filename: &str,
    content_type: &str,
    tmp: &Path,
    error: i64,
    size: usize,
) {
    // php strips any directory part off the client's file name.
    let base = filename.rsplit(['/', '\\']).next().unwrap_or(filename);
    let entries: [(&str, Value); 6] = [
        ("name", Value::string(base.as_bytes())),
        ("full_path", Value::string(filename.as_bytes())),
        ("type", Value::string(content_type.as_bytes())),
        ("tmp_name", Value::string(tmp.to_string_lossy().as_bytes())),
        ("error", Value::Int(error)),
        ("size", Value::Int(size as i64)),
    ];
    // `field` is `base` or `base[...]`; each attribute goes under
    // `base[attr]` or `base[attr][...]`.
    let (base_name, rest) = match field.iter().position(|b| *b == b'[') {
        Some(i) => (&field[..i], &field[i..]),
        None => (field, &b""[..]),
    };
    for (attr, value) in entries {
        let mut name = base_name.to_vec();
        name.push(b'[');
        name.extend_from_slice(attr.as_bytes());
        name.push(b']');
        name.extend_from_slice(rest);
        register_variable(files, &name, value);
    }
}
