//! The built-in web server: `rphp -S host:port [-t docroot] [router.php]`,
//! php's `cli-server` SAPI (ADR-033).
//!
//! One connection carries one request (`Connection: close`, as php's does),
//! served in turn on the accept loop: the request is read, its path is
//! resolved under the document root the way `php -S` resolves it
//! (`resolve.rs`), and it is either a static file sent from disk or a php
//! script run in a fresh interpreter whose superglobals `request.rs` seeds
//! from the request. `PHP_CLI_SERVER_WORKERS=n` runs `n` accept loops.
//!
//! A script's response is a php response: the status line and the header
//! list the script built (`header()`, `setcookie()`, `http_response_code()`),
//! sent by the sink ahead of the first body byte — which, with php's
//! `output_buffering=4096` and no `implicit_flush`, is the first 4 KiB chunk,
//! an explicit `flush()`, or the end of the script. The 404/405/400 pages,
//! the static-file headers, the access log's lines and stamps are php's.
#![forbid(unsafe_code)]

mod http;
mod mime;
mod multipart;
mod request;
mod resolve;

use std::io::Write;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rphp_embed::{Engine, EngineConfig, OutputSink};
use rphp_runtime::{Interp, SharedHead};
use rphp_value::Value;

use http::{ReadError, Request};

/// The version the server announces (`X-Powered-By`, `SERVER_SOFTWARE`).
pub const PHP_VERSION: &str = "8.5.0";

/// What `rphp -S` was asked to serve.
#[derive(Clone, Debug)]
pub struct ServerOptions {
    /// `host:port` as given (`localhost:8000`, `0.0.0.0:80`, `[::1]:8000`).
    pub listen: String,
    /// `-t`: the document root (the working directory when absent).
    pub docroot: Option<PathBuf>,
    /// The router script, run before every request.
    pub router: Option<PathBuf>,
    /// `-d` overrides.
    pub ini: Vec<(String, String)>,
}

/// Run the server until the process is killed. Returns the exit code:
/// 1 when the address cannot be bound or the document root does not exist.
pub fn run(opts: ServerOptions) -> i32 {
    let (host, port) = match split_listen(&opts.listen) {
        Some(hp) => hp,
        None => {
            eprintln!("Invalid address: {}", opts.listen);
            return 1;
        }
    };
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let docroot = match &opts.docroot {
        Some(d) => {
            let d = if d.is_absolute() { d.clone() } else { cwd.join(d) };
            match d.canonicalize() {
                Ok(c) if c.is_dir() => c,
                _ => {
                    eprintln!("Directory {} does not exist.", d.display());
                    return 1;
                }
            }
        }
        None => cwd.canonicalize().unwrap_or(cwd.clone()),
    };
    let router = opts.router.as_ref().map(|r| {
        let r = if r.is_absolute() { r.clone() } else { cwd.join(r) };
        r.canonicalize().unwrap_or(r)
    });
    if let Some(r) = &router {
        if !r.is_file() {
            eprintln!("Could not open input file: {}", r.display());
            return 1;
        }
    }
    let listener = match TcpListener::bind((host.as_str(), port)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("Failed to listen on {host}:{port} (reason: {})", reason_text(&e));
            return 1;
        }
    };
    let bound = listener.local_addr().map(|a| a.port()).unwrap_or(port);
    let server = Arc::new(Server {
        engine: Engine::new(EngineConfig {
            ini: opts.ini.clone(),
            ..EngineConfig::server()
        }),
        host: host.clone(),
        port: bound,
        docroot,
        router,
        cwd,
    });
    log(&format!(
        "PHP {PHP_VERSION} Development Server (http://{host}:{bound}) started"
    ));
    let workers: usize = std::env::var("PHP_CLI_SERVER_WORKERS")
        .ok()
        .and_then(|w| w.parse().ok())
        .filter(|w| *w >= 1)
        .unwrap_or(1);
    // Every worker is one long-lived thread with the engine's stack
    // reservation, serving its connections in turn: the thread keeps the
    // compiled-unit cache warm across requests (opcache's role).
    let mut handles = Vec::new();
    for _ in 0..workers {
        let l = match listener.try_clone() {
            Ok(l) => l,
            Err(_) => break,
        };
        let s = Arc::clone(&server);
        match rphp_embed::request_thread(move || accept_loop(&s, &l)) {
            Ok(h) => handles.push(h),
            Err(e) => {
                log(&format!("Failed to start a worker: {e}"));
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

/// `host:port` → `(host, port)`; a bracketed IPv6 host loses its brackets.
fn split_listen(listen: &str) -> Option<(String, u16)> {
    let (host, port) = listen.rsplit_once(':')?;
    let port: u16 = port.parse().ok()?;
    let host = host.trim_start_matches('[').trim_end_matches(']');
    Some((host.to_string(), port))
}

fn reason_text(e: &std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::AddrInUse => "Address already in use".to_string(),
        std::io::ErrorKind::PermissionDenied => "Permission denied".to_string(),
        _ => e.to_string(),
    }
}

/// The process-wide state every request shares.
struct Server {
    engine: Engine,
    host: String,
    port: u16,
    docroot: PathBuf,
    router: Option<PathBuf>,
    /// Where the server was started: the router runs with this working
    /// directory, a script with its own.
    cwd: PathBuf,
}

fn accept_loop(server: &Server, listener: &TcpListener) {
    for conn in listener.incoming() {
        let Ok(stream) = conn else { continue };
        let remote = match stream.peer_addr() {
            Ok(a) => a,
            Err(_) => continue,
        };
        let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(60)));
        let _ = stream.set_nodelay(true);
        handle_connection(server, stream, remote);
    }
}

/// `[Sun Sep 20 11:49:38 2026] <message>` on stderr, as `php -S` logs.
fn log(message: &str) {
    eprintln!("[{}] {message}", stamp());
}

/// The machine's local time in `ctime()` form (`Sun Sep  5 09:04:01 2026`).
fn stamp() -> String {
    let now = jiff::Zoned::now();
    now.strftime("%a %b %e %H:%M:%S %Y").to_string()
}

fn handle_connection(server: &Server, mut stream: TcpStream, remote: SocketAddr) {
    log(&format!("{remote} Accepted"));
    match http::read_request(&mut stream, remote) {
        Ok(req) => dispatch(server, &mut stream, &req),
        Err(ReadError::Empty) => {}
        Err(ReadError::Io(e)) => log(&format!("{remote} Invalid request ({e})")),
        Err(ReadError::Bad(why)) => {
            let head = ResponseHeadCtx {
                version: (1, 1),
                host: None,
            };
            send_error_page(&mut stream, &head, 400, "", "GET", "?", false);
            log(&format!("{remote} Invalid request (Malformed HTTP request: {why})"));
        }
    }
    let _ = stream.flush();
    let _ = stream.shutdown(std::net::Shutdown::Write);
    log(&format!("{remote} Closing"));
}

/// What the head of every response repeats from the request.
#[derive(Clone)]
struct ResponseHeadCtx {
    version: (u8, u8),
    /// The request's `Host`, echoed back as php's server does.
    host: Option<String>,
}

impl ResponseHeadCtx {
    fn of(req: &Request) -> ResponseHeadCtx {
        ResponseHeadCtx {
            version: req.version,
            host: req.header("Host").map(str::to_string),
        }
    }

    /// `Host`, `Date` (unless the script set one), `Connection: close`.
    fn essential(&self, out: &mut Vec<u8>, has_date: bool) {
        if let Some(h) = &self.host {
            out.extend_from_slice(format!("Host: {h}\r\n").as_bytes());
        }
        if !has_date {
            out.extend_from_slice(format!("Date: {}\r\n", http::http_date_now()).as_bytes());
        }
        out.extend_from_slice(b"Connection: close\r\n");
    }
}

/// php's `[200]: GET /path` access-log line, with a message when there is
/// one and the last fatal error's text when the script died of one.
fn log_response(req: &Request, status: i64, message: Option<&str>, error: Option<String>) {
    let mut line = format!("{} [{}]: {} {}", req.remote, status, req.method, req.uri);
    if let Some(m) = message {
        line.push_str(&format!(" - {m}"));
    }
    if let Some(e) = error {
        line.push_str(&format!(" - {e}"));
    }
    log(&line);
}

/// Serve one request: router, script or static file.
fn dispatch(server: &Server, stream: &mut TcpStream, req: &Request) {
    let vpath = resolve::normalize_vpath(&req.raw_path);
    let translated = resolve::translate(&server.docroot, &vpath);
    let is_static = translated.as_ref().is_none_or(|t| !t.is_php);
    let head_ctx = ResponseHeadCtx::of(req);

    if server.router.is_none() && is_static {
        send_static(stream, &head_ctx, req, translated.as_ref());
        return;
    }

    // A request that runs php: one interpreter, its response head shared
    // with the sink that sends it.
    let request_time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    let sink = ResponseSink::new(stream, head_ctx.clone(), req.method == "HEAD");
    let head = sink.head.clone();
    let mut it = server.engine.new_interp_unseeded(Box::new(sink));
    it.head = head.clone();
    it.error_log = Some(Box::new(|entry: &str| log(entry)));
    // php adds `X-Powered-By` at request startup when `expose_php` is on.
    if it.ini.bool("expose_php") {
        head.lock().unwrap().headers.push((
            "X-Powered-By".to_string(),
            format!("X-Powered-By: PHP/{PHP_VERSION}"),
        ));
    }
    request::bind(
        &mut it,
        &request::Binding {
            request: req,
            docroot: &server.docroot,
            host: &server.host,
            port: server.port,
            vpath: &vpath,
            script_name: translated.as_ref().map(|t| t.script_name.as_str()),
            script_filename: translated.as_ref().map(|t| t.path.as_path()),
            path_info: translated.as_ref().and_then(|t| t.path_info.as_deref()),
            router: server.router.as_deref(),
            request_time,
        },
    );
    // `output_buffering=4096`: the level every request starts inside.
    let ob = it.ini.int("output_buffering");
    if ob > 0 {
        it.out
            .push(None, ob as usize, rphp_runtime::PHP_OUTPUT_HANDLER_STDFLAGS);
    }

    let mut declined = false;
    if let Some(router) = &server.router {
        it.cwd = server.cwd.clone();
        match run_script(server, &mut it, router, true) {
            Outcome::Returned(v) => declined = matches!(v, Value::Bool(false)),
            Outcome::Ended => {}
            Outcome::NotLoaded => {
                finish(&mut it);
                log_response(req, 500, None, None);
                cleanup(&mut it);
                return;
            }
        }
        if !declined {
            finish(&mut it);
            log_response(req, status_of(&head), None, last_fatal(&it));
            cleanup(&mut it);
            return;
        }
    }
    if !is_static {
        let t = translated.as_ref().expect("a php script");
        it.cwd = t.path.parent().map(Path::to_path_buf).unwrap_or_else(|| server.cwd.clone());
        if let Outcome::NotLoaded = run_script(server, &mut it, &t.path, false) {
            // A parse error is a php response of its own (already sent).
        }
        finish(&mut it);
        log_response(req, status_of(&head), None, last_fatal(&it));
        cleanup(&mut it);
        return;
    }
    // The router declined and the target is a file: php throws the
    // request state away (its head is never sent) and serves the file.
    discard_output(&mut it);
    cleanup(&mut it);
    send_static(stream, &head_ctx, req, translated.as_ref());
}

/// How running a script ended.
enum Outcome {
    /// The script's `return` value (`null` when it fell off the end).
    Returned(Value),
    /// A fatal error or `exit`, already rendered.
    Ended,
    /// It did not compile (the parse error was rendered).
    NotLoaded,
}

/// Compile and run `path` as the main script.
fn run_script(server: &Server, it: &mut Interp, path: &Path, keep_script_path: bool) -> Outcome {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(_) => return Outcome::NotLoaded,
    };
    let name = path.to_string_lossy().into_owned();
    if !keep_script_path {
        it.script_path = Some(path.to_path_buf());
    }
    if let Err(err) = server.engine.load(it, &bytes, &name) {
        server.engine.report_load_error(it, &name, err);
        return Outcome::NotLoaded;
    }
    match it.run_main() {
        Ok(v) => Outcome::Returned(v),
        Err(u) => {
            it.handle_top_level_unwind(u);
            Outcome::Ended
        }
    }
}

/// php's request shutdown: shutdown functions, destructors, the output
/// buffers flushed through the sink (which sends the head if nothing has).
fn finish(it: &mut Interp) {
    let code = it.run_shutdown_functions(0);
    if let Err(u) = it.shutdown_destructors() {
        it.handle_top_level_unwind(u);
    }
    let _ = code;
    it.finish_output();
}

/// Drop whatever the router buffered without sending a head.
fn discard_output(it: &mut Interp) {
    while it.out.level() > 0 {
        let _ = it.out.pop();
    }
}

/// Remove the request's uploaded temporary files.
fn cleanup(it: &mut Interp) {
    for f in std::mem::take(&mut it.uploaded_files) {
        let _ = std::fs::remove_file(f);
    }
}

fn status_of(head: &SharedHead) -> i64 {
    let code = head.lock().map(|h| h.code).unwrap_or(0);
    if code == 0 {
        200
    } else {
        code
    }
}

/// `<message> in <file> on line <n>` of the fatal error the script died
/// of, for the access log.
fn last_fatal(it: &Interp) -> Option<String> {
    let e = it.last_error.as_ref()?;
    // E_ERROR | E_PARSE | E_CORE_ERROR | E_COMPILE_ERROR | E_USER_ERROR | E_RECOVERABLE_ERROR
    if e.kind & (1 | 4 | 16 | 64 | 256 | 4096) == 0 {
        return None;
    }
    Some(format!("{} in {} on line {}", e.message, e.file, e.line))
}

// ---- static files and error pages ------------------------------------------------

/// A static file, or php's 404 / 405 page.
fn send_static(stream: &mut TcpStream, ctx: &ResponseHeadCtx, req: &Request, t: Option<&resolve::Translated>) {
    if matches!(req.method.as_str(), "DELETE" | "PUT" | "PATCH") {
        send_error_page(stream, ctx, 405, &req.uri, &req.method, &req.remote.to_string(), true);
        return;
    }
    let Some(t) = t else {
        send_error_page(stream, ctx, 404, &req.uri, &req.method, &req.remote.to_string(), true);
        return;
    };
    let Ok(bytes) = std::fs::read(&t.path) else {
        send_error_page(stream, ctx, 404, &req.uri, &req.method, &req.remote.to_string(), true);
        return;
    };
    let mut out = Vec::new();
    out.extend_from_slice(http::status_line(ctx.version, 200).as_bytes());
    ctx.essential(&mut out, false);
    if let Some(mime) = mime::lookup(resolve::extension(&t.script_name)) {
        out.extend_from_slice(b"Content-Type: ");
        out.extend_from_slice(mime.as_bytes());
        if mime.starts_with("text/") {
            out.extend_from_slice(b"; charset=UTF-8");
        }
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(format!("Content-Length: {}\r\n\r\n", bytes.len()).as_bytes());
    if req.method != "HEAD" {
        out.extend_from_slice(&bytes);
    }
    let _ = stream.write_all(&out);
    log_response(req, 200, None, None);
}

/// php's stylesheet for its error pages (`php_cli_server_css`).
const ERROR_CSS: &str = "<style>\n\
body { background-color: #fcfcfc; color: #333333; margin: 0; padding:0; }\n\
h1 { font-size: 1.5em; font-weight: normal; background-color: #9999cc; min-height:2em; line-height:2em; border-bottom: 1px inset black; margin: 0; }\n\
h1, p { padding-left: 10px; }\n\
code.url { background-color: #eeeeee; font-family:monospace; padding:0 2px;}\n\
</style>\n";

/// The 400 / 404 / 405 / 500 / 501 pages.
#[allow(clippy::too_many_arguments)]
fn send_error_page(
    stream: &mut TcpStream,
    ctx: &ResponseHeadCtx,
    status: i64,
    uri: &str,
    method: &str,
    remote: &str,
    log_it: bool,
) {
    let reason = http::reason(status);
    let escaped = html_escape(uri);
    let content = match status {
        400 => format!("<h1>{reason}</h1><p>Your browser sent a request that this server could not understand.</p>"),
        404 => format!(
            "<h1>{reason}</h1><p>The requested resource <code class=\"url\">{escaped}</code> was not found on this server.</p>"
        ),
        405 => format!("<h1>{reason}</h1><p>Requested method not allowed.</p>"),
        500 => format!("<h1>{reason}</h1><p>The server is temporarily unavailable.</p>"),
        _ => format!("<h1>{reason}</h1><p>Request method not supported.</p>"),
    };
    let body = if method == "HEAD" {
        String::new()
    } else {
        format!(
            "<!doctype html><html><head><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>{status} {reason}</title>{ERROR_CSS}</head><body>{content}</body></html>"
        )
    };
    let mut out = Vec::new();
    out.extend_from_slice(http::status_line(ctx.version, status).as_bytes());
    ctx.essential(&mut out, false);
    out.extend_from_slice(format!("X-Powered-By: PHP/{PHP_VERSION}\r\n").as_bytes());
    out.extend_from_slice(b"Content-Type: text/html; charset=UTF-8\r\n");
    out.extend_from_slice(format!("Content-Length: {}\r\n", body.len()).as_bytes());
    if status == 405 {
        out.extend_from_slice(b"Allow: GET, HEAD, POST\r\n");
    }
    out.extend_from_slice(b"\r\n");
    out.extend_from_slice(body.as_bytes());
    let _ = stream.write_all(&out);
    if log_it {
        let message = match status {
            404 => "No such file or directory",
            405 => "Method not allowed",
            400 => "Malformed HTTP request",
            _ => "?",
        };
        log(&format!("{remote} [{status}]: {method} {uri} - {message}"));
    }
}

fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#039;"),
            c => out.push(c),
        }
    }
    out
}

// ---- the response sink -------------------------------------------------------------

/// The interpreter's output sink for one request: sends the response head
/// ahead of the first byte (or on `flush()`), then streams the body — or
/// drops it, for a `HEAD` request.
struct ResponseSink {
    stream: TcpStream,
    ctx: ResponseHeadCtx,
    head: SharedHead,
    head_only: bool,
    started: bool,
}

impl ResponseSink {
    fn new(stream: &TcpStream, ctx: ResponseHeadCtx, head_only: bool) -> ResponseSink {
        ResponseSink {
            stream: stream.try_clone().expect("socket clone"),
            ctx,
            head: SharedHead::default(),
            head_only,
            started: false,
        }
    }

    /// php's `sapi_cli_server_send_headers`: the status line, the essential
    /// headers, the list, and the default `Content-type` when the script set
    /// none (never for a 304).
    fn send_head(&mut self) {
        if self.started {
            return;
        }
        self.started = true;
        let mut out = Vec::new();
        let mut head = self.head.lock().unwrap();
        match &head.status_line {
            Some(line) => {
                out.extend_from_slice(line.as_bytes());
                out.extend_from_slice(b"\r\n");
            }
            None => out.extend_from_slice(http::status_line(self.ctx.version, head.code).as_bytes()),
        }
        let has_date = head.headers.iter().any(|(n, _)| n.eq_ignore_ascii_case("Date"));
        self.ctx.essential(&mut out, has_date);
        // php appends the default content type to the list itself, where
        // `headers_list()` sees it from then on.
        if !head.has_content_type && head.code != 304 {
            head.headers.push((
                "Content-type".to_string(),
                "Content-type: text/html; charset=UTF-8".to_string(),
            ));
            head.has_content_type = true;
        }
        for (_, line) in &head.headers {
            out.extend_from_slice(line.as_bytes());
            out.extend_from_slice(b"\r\n");
        }
        out.extend_from_slice(b"\r\n");
        head.sent = true;
        drop(head);
        let _ = self.stream.write_all(&out);
    }
}

impl OutputSink for ResponseSink {
    fn write(&mut self, bytes: &[u8]) {
        self.send_head();
        if !self.head_only {
            let _ = self.stream.write_all(bytes);
        }
    }

    fn flush(&mut self) {
        self.send_head();
        let _ = self.stream.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listen_addresses() {
        assert_eq!(split_listen("localhost:8000"), Some(("localhost".into(), 8000)));
        assert_eq!(split_listen("[::1]:8000"), Some(("::1".into(), 8000)));
        assert_eq!(split_listen("0.0.0.0:80"), Some(("0.0.0.0".into(), 80)));
        assert_eq!(split_listen("nope"), None);
    }

    #[test]
    fn escapes() {
        assert_eq!(html_escape("/a<b>&\"'"), "/a&lt;b&gt;&amp;&quot;&#039;");
    }
}
