//! Binding one HTTP request to an interpreter: `$_SERVER` exactly as
//! `php -S` lays it out; the superglobals themselves come from the
//! embed crate's CGI binding, shared with the FastCGI SAPI.

use std::path::Path;

use rphp_embed::cgi::{self, CgiRequest};
use rphp_runtime::Interp;
use rphp_value::{Array, ArrayKey, Value};

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
    cgi::bind(
        it,
        CgiRequest {
            method: &req.method,
            query: req.query.as_deref(),
            content_type: req.header("Content-Type"),
            cookie: req.header("Cookie"),
            headers: req.headers.clone(),
            body: &req.body,
            server: server_array(b),
            request_time: b.request_time,
        },
    );
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
        let key = cgi::header_key(name);
        if key == "CONTENT_TYPE" || key == "CONTENT_LENGTH" {
            set(&key, value);
        }
        set(&format!("HTTP_{key}"), value);
    }
    if let Some(auth) = req.header("Authorization") {
        for (k, v) in cgi::auth_vars(auth) {
            set(k, &v);
        }
    }
    s.set(ArrayKey::str(b"REQUEST_TIME_FLOAT"), Value::Float(b.request_time));
    s.set(ArrayKey::str(b"REQUEST_TIME"), Value::Int(b.request_time as i64));
    s
}
