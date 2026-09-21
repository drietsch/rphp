//! The FastCGI SAPI against `php-fpm` (SAPI-4): both listen on ephemeral
//! ports, every request below is sent to each as a web server would
//! (nginx's `fastcgi_params` layout), and the raw FastCGI responses —
//! the CGI head, the body, the stderr stream, the end-request status —
//! must match byte for byte once the few values no engine decides
//! (request times, session ids, upload temp names, clock reads) are
//! replaced by placeholders.
//!
//! **Skipped** (not failed) when no php-fpm is found (`PHP_FPM_BIN`, else
//! `../sbin/php-fpm` next to the php found for the other harnesses, else
//! `php-fpm` on `PATH`).

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use rphp_test::differential::{find_on_path, find_php};

/// The binary under test.
const RPHP: &str = env!("CARGO_BIN_EXE_rphp");

/// The ini both servers run under (php-fpm's own defaults plus the pins
/// the tests print under).
const INI: &[(&str, &str)] = &[
    ("display_errors", "1"),
    ("log_errors", "0"),
    ("error_reporting", "E_ALL"),
    ("date.timezone", "UTC"),
    ("precision", "14"),
    ("serialize_precision", "-1"),
    ("short_open_tag", "0"),
    ("zend.assertions", "-1"),
];

fn docroot() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/http/docroot");
    dir.canonicalize().unwrap_or_else(|e| panic!("{} should exist: {e}", dir.display()))
}

fn find_php_fpm() -> Option<PathBuf> {
    if let Some(bin) = std::env::var_os("PHP_FPM_BIN") {
        let bin = PathBuf::from(bin);
        return bin.is_file().then_some(bin);
    }
    if let Some(php) = find_php() {
        let php = php.canonicalize().unwrap_or(php);
        if let Some(prefix) = php.parent().and_then(Path::parent) {
            let fpm = prefix.join("sbin/php-fpm");
            if fpm.is_file() {
                return Some(fpm);
            }
        }
    }
    find_on_path("php-fpm")
}

struct Server {
    child: Child,
    port: u16,
    name: &'static str,
    _tmp: Option<PathBuf>,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(t) = &self._tmp {
            let _ = std::fs::remove_dir_all(t);
        }
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn wait_listening(child: &mut Child, port: u16, name: &str) {
    let start = Instant::now();
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            panic!("{name} exited before listening: {status}");
        }
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(30), "{name} did not start listening");
        std::thread::sleep(Duration::from_millis(30));
    }
}

fn start_fpm(bin: &Path) -> Server {
    let port = free_port();
    let tmp = std::env::temp_dir().join(format!("rphp-fcgi-test-{}-{port}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    let conf = tmp.join("php-fpm.conf");
    std::fs::write(
        &conf,
        format!(
            "[global]\nerror_log = {}\ndaemonize = no\n[www]\nlisten = 127.0.0.1:{port}\npm = static\npm.max_children = 1\ncatch_workers_output = yes\n",
            tmp.join("error.log").display()
        ),
    )
    .unwrap();
    let mut cmd = Command::new(bin);
    cmd.arg("-n").arg("-F").arg("-y").arg(&conf);
    for (k, v) in INI {
        cmd.arg("-d").arg(format!("{k}={v}"));
    }
    cmd.env("TZ", "UTC").env("LC_ALL", "C").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    let mut child = cmd.spawn().unwrap_or_else(|e| panic!("cannot start php-fpm ({}): {e}", bin.display()));
    wait_listening(&mut child, port, "php-fpm");
    Server { child, port, name: "php-fpm", _tmp: Some(tmp) }
}

fn start_rphp() -> Server {
    let port = free_port();
    let mut cmd = Command::new(RPHP);
    for (k, v) in INI {
        cmd.arg("-d").arg(format!("{k}={v}"));
    }
    cmd.arg("-b").arg(format!("127.0.0.1:{port}"));
    cmd.env("TZ", "UTC").env("LC_ALL", "C").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    let mut child = cmd.spawn().unwrap_or_else(|e| panic!("cannot start rphp: {e}"));
    wait_listening(&mut child, port, "rphp");
    Server { child, port, name: "rphp", _tmp: None }
}

// ---- a FastCGI client ------------------------------------------------------

fn record(kind: u8, id: u16, content: &[u8]) -> Vec<u8> {
    let mut out = vec![1, kind, (id >> 8) as u8, id as u8, (content.len() >> 8) as u8, content.len() as u8, 0, 0];
    out.extend_from_slice(content);
    out
}

fn pairs(params: &[(String, String)]) -> Vec<u8> {
    let mut out = Vec::new();
    for (k, v) in params {
        for len in [k.len(), v.len()] {
            if len < 128 {
                out.push(len as u8);
            } else {
                out.extend_from_slice(&((len as u32) | 0x8000_0000).to_be_bytes());
            }
        }
        out.extend_from_slice(k.as_bytes());
        out.extend_from_slice(v.as_bytes());
    }
    out
}

/// What came back: stdout, stderr, the end-request app status.
struct Reply {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    app_status: Option<u32>,
}

fn exchange(server: &Server, params: &[(String, String)], body: &[u8]) -> Reply {
    let mut s = TcpStream::connect(("127.0.0.1", server.port)).unwrap_or_else(|e| panic!("connect to {}: {e}", server.name));
    s.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
    let mut out = record(1, 1, &[0, 1, 0, 0, 0, 0, 0, 0]);
    let p = pairs(params);
    for chunk in p.chunks(32000) {
        out.extend(record(4, 1, chunk));
    }
    out.extend(record(4, 1, &[]));
    for chunk in body.chunks(32000) {
        out.extend(record(5, 1, chunk));
    }
    out.extend(record(5, 1, &[]));
    s.write_all(&out).unwrap();
    let mut raw = Vec::new();
    let _ = s.read_to_end(&mut raw);
    let mut reply = Reply { stdout: Vec::new(), stderr: Vec::new(), app_status: None };
    let mut i = 0;
    while i + 8 <= raw.len() {
        let kind = raw[i + 1];
        let len = ((raw[i + 4] as usize) << 8) | raw[i + 5] as usize;
        let pad = raw[i + 6] as usize;
        let content = &raw[i + 8..(i + 8 + len).min(raw.len())];
        match kind {
            6 => reply.stdout.extend_from_slice(content),
            7 => reply.stderr.extend_from_slice(content),
            3 if content.len() >= 4 => reply.app_status = Some(u32::from_be_bytes([content[0], content[1], content[2], content[3]])),
            _ => {}
        }
        i += 8 + len + pad;
    }
    reply
}

// ---- requests --------------------------------------------------------------

struct Req {
    method: &'static str,
    /// The request target (`/index.php/extra?x=1`): the script is the
    /// part up to `.php`, the rest `PATH_INFO`.
    target: &'static str,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

fn get(target: &'static str) -> Req {
    Req { method: "GET", target, headers: Vec::new(), body: Vec::new() }
}

fn with(mut r: Req, name: &str, value: &str) -> Req {
    r.headers.push((name.to_string(), value.to_string()));
    r
}

fn form(method: &'static str, target: &'static str, body: &str) -> Req {
    Req {
        method,
        target,
        headers: vec![("Content-Type".to_string(), "application/x-www-form-urlencoded".to_string())],
        body: body.as_bytes().to_vec(),
    }
}

fn multipart(target: &'static str) -> Req {
    let b = "------------------------ladderboundary";
    let body = format!(
        "--{b}\r\nContent-Disposition: form-data; name=\"a\"\r\n\r\n1\r\n\
         --{b}\r\nContent-Disposition: form-data; name=\"b[]\"\r\n\r\n2\r\n\
         --{b}\r\nContent-Disposition: form-data; name=\"f\"; filename=\"up.txt\"\r\nContent-Type: text/plain\r\n\r\nfile content\r\n\
         --{b}--\r\n"
    );
    Req {
        method: "POST",
        target,
        headers: vec![("Content-Type".to_string(), format!("multipart/form-data; boundary={b}"))],
        body: body.into_bytes(),
    }
}

/// nginx's `fastcgi_params` for a request, `SCRIPT_FILENAME` under
/// `docroot` (missing scripts stay missing: the servers answer for them).
fn params(root: &Path, req: &Req) -> Vec<(String, String)> {
    let (path, query) = req.target.split_once('?').map_or((req.target, None), |(p, q)| (p, Some(q)));
    let (script, path_info) = match path.find(".php") {
        Some(i) => (&path[..i + 4], &path[i + 4..]),
        None => (path, ""),
    };
    let mut p = vec![
        ("SCRIPT_FILENAME".to_string(), format!("{}{script}", root.display())),
        ("QUERY_STRING".to_string(), query.unwrap_or("").to_string()),
        ("REQUEST_METHOD".to_string(), req.method.to_string()),
        ("CONTENT_TYPE".to_string(), req.headers.iter().find(|(k, _)| k == "Content-Type").map(|(_, v)| v.clone()).unwrap_or_default()),
        ("CONTENT_LENGTH".to_string(), if req.body.is_empty() { String::new() } else { req.body.len().to_string() }),
        ("SCRIPT_NAME".to_string(), script.to_string()),
        ("REQUEST_URI".to_string(), req.target.to_string()),
        ("DOCUMENT_URI".to_string(), path.to_string()),
        ("DOCUMENT_ROOT".to_string(), root.display().to_string()),
        ("SERVER_PROTOCOL".to_string(), "HTTP/1.1".to_string()),
        ("REQUEST_SCHEME".to_string(), "http".to_string()),
        ("GATEWAY_INTERFACE".to_string(), "CGI/1.1".to_string()),
        ("SERVER_SOFTWARE".to_string(), "nginx/1.27.0".to_string()),
        ("REMOTE_ADDR".to_string(), "127.0.0.1".to_string()),
        ("REMOTE_PORT".to_string(), "54321".to_string()),
        ("SERVER_ADDR".to_string(), "127.0.0.1".to_string()),
        ("SERVER_PORT".to_string(), "80".to_string()),
        ("SERVER_NAME".to_string(), "example.test".to_string()),
        ("REDIRECT_STATUS".to_string(), "200".to_string()),
    ];
    if !path_info.is_empty() {
        p.push(("PATH_INFO".to_string(), path_info.to_string()));
        p.push(("PATH_TRANSLATED".to_string(), format!("{}{path_info}", root.display())));
    }
    p.push(("HTTP_HOST".to_string(), "example.test".to_string()));
    p.push(("HTTP_USER_AGENT".to_string(), "rphp-fcgi-test".to_string()));
    p.push(("HTTP_ACCEPT".to_string(), "*/*".to_string()));
    for (k, v) in &req.headers {
        let key: String = k.chars().map(|c| if c == '-' { '_' } else { c.to_ascii_uppercase() }).collect();
        if key != "CONTENT_TYPE" {
            p.push((format!("HTTP_{key}"), v.clone()));
        }
    }
    p
}

/// The placeholders: request times, session ids, upload temp names, the
/// clock-relative `Max-Age`, object ids.
fn normalize(raw: &[u8]) -> String {
    let text = String::from_utf8_lossy(raw).into_owned();
    let out = regex_replace(&text, "PHPSESSID=", |rest| rest.chars().take_while(|c| c.is_ascii_alphanumeric()).count(), "PHPSESSID=%SID%");
    let out = regex_replace(&out, "tmp/php", |rest| rest.chars().take_while(|c| c.is_ascii_alphanumeric()).count(), "tmp/php%TMP%");
    let out = regex_replace(&out, "Max-Age=", |rest| rest.chars().take_while(|c| c.is_ascii_digit()).count(), "Max-Age=%N%");
    let out = regex_replace(&out, "'REQUEST_TIME_FLOAT' => ", |rest| rest.chars().take_while(|c| c.is_ascii_digit() || *c == '.').count(), "'REQUEST_TIME_FLOAT' => %T%");
    let out = regex_replace(&out, "'REQUEST_TIME' => ", |rest| rest.chars().take_while(|c| c.is_ascii_digit()).count(), "'REQUEST_TIME' => %T%");
    regex_replace(&out, "#", |rest| {
        let n = rest.chars().take_while(|c| c.is_ascii_digit()).count();
        if n > 0 && rest[n..].starts_with(' ') {
            n
        } else {
            0
        }
    }, "#%ID%")
}

fn regex_replace(text: &str, prefix: &str, len: impl Fn(&str) -> usize, with: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find(prefix) {
        out.push_str(&rest[..i]);
        let after = &rest[i + prefix.len()..];
        let n = len(after);
        if n == 0 {
            out.push_str(prefix);
            rest = after;
        } else {
            out.push_str(with);
            rest = &after[n..];
        }
    }
    out.push_str(rest);
    out
}

fn compare(fpm: &Server, rphp: &Server, root: &Path, name: &str, req: &Req, failures: &mut Vec<String>) {
    let p = params(root, req);
    let a = exchange(fpm, &p, &req.body);
    let b = exchange(rphp, &p, &req.body);
    let render = |r: &Reply| format!("{}\n--- stderr ---\n{}\n--- app status {:?}", normalize(&r.stdout), normalize(&r.stderr), r.app_status);
    let (a, b) = (render(&a), render(&b));
    if a == b {
        println!("ok   {name}");
        return;
    }
    let mut diff = String::new();
    for (i, (l, r)) in a.lines().zip(b.lines()).enumerate() {
        if l != r {
            diff = format!("first difference at line {}:\n  php-fpm: {}\n  rphp:    {}", i + 1, l, r);
            break;
        }
    }
    if diff.is_empty() {
        diff = format!("php-fpm has {} lines, rphp {}", a.lines().count(), b.lines().count());
    }
    println!("FAIL {name}\n{diff}");
    failures.push(name.to_string());
}

#[test]
fn responses_match_php_fpm() {
    let Some(fpm) = find_php_fpm() else {
        eprintln!("skipped: no php-fpm found (set PHP_FPM_BIN)");
        return;
    };
    let root = docroot();
    let fpm_srv = start_fpm(&fpm);
    let rphp_srv = start_rphp();
    let mut failures = Vec::new();
    let cases: Vec<(&str, Req)> = vec![
        ("GET index.php", with(get("/index.php"), "Cookie", "s=1; t=two"), ),
        ("query", get("/index.php?a=1&b[]=2&b[]=3&c[x]=y&d.e=1&f%20g=2&h=%41%zz&i&j[=1&k]=2&l[a][b]=3&m[]=1&m[x]=2&+n+=1")),
        ("cookies", with(get("/index.php"), "Cookie", "s=1; t=two; s=3; a[b]=c; d%20e=f%20g; =x; noval; p+q=r+s%2B; e.f=1; g[]=1; g[]=2")),
        ("missing script", get("/nonexistent.php")),
        ("directory", get("/dir")),
        ("sub page", get("/sub/page.php")),
        ("PATH_INFO", get("/sub/page.php/extra/path?q=1")),
        ("headers", get("/headers.php")),
        ("redirect", get("/redirect.php")),
        ("fatal", get("/fatal.php")),
        ("exit", get("/exit.php")),
        ("exit with message", get("/exit2.php")),
        ("streamed body", get("/big.php")),
        ("duplicate headers", get("/h2.php")),
        ("status line", get("/h3.php")),
        ("headers after buffered output", get("/h4.php")),
        ("flush sends the head", get("/hs.php")),
        ("headers_sent after output", get("/hs2.php")),
        ("notices", get("/hs3.php")),
        ("flush then header", get("/hs4.php")),
        ("setcookie", get("/cookies.php")),
        ("session", get("/sess.php")),
        ("POST form", form("POST", "/post.php?q=1", "a=1&b[]=2&c[k]=v")),
        ("POST multipart", multipart("/post.php")),
        ("POST json", Req { method: "POST", target: "/post.php", headers: vec![("Content-Type".to_string(), "application/json".to_string())], body: b"{\"x\":1}".to_vec() }),
        ("PUT form", form("PUT", "/post.php", "a=1")),
        ("HEAD script", Req { method: "HEAD", target: "/index.php", headers: Vec::new(), body: Vec::new() }),
        ("basic auth", with(get("/auth.php"), "Authorization", "Basic dXNlcjpwYTpzcw==")),
        ("digest auth", with(get("/auth.php"), "Authorization", "Digest username=\"u\", realm=\"r\"")),
        ("warnings", get("/warn.php")),
        ("no output", get("/nooutput.php")),
        ("304", get("/notmodified.php")),
        ("status via header()", get("/status.php")),
        ("content types", get("/ct.php")),
        ("ob_flush + flush", get("/flush.php")),
        ("error_log and finish_request", get("/fcgi.php")),
        ("status line naming 200", get("/status200.php")),
    ];
    for (name, req) in &cases {
        compare(&fpm_srv, &rphp_srv, &root, name, req, &mut failures);
    }
    assert!(failures.is_empty(), "responses differ from php-fpm: {failures:?}");
}
