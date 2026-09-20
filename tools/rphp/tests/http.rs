//! The built-in web server against `php -S` (SAPI-3, ADR-033): both serve
//! `examples/http/docroot` on ephemeral ports, every request below is sent
//! to each, and the raw responses — status line, headers, body — must match
//! byte for byte once the port, the `Date` header and the few values no
//! engine decides (session ids, upload temp names, `Max-Age` clock reads)
//! are replaced by placeholders.
//!
//! **Skipped** (not failed) when no php is found (`PHP_BIN` or `php` on
//! `PATH`).

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use rphp_test::differential::find_php;

/// The binary under test.
const RPHP: &str = env!("CARGO_BIN_EXE_rphp");

/// The ini both servers run under: `php -S`'s own defaults (`html_errors`
/// on, no output buffering) with the oracle's pins for what the tests
/// print.
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

/// A running server on a port of its own.
struct Server {
    child: Child,
    port: u16,
    name: &'static str,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn start(name: &'static str, bin: &Path, docroot: &Path, router: Option<&Path>) -> Server {
    let port = free_port();
    let mut cmd = Command::new(bin);
    if name == "php" {
        cmd.arg("-n");
    }
    for (k, v) in INI {
        cmd.arg("-d").arg(format!("{k}={v}"));
    }
    cmd.arg("-S").arg(format!("127.0.0.1:{port}")).arg("-t").arg(docroot);
    if let Some(r) = router {
        cmd.arg(r);
    }
    cmd.current_dir(docroot)
        .env("TZ", "UTC")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = cmd.spawn().unwrap_or_else(|e| panic!("cannot start {name} ({}): {e}", bin.display()));
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
    Server { child, port, name }
}

/// One request: the raw head after the request line (extra headers) and
/// the body.
struct Req {
    method: &'static str,
    target: &'static str,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

fn get(target: &'static str) -> Req {
    Req {
        method: "GET",
        target,
        headers: Vec::new(),
        body: Vec::new(),
    }
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
         --{b}\r\nContent-Disposition: form-data; name=\"g[]\"; filename=\"dir/up.txt\"\r\nContent-Type: text/plain\r\n\r\nfile content\r\n\
         --{b}--\r\n"
    );
    Req {
        method: "POST",
        target,
        headers: vec![("Content-Type".to_string(), format!("multipart/form-data; boundary={b}"))],
        body: body.into_bytes(),
    }
}

fn exchange(server: &Server, req: &Req, version: &str) -> Vec<u8> {
    let mut s = TcpStream::connect(("127.0.0.1", server.port))
        .unwrap_or_else(|e| panic!("connect to {}: {e}", server.name));
    s.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
    let mut head = format!(
        "{} {} {version}\r\nHost: 127.0.0.1:{}\r\nUser-Agent: rphp-http-test\r\nAccept: */*\r\n",
        req.method, req.target, server.port
    );
    for (k, v) in &req.headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    if !req.body.is_empty() {
        head.push_str(&format!("Content-Length: {}\r\n", req.body.len()));
    }
    head.push_str("\r\n");
    s.write_all(head.as_bytes()).unwrap();
    s.write_all(&req.body).unwrap();
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    out
}

/// The placeholders: port, `Date`, session ids, upload temp names, the
/// clock-relative `Max-Age`, and the object ids `var_dump` may print.
fn normalize(raw: &[u8], port: u16) -> String {
    let text = String::from_utf8_lossy(raw).into_owned();
    let text = text.replace(&format!("127.0.0.1:{port}"), "127.0.0.1:%PORT%");
    let text = text.replace(&format!("127.0.0.1%3A{port}"), "127.0.0.1%3A%PORT%");
    let text = text.replace(&format!("'SERVER_PORT' => '{port}'"), "'SERVER_PORT' => '%PORT%'");
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        if line.len() > 5 && line[..5].eq_ignore_ascii_case("date:") {
            out.push_str("Date: %DATE%\r\n");
        } else {
            out.push_str(line);
        }
    }
    let out = regex_replace(&out, "PHPSESSID=", |rest| rest.chars().take_while(|c| c.is_ascii_alphanumeric()).count(), "PHPSESSID=%SID%");
    let out = regex_replace(&out, "/T/php", |rest| rest.chars().take_while(|c| c.is_ascii_alphanumeric()).count(), "/T/php%TMP%");
    let out = regex_replace(&out, "Max-Age=", |rest| rest.chars().take_while(|c| c.is_ascii_digit()).count(), "Max-Age=%N%");
    regex_replace(&out, "#", |rest| {
        let n = rest.chars().take_while(|c| c.is_ascii_digit()).count();
        // Only `object(...)#N` / `#N (` — a `#` followed by digits and a space.
        if n > 0 && rest[n..].starts_with(' ') {
            n
        } else {
            0
        }
    }, "#%ID%")
}

/// Replace every `prefix<span>` where `span` is decided by `len(rest)`.
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

fn compare(php: &Server, rphp: &Server, name: &str, req: &Req, version: &str, failures: &mut Vec<String>) {
    let a = normalize(&exchange(php, req, version), php.port);
    let b = normalize(&exchange(rphp, req, version), rphp.port);
    if a == b {
        println!("ok   {name}");
        return;
    }
    let mut diff = String::new();
    for (i, (l, r)) in a.lines().zip(b.lines()).enumerate() {
        if l != r {
            diff = format!("first difference at line {}:\n  php:  {}\n  rphp: {}", i + 1, l, r);
            break;
        }
    }
    if diff.is_empty() {
        diff = format!("php has {} lines, rphp {}", a.lines().count(), b.lines().count());
    }
    println!("FAIL {name}\n{diff}");
    failures.push(name.to_string());
}

#[test]
fn responses_match_php_s() {
    let Some(php) = find_php() else {
        eprintln!("skipped: no php found (set PHP_BIN or put php on PATH)");
        return;
    };
    let root = docroot();
    let php_srv = start("php", &php, &root, None);
    let rphp_srv = start("rphp", Path::new(RPHP), &root, None);
    let mut failures = Vec::new();
    let cases: Vec<(&str, Req, &str)> = vec![
        ("GET /", with(get("/"), "Cookie", "s=1; t=two"), "HTTP/1.1"),
        (
            "GET / with a query",
            get("/?a=1&b[]=2&b[]=3&c[x]=y&d.e=1&f%20g=2&h=%41%zz&i&j[=1&k]=2&l[a][b]=3&m[]=1&m[x]=2&+n+=1"),
            "HTTP/1.1",
        ),
        (
            "cookies",
            with(get("/index.php"), "Cookie", "s=1; t=two; s=3; a[b]=c; d%20e=f%20g; =x; noval; p+q=r+s%2B; e.f=1; g[]=1; g[]=2"),
            "HTTP/1.1",
        ),
        ("GET /nonexistent", get("/nonexistent"), "HTTP/1.1"),
        ("GET /nonexistent/", get("/nonexistent/"), "HTTP/1.1"),
        ("GET /sub/page.php", get("/sub/page.php"), "HTTP/1.1"),
        ("PATH_INFO", get("/sub/page.php/extra/path?q=1"), "HTTP/1.1"),
        ("static text", get("/static.txt"), "HTTP/1.1"),
        ("directory index.html", get("/dir"), "HTTP/1.1"),
        ("directory index.html slash", get("/dir/"), "HTTP/1.1"),
        ("directory index.php", get("/dir3"), "HTTP/1.1"),
        ("directory index.php slash", get("/dir3/"), "HTTP/1.1"),
        ("static png", get("/img.png"), "HTTP/1.1"),
        ("headers", get("/headers.php"), "HTTP/1.1"),
        ("redirect", get("/redirect.php"), "HTTP/1.1"),
        ("fatal", get("/fatal.php"), "HTTP/1.1"),
        ("exit", get("/exit.php"), "HTTP/1.1"),
        ("exit with message", get("/exit2.php"), "HTTP/1.1"),
        ("streamed body", get("/big.php"), "HTTP/1.1"),
        ("404 directory", get("/sub"), "HTTP/1.1"),
        ("404 directory slash", get("/sub/"), "HTTP/1.1"),
        ("deep PATH_INFO", get("/nonexistent/deeper/x.php"), "HTTP/1.1"),
        ("index.php PATH_INFO", get("/index.php/foo"), "HTTP/1.1"),
        ("bad query", get("/index.php?%zz=1"), "HTTP/1.1"),
        ("duplicate headers", get("/h2.php"), "HTTP/1.1"),
        ("status line", get("/h3.php"), "HTTP/1.1"),
        ("headers after buffered output", get("/h4.php"), "HTTP/1.1"),
        ("flush sends the head", get("/hs.php"), "HTTP/1.1"),
        ("headers_sent after output", get("/hs2.php"), "HTTP/1.1"),
        ("notices", get("/hs3.php"), "HTTP/1.1"),
        ("flush then header", get("/hs4.php"), "HTTP/1.1"),
        ("setcookie", get("/cookies.php"), "HTTP/1.1"),
        ("session", get("/sess.php"), "HTTP/1.1"),
        ("POST form", form("POST", "/post.php?q=1", "a=1&b[]=2&c[k]=v"), "HTTP/1.1"),
        ("POST multipart", multipart("/post.php"), "HTTP/1.1"),
        (
            "POST json",
            Req {
                method: "POST",
                target: "/post.php",
                headers: vec![("Content-Type".to_string(), "application/json".to_string())],
                body: b"{\"x\":1}".to_vec(),
            },
            "HTTP/1.1",
        ),
        ("PUT form", form("PUT", "/post.php", "a=1"), "HTTP/1.1"),
        ("PATCH form", form("PATCH", "/post.php", "a=1"), "HTTP/1.1"),
        ("HEAD script", Req { method: "HEAD", target: "/index.php", headers: Vec::new(), body: Vec::new() }, "HTTP/1.1"),
        ("HEAD static", Req { method: "HEAD", target: "/static.txt", headers: Vec::new(), body: Vec::new() }, "HTTP/1.1"),
        ("HEAD 404", Req { method: "HEAD", target: "/sub/", headers: Vec::new(), body: Vec::new() }, "HTTP/1.1"),
        ("HTTP/1.0", get("/sub/page.php"), "HTTP/1.0"),
        ("OPTIONS", Req { method: "OPTIONS", target: "/sub/page.php", headers: Vec::new(), body: Vec::new() }, "HTTP/1.1"),
        ("DELETE static", Req { method: "DELETE", target: "/static.txt", headers: Vec::new(), body: Vec::new() }, "HTTP/1.1"),
        ("PUT static", Req { method: "PUT", target: "/static.txt", headers: Vec::new(), body: Vec::new() }, "HTTP/1.1"),
        ("POST static", form("POST", "/static.txt", "x=1"), "HTTP/1.1"),
        ("SEARCH", Req { method: "SEARCH", target: "/sub/page.php", headers: Vec::new(), body: Vec::new() }, "HTTP/1.1"),
        ("basic auth", with(get("/auth.php"), "Authorization", "Basic dXNlcjpwYTpzcw=="), "HTTP/1.1"),
        ("digest auth", with(get("/auth.php"), "Authorization", "Digest username=\"u\", realm=\"r\""), "HTTP/1.1"),
        ("encoded path", get("/%73ub/page.php"), "HTTP/1.1"),
        ("dot segments", get("/a%20b/../sub/page.php?x=%20"), "HTTP/1.1"),
        ("404 escapes the uri", get("/notfound/<b>&\"x"), "HTTP/1.1"),
        ("repeated request headers", with(with(get("/index.php"), "X-Custom-Header", "v1"), "X-Custom-Header", "v2"), "HTTP/1.1"),
        ("warnings", get("/warn.php"), "HTTP/1.1"),
        ("no output", get("/nooutput.php"), "HTTP/1.1"),
        ("304", get("/notmodified.php"), "HTTP/1.1"),
        ("status via header()", get("/status.php"), "HTTP/1.1"),
        ("content types", get("/ct.php"), "HTTP/1.1"),
        ("ob_flush + flush", get("/flush.php"), "HTTP/1.1"),
    ];
    for (name, req, version) in &cases {
        compare(&php_srv, &rphp_srv, name, req, version, &mut failures);
    }
    assert!(failures.is_empty(), "responses differ from php -S: {failures:?}");
}

#[test]
fn router_script_matches_php_s() {
    let Some(php) = find_php() else {
        eprintln!("skipped: no php found (set PHP_BIN or put php on PATH)");
        return;
    };
    let root = docroot();
    let router = root.parent().unwrap().join("router.php");
    let php_srv = start("php", &php, &root, Some(&router));
    let rphp_srv = start("rphp", Path::new(RPHP), &root, Some(&router));
    let mut failures = Vec::new();
    let cases: Vec<(&str, Req)> = vec![
        ("router: /", get("/")),
        ("router: script", get("/sub/page.php?x=1")),
        ("router: declined static", get("/static.txt")),
        ("router: declined missing", get("/static-missing.txt")),
        ("router: POST", form("POST", "/anything", "a=1")),
    ];
    for (name, req) in &cases {
        compare(&php_srv, &rphp_srv, name, req, "HTTP/1.1", &mut failures);
    }
    assert!(failures.is_empty(), "responses differ from php -S: {failures:?}");
}
