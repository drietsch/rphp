//! The FastCGI SAPI (SAPI-4): `rphp -b host:port` answers a web server's
//! FastCGI requests the way php-fpm does — the same `$_SERVER` layout
//! (`USER`, `HOME`, the parameters as received, `FCGI_ROLE`, `PHP_SELF`,
//! the request times), the same CGI response head (`Status:` only when
//! the code is not 200, `X-Powered-By`, the default `Content-type`), the
//! same answers for a missing or forbidden script, `error_log()` on
//! `FCGI_STDERR` as `PHP message: …`, `fastcgi_finish_request()`, and
//! `FCGI_GET_VALUES` answered as php-fpm answers it (`FCGI_MPXS_CONNS` =
//! 0). Measured against `php-fpm -n` (`tools/rphp/tests/fcgi.rs`).
//!
//! Every worker is one long-lived thread with the engine's stack
//! reservation, serving its connections in turn; a connection carries one
//! request at a time (no multiplexing, like php-fpm) and stays open when
//! the web server asked for it (`FCGI_KEEP_CONN`).

use std::io::{self, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rphp_embed::cgi::{self, CgiRequest};
use rphp_embed::{Engine, EngineConfig, Interp, OutputSink};
use rphp_runtime::SharedHead;
use rphp_value::{Array, ArrayKey, Value};

mod record;

use record::*;

/// php's version, for `X-Powered-By`.
const PHP_VERSION: &str = "8.5.0";

/// What `rphp -b` runs with.
pub struct FcgiOptions {
    /// `host:port` to listen on.
    pub bind: String,
    /// `-d` overrides.
    pub ini: Vec<(String, String)>,
    /// Worker threads (`PHP_FCGI_CHILDREN`, default 1).
    pub workers: usize,
}

struct Server {
    engine: Engine,
}

/// Run the FastCGI server until the process is killed. Returns the exit
/// code: 1 when the address cannot be bound.
pub fn run(opts: FcgiOptions) -> i32 {
    let listener = match TcpListener::bind(&opts.bind) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("rphp: unable to bind listening socket for address '{}': {e}", opts.bind);
            return 1;
        }
    };
    let server = Arc::new(Server {
        engine: Engine::new(EngineConfig {
            ini: opts.ini.clone(),
            ..EngineConfig::fcgi()
        }),
    });
    log("NOTICE: ready to handle connections");
    let mut handles = Vec::new();
    for _ in 0..opts.workers.max(1) {
        let l = match listener.try_clone() {
            Ok(l) => l,
            Err(_) => break,
        };
        let s = Arc::clone(&server);
        match rphp_embed::request_thread(move || accept_loop(&s, &l)) {
            Ok(h) => handles.push(h),
            Err(e) => {
                log(&format!("ERROR: failed to start a worker: {e}"));
                break;
            }
        }
    }
    if handles.is_empty() {
        accept_loop(&server, &listener);
    }
    for h in handles {
        let _ = h.join();
    }
    0
}

/// `[20-Sep-2026 18:56:00] <message>` on stderr, as php-fpm logs.
fn log(message: &str) {
    let now = jiff::Zoned::now();
    eprintln!("[{}] {message}", now.strftime("%d-%b-%Y %H:%M:%S"));
}

fn accept_loop(server: &Server, listener: &TcpListener) {
    for conn in listener.incoming() {
        let Ok(stream) = conn else { continue };
        let _ = stream.set_nodelay(true);
        let _ = handle_connection(server, stream);
    }
}

/// A request being assembled from the connection's records.
struct Pending {
    id: u16,
    keep: bool,
    params: Vec<(Vec<u8>, Vec<u8>)>,
    params_done: bool,
    stdin: Vec<u8>,
}

/// Serve every request the connection carries, in turn.
fn handle_connection(server: &Server, mut stream: TcpStream) -> io::Result<()> {
    let mut pending: Option<Pending> = None;
    loop {
        let Some(rec) = read_record(&mut stream)? else {
            return Ok(());
        };
        if rec.request_id == NULL_REQUEST_ID {
            management(&mut stream, &rec)?;
            continue;
        }
        match rec.kind {
            BEGIN_REQUEST => {
                let role = u16::from_be_bytes([rec.content[0], rec.content[1]]);
                let flags = rec.content.get(2).copied().unwrap_or(0);
                if pending.is_some() {
                    // One request per connection, as php-fpm serves them.
                    write_record(&mut stream, END_REQUEST, rec.request_id, &end_request_body(0, CANT_MPX_CONN))?;
                    continue;
                }
                if role != ROLE_RESPONDER {
                    write_record(&mut stream, END_REQUEST, rec.request_id, &end_request_body(0, UNKNOWN_ROLE))?;
                    continue;
                }
                pending = Some(Pending { id: rec.request_id, keep: flags & KEEP_CONN != 0, params: Vec::new(), params_done: false, stdin: Vec::new() });
            }
            ABORT_REQUEST => {
                if pending.as_ref().is_some_and(|p| p.id == rec.request_id) {
                    write_record(&mut stream, END_REQUEST, rec.request_id, &end_request_body(0, REQUEST_COMPLETE))?;
                    pending = None;
                }
            }
            PARAMS => {
                let Some(p) = pending.as_mut().filter(|p| p.id == rec.request_id) else { continue };
                if rec.content.is_empty() {
                    p.params_done = true;
                } else {
                    p.params.extend(decode_pairs(&rec.content));
                }
            }
            STDIN => {
                let Some(p) = pending.as_mut().filter(|p| p.id == rec.request_id) else { continue };
                if rec.content.is_empty() {
                    let req = pending.take().expect("pending request");
                    let keep = req.keep;
                    serve(server, &mut stream, req)?;
                    if !keep {
                        let _ = stream.shutdown(std::net::Shutdown::Both);
                        return Ok(());
                    }
                } else {
                    p.stdin.extend_from_slice(&rec.content);
                }
            }
            other => {
                write_record(&mut stream, UNKNOWN_TYPE, NULL_REQUEST_ID, &[other, 0, 0, 0, 0, 0, 0, 0])?;
            }
        }
    }
}

/// `FCGI_GET_VALUES`: php-fpm answers only `FCGI_MPXS_CONNS` (`0`).
fn management(stream: &mut TcpStream, rec: &Record) -> io::Result<()> {
    if rec.kind != GET_VALUES {
        return write_record(stream, UNKNOWN_TYPE, NULL_REQUEST_ID, &[rec.kind, 0, 0, 0, 0, 0, 0, 0]);
    }
    let mut pairs: Vec<(&[u8], &[u8])> = Vec::new();
    for (name, _) in decode_pairs(&rec.content) {
        if name == b"FCGI_MPXS_CONNS" {
            pairs.push((b"FCGI_MPXS_CONNS", b"0"));
        }
    }
    write_record(stream, GET_VALUES_RESULT, NULL_REQUEST_ID, &encode_pairs(&pairs))
}

/// The response side of one request: `FCGI_STDOUT` records behind the
/// interpreter's output, the CGI head ahead of the first byte,
/// `FCGI_STDERR` for the log, `FCGI_END_REQUEST` once.
struct Response {
    stream: TcpStream,
    id: u16,
    head: SharedHead,
    head_only: bool,
    started: bool,
    ended: bool,
}

type SharedResponse = Arc<Mutex<Response>>;

impl Response {
    /// php-fpm's `sapi_cgi_send_headers`: `Status:` for a code other than
    /// 200 (the script's own status line's text when it set one), the
    /// header list, the default `Content-type` when the script set none.
    fn send_head(&mut self) {
        if self.started {
            return;
        }
        self.started = true;
        let mut out = Vec::new();
        let mut head = self.head.lock().unwrap();
        let code = if head.code == 0 { 200 } else { head.code };
        // A 200 goes without a `Status:` line, whatever the script said.
        if code != 200 {
            if let Some(line) = &head.status_line {
                // `HTTP/1.1 418 Teapot` → `Status: 418 Teapot`.
                let text = line.splitn(2, ' ').nth(1).unwrap_or("");
                out.extend_from_slice(format!("Status: {text}\r\n").as_bytes());
            } else {
                // php's table has no phrase for some codes: `Status: 418`.
                match cgi::reason(code) {
                    Some(r) => out.extend_from_slice(format!("Status: {code} {r}\r\n").as_bytes()),
                    None => out.extend_from_slice(format!("Status: {code}\r\n").as_bytes()),
                }
            }
        }
        if !head.has_content_type && code != 304 {
            head.headers.push(("Content-type".to_string(), "Content-type: text/html; charset=UTF-8".to_string()));
            head.has_content_type = true;
        }
        for (_, line) in &head.headers {
            out.extend_from_slice(line.as_bytes());
            out.extend_from_slice(b"\r\n");
        }
        out.extend_from_slice(b"\r\n");
        head.sent = true;
        drop(head);
        let _ = write_record(&mut self.stream, STDOUT, self.id, &out);
    }

    fn end(&mut self) {
        if self.ended {
            return;
        }
        self.ended = true;
        let _ = write_record(&mut self.stream, STDOUT, self.id, &[]);
        let _ = write_record(&mut self.stream, END_REQUEST, self.id, &end_request_body(0, REQUEST_COMPLETE));
        let _ = self.stream.flush();
    }
}

/// The interpreter's output sink over a shared [`Response`].
struct FcgiSink(SharedResponse);

impl OutputSink for FcgiSink {
    fn write(&mut self, bytes: &[u8]) {
        let mut r = self.0.lock().unwrap();
        if r.ended {
            return;
        }
        r.send_head();
        if !r.head_only && !bytes.is_empty() {
            let id = r.id;
            let _ = write_record(&mut r.stream, STDOUT, id, bytes);
        }
    }

    fn flush(&mut self) {
        let mut r = self.0.lock().unwrap();
        if r.ended {
            return;
        }
        r.send_head();
        let _ = r.stream.flush();
    }

    fn finish_request(&mut self) -> bool {
        let mut r = self.0.lock().unwrap();
        r.send_head();
        r.end();
        true
    }
}

/// A parameter's value as text.
fn param<'a>(params: &'a [(Vec<u8>, Vec<u8>)], name: &str) -> Option<&'a str> {
    params
        .iter()
        .find(|(n, _)| n == name.as_bytes())
        .and_then(|(_, v)| std::str::from_utf8(v).ok())
}

/// Run the script the request names, or answer for it the way php-fpm
/// does when there is none.
fn serve(server: &Server, stream: &mut TcpStream, req: Pending) -> io::Result<()> {
    let response = Arc::new(Mutex::new(Response {
        stream: stream.try_clone()?,
        id: req.id,
        head: SharedHead::default(),
        head_only: param(&req.params, "REQUEST_METHOD") == Some("HEAD"),
        started: false,
        ended: false,
    }));
    let script = param(&req.params, "SCRIPT_FILENAME").map(PathBuf::from);
    let refusal = match &script {
        Some(p) if p.is_dir() => Some((403, "Access denied.\n", format!("Access to the script '{}' has been denied (see security.limit_extensions)", p.display()))),
        Some(p) if p.is_file() => None,
        _ => Some((404, "File not found.\n", "Primary script unknown".to_string())),
    };
    let head = response.lock().unwrap().head.clone();
    // php adds `X-Powered-By` at request startup when `expose_php` is on.
    if expose_php(server) {
        head.lock().unwrap().headers.push(("X-Powered-By".to_string(), format!("X-Powered-By: PHP/{PHP_VERSION}")));
    }
    if let Some((code, body, err)) = refusal {
        write_record(stream, STDERR, req.id, err.as_bytes())?;
        {
            let mut h = head.lock().unwrap();
            h.code = code;
        }
        let mut r = response.lock().unwrap();
        r.send_head();
        if !r.head_only {
            let id = r.id;
            write_record(&mut r.stream, STDOUT, id, body.as_bytes())?;
        }
        r.end();
        return Ok(());
    }
    let script = script.expect("a file");
    let request_time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    let mut it = server.engine.new_interp_unseeded(Box::new(FcgiSink(Arc::clone(&response))));
    it.head = head;
    // `error_log()` goes to the web server as `PHP message: …` while the
    // request is open (php-fpm keeps only its own log after
    // `fastcgi_finish_request()`).
    let err_out = Arc::clone(&response);
    it.error_log = Some(Box::new(move |entry: &str| {
        if let Ok(mut r) = err_out.lock() {
            if !r.ended {
                let id = r.id;
                let _ = write_record(&mut r.stream, STDERR, id, format!("PHP message: {entry}").as_bytes());
            }
        }
    }));
    let (server_vars, headers) = server_array(&req.params, request_time);
    // php-fpm's request environment: `USER`, `HOME` (its cleared process
    // environment), the parameters as its table yields them, `FCGI_ROLE`.
    let mut env: Vec<(String, String)> = Vec::new();
    for key in ["USER", "HOME"] {
        if let Ok(v) = std::env::var(key) {
            env.push((key.to_string(), v));
        }
    }
    for (name, value) in req.params.iter().rev() {
        env.push((String::from_utf8_lossy(name).into_owned(), String::from_utf8_lossy(value).into_owned()));
    }
    env.push(("FCGI_ROLE".to_string(), "RESPONDER".to_string()));
    it.request_env = Some(env);
    let method = param(&req.params, "REQUEST_METHOD").unwrap_or("GET").to_string();
    cgi::bind(
        &mut it,
        CgiRequest {
            method: &method,
            query: param(&req.params, "QUERY_STRING"),
            content_type: param(&req.params, "CONTENT_TYPE").filter(|s| !s.is_empty()),
            cookie: param(&req.params, "HTTP_COOKIE"),
            headers,
            body: &req.stdin,
            server: server_vars,
            request_time,
        },
    );
    let ob = it.ini.int("output_buffering");
    if ob > 0 {
        it.out.push(None, ob as usize, rphp_runtime::PHP_OUTPUT_HANDLER_STDFLAGS);
    }
    if let Some(dir) = script.parent() {
        it.cwd = dir.to_path_buf();
    }
    run_script(server, &mut it, &script);
    finish(&mut it);
    for f in std::mem::take(&mut it.uploaded_files) {
        let _ = std::fs::remove_file(f);
    }
    drop(it);
    let mut r = response.lock().unwrap();
    r.send_head();
    r.end();
    Ok(())
}

fn expose_php(server: &Server) -> bool {
    server
        .engine
        .config()
        .ini
        .iter()
        .rev()
        .find(|(k, _)| k == "expose_php")
        .is_none_or(|(_, v)| rphp_runtime::parse_bool(v))
}

/// Compile and run the script as the main program.
fn run_script(server: &Server, it: &mut Interp, path: &Path) {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(_) => return,
    };
    let name = path.to_string_lossy().into_owned();
    it.script_path = Some(path.to_path_buf());
    if let Err(err) = server.engine.load(it, &bytes, &name) {
        server.engine.report_load_error(it, &name, err);
        return;
    }
    if let Err(u) = it.run_main() {
        it.handle_top_level_unwind(u);
    }
}

/// php's request shutdown: shutdown functions, destructors, the output
/// buffers flushed through the sink.
fn finish(it: &mut Interp) {
    let _ = it.run_shutdown_functions(0);
    if let Err(u) = it.shutdown_destructors() {
        it.handle_top_level_unwind(u);
    }
    it.finish_output();
}

/// `$_SERVER` as php-fpm lays it out — `USER` and `HOME`, the parameters
/// in the order php-fpm's table yields them (the reverse of arrival),
/// `FCGI_ROLE`, `PHP_SELF`, the request times — and the header list for
/// `getallheaders()` in the same pass.
fn server_array(params: &[(Vec<u8>, Vec<u8>)], request_time: f64) -> (Array, Vec<(String, String)>) {
    let mut s = Array::new();
    let mut set = |k: &str, v: &[u8]| {
        s.set(ArrayKey::str(k.as_bytes()), Value::string(v));
    };
    if let Ok(user) = std::env::var("USER") {
        set("USER", user.as_bytes());
    }
    if let Ok(home) = std::env::var("HOME") {
        set("HOME", home.as_bytes());
    }
    let mut headers = Vec::new();
    for (name, value) in params.iter().rev() {
        let key = String::from_utf8_lossy(name).into_owned();
        set(&key, value);
        let header = if let Some(rest) = key.strip_prefix("HTTP_") {
            Some(header_name(rest))
        } else if key == "CONTENT_TYPE" || key == "CONTENT_LENGTH" {
            Some(header_name(&key))
        } else {
            None
        };
        if let Some(h) = header {
            headers.push((h, String::from_utf8_lossy(value).into_owned()));
        }
    }
    set("FCGI_ROLE", b"RESPONDER");
    if let Some(auth) = param(params, "HTTP_AUTHORIZATION") {
        for (k, v) in cgi::auth_vars(auth) {
            set(k, v.as_bytes());
        }
    }
    let script_name = param(params, "SCRIPT_NAME").unwrap_or("");
    let php_self = match param(params, "PATH_INFO") {
        Some(pi) => format!("{script_name}{pi}"),
        None => script_name.to_string(),
    };
    set("PHP_SELF", php_self.as_bytes());
    s.set(ArrayKey::str(b"REQUEST_TIME_FLOAT"), Value::Float(request_time));
    s.set(ArrayKey::str(b"REQUEST_TIME"), Value::Int(request_time as i64));
    (s, headers)
}

/// `USER_AGENT` → `User-Agent`, as php-fpm's `getallheaders()` spells it.
fn header_name(key: &str) -> String {
    key.split('_')
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_ascii_uppercase().to_string() + &c.as_str().to_ascii_lowercase(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join("-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_names_are_spelled_like_fpm() {
        assert_eq!(header_name("USER_AGENT"), "User-Agent");
        assert_eq!(header_name("CONTENT_LENGTH"), "Content-Length");
        assert_eq!(header_name("HOST"), "Host");
    }

    #[test]
    fn php_self_takes_path_info() {
        let params = vec![(b"SCRIPT_NAME".to_vec(), b"/a.php".to_vec()), (b"PATH_INFO".to_vec(), b"/x".to_vec())];
        let (s, _) = server_array(&params, 1.0);
        assert_eq!(s.get(&ArrayKey::str(b"PHP_SELF")).map(|v| v.to_php_string()), Some("/a.php/x".to_string()));
    }
}
