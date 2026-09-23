//! The `ssl`/`tls` transports and `https://` against a real TLS server
//! (S12): a `rustls` server on loopback presents the self-signed fixture in
//! `fixtures/tls`, and the same script is run under php and under rphp and
//! required to match byte for byte.
//!
//! Standing the server up here rather than in php keeps the test hermetic —
//! it reaches no further than loopback — and keeps it a *differential* test,
//! since php trusts the fixture through the same `cafile` option rphp does.
//!
//! **Skipped** (not failed) when no php is found (`PHP_BIN` or `php` on
//! `PATH`).

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;

use rphp_test::differential::find_php;

/// The binary under test.
const RPHP: &str = env!("CARGO_BIN_EXE_rphp");

/// The ini both sides run under — the oracle's pins, so a warning prints
/// the same way for each.
const INI: &[(&str, &str)] = &[
    ("display_errors", "1"),
    ("log_errors", "0"),
    ("error_reporting", "E_ALL"),
    ("html_errors", "0"),
    ("date.timezone", "UTC"),
    ("precision", "14"),
    ("serialize_precision", "-1"),
    ("short_open_tag", "0"),
    ("zend.assertions", "-1"),
];

fn fixture(name: &str) -> PathBuf {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/tls").join(name);
    p.canonicalize().unwrap_or_else(|e| panic!("{} should exist: {e}", p.display()))
}

/// The fixture certificate and key as rustls wants them.
fn server_config() -> Arc<rustls::ServerConfig> {
    let cert_pem = std::fs::read(fixture("cert.pem")).expect("cert.pem");
    let key_pem = std::fs::read(fixture("key.pem")).expect("key.pem");
    let certs: Vec<_> = rustls_pemfile::certs(&mut &cert_pem[..]).map(|c| c.expect("a certificate")).collect();
    let key = rustls_pemfile::private_key(&mut &key_pem[..]).expect("a readable key").expect("a key");
    Arc::new(
        rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .expect("the fixture is a usable certificate/key pair"),
    )
}

/// A TLS server on an ephemeral loopback port, answering every request with
/// one fixed response and nothing that varies between runs.
struct Server {
    port: u16,
    stop: Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Server {
    fn start() -> Server {
        let cfg = server_config();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().expect("a bound address").port();
        listener.set_nonblocking(true).expect("non-blocking listener");
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let thread = std::thread::spawn(move || {
            while !flag.load(std::sync::atomic::Ordering::Relaxed) {
                match listener.accept() {
                    Ok((sock, _)) => {
                        let cfg = Arc::clone(&cfg);
                        // One connection at a time is plenty, and keeps the
                        // ordering of the script's requests obvious.
                        std::thread::spawn(move || serve(sock, cfg));
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
        });
        Server { port, stop, thread: Some(thread) }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Handshake, read the request head, answer, close.
fn serve(sock: std::net::TcpStream, cfg: Arc<rustls::ServerConfig>) {
    sock.set_nonblocking(false).ok();
    sock.set_read_timeout(Some(std::time::Duration::from_secs(10))).ok();
    let Ok(conn) = rustls::ServerConnection::new(cfg) else { return };
    let mut tls = rustls::StreamOwned::new(conn, sock);
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
        match tls.read(&mut byte) {
            Ok(0) | Err(_) => return,
            Ok(_) => head.push(byte[0]),
        }
        if head.len() > 8192 {
            return;
        }
    }
    let body = "OVER TLS";
    let response = format!(
        "HTTP/1.1 200 OK\r\nX-Transport: tls\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = tls.write_all(response.as_bytes());
    let _ = tls.flush();
}

/// Run `script` under one engine and collect stdout+stderr.
fn run(engine: &std::path::Path, args: &[&str], script: &std::path::Path) -> String {
    let mut cmd = Command::new(engine);
    cmd.arg("-n");
    for (k, v) in INI {
        cmd.arg("-d").arg(format!("{k}={v}"));
    }
    // php's CLI is `php [options] script.php [args…]`, in that order.
    cmd.arg(script).args(args);
    let out = cmd.output().unwrap_or_else(|e| panic!("{} should run: {e}", engine.display()));
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    // The script's path appears in warnings; the port is chosen by the
    // kernel. Neither is the engine's doing.
    text.replace(&script.display().to_string(), "<script>")
}

#[test]
fn tls_transport_matches_stock_php() {
    let Some(php) = find_php() else {
        eprintln!("no php found — skipping the tls differential");
        return;
    };
    let server = Server::start();
    let ca = fixture("ca.pem");
    let dir = std::env::temp_dir().join(format!("rphp-tls-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let script = dir.join("tls-cases.php");
    std::fs::write(&script, CASES).expect("the script is written");

    let port = server.port.to_string();
    let args = [port.as_str(), ca.to_str().expect("a utf-8 path")];
    let from_php = run(&php, &args, &script);
    let from_rphp = run(std::path::Path::new(RPHP), &args, &script);

    let _ = std::fs::remove_file(&script);
    let _ = std::fs::remove_dir(&dir);

    assert_eq!(
        from_php, from_rphp,
        "\n--- php ---\n{from_php}\n--- rphp ---\n{from_rphp}\n"
    );
    // Guard against the test going quietly vacuous: a server that never
    // answered, or a refusal that stopped refusing, would still make the
    // two sides equal.
    for marker in [
        "OVER TLS",
        "untrusted refused: bool(true)",
        "mismatch refused: bool(true)",
        "ValueError: stream_socket_enable_crypto()",
    ] {
        assert!(
            from_php.contains(marker),
            "php's own run should show {marker:?}, so the case is really being exercised:\n{from_php}"
        );
    }
}

/// Be the client of a TLS server the script under test stands up: wait for
/// it to publish its port, then say `ping` and read one line back. A
/// handshake the server refuses is part of what is being tested, so it is
/// not an error here.
fn ping(port_file: &std::path::Path) -> Option<String> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let port = loop {
        if let Ok(p) = std::fs::read_to_string(port_file) {
            if let Ok(p) = p.trim().parse::<u16>() {
                break p;
            }
        }
        if std::time::Instant::now() > deadline {
            return None;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    let ca_pem = std::fs::read(fixture("ca.pem")).expect("ca.pem");
    let mut roots = rustls::RootCertStore::empty();
    for c in rustls_pemfile::certs(&mut &ca_pem[..]) {
        roots.add(c.expect("a certificate")).expect("the CA is usable");
    }
    let cfg = rustls::ClientConfig::builder().with_root_certificates(roots).with_no_client_auth();
    let name = rustls::pki_types::ServerName::try_from("localhost").expect("a name");
    let conn = rustls::ClientConnection::new(Arc::new(cfg), name).expect("a client");
    let sock = std::net::TcpStream::connect(("127.0.0.1", port)).ok()?;
    sock.set_read_timeout(Some(std::time::Duration::from_secs(10))).ok();
    let mut tls = rustls::StreamOwned::new(conn, sock);
    tls.write_all(b"ping\n").ok()?;
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    while !line.ends_with(b"\n") {
        match tls.read(&mut byte) {
            Ok(0) | Err(_) => break,
            Ok(_) => line.push(byte[0]),
        }
    }
    Some(String::from_utf8_lossy(&line).into_owned())
}

/// Run the server script under one engine in `mode`, with a client pinging
/// it, and collect what the server printed (and what the client heard).
fn run_server(engine: &std::path::Path, script: &std::path::Path, mode: &str, dir: &std::path::Path) -> String {
    let port_file = dir.join(format!("port-{mode}"));
    let _ = std::fs::remove_file(&port_file);
    let fixtures = fixture("cert.pem").parent().expect("the fixture dir").to_path_buf();
    let mut cmd = Command::new(engine);
    cmd.arg("-n");
    for (k, v) in INI {
        cmd.arg("-d").arg(format!("{k}={v}"));
    }
    cmd.arg(script).arg(mode).arg(&fixtures).arg(&port_file);
    cmd.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    let child = cmd.spawn().unwrap_or_else(|e| panic!("{} should run: {e}", engine.display()));
    let heard = ping(&port_file);
    let out = child.wait_with_output().expect("the server exits");
    let _ = std::fs::remove_file(&port_file);
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    text.push_str(&format!("client heard: {heard:?}\n"));
    text.replace(&script.display().to_string(), "<script>").replace(&fixtures.display().to_string(), "<fixtures>")
}

#[test]
fn tls_server_matches_stock_php() {
    let Some(php) = find_php() else {
        eprintln!("no php found — skipping the tls server differential");
        return;
    };
    let dir = std::env::temp_dir().join(format!("rphp-tls-server-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let script = dir.join("tls-server.php");
    std::fs::write(&script, SERVER).expect("the script is written");
    for mode in ["tls", "upgrade", "nocert", "badcert", "badkey"] {
        let from_php = run_server(&php, &script, mode, &dir);
        let from_rphp = run_server(std::path::Path::new(RPHP), &script, mode, &dir);
        assert_eq!(
            from_php, from_rphp,
            "\n=== {mode} ===\n--- php ---\n{from_php}\n--- rphp ---\n{from_rphp}\n"
        );
        if mode == "tls" || mode == "upgrade" {
            assert!(from_php.contains("Some(\"pong\\n\")"), "php's {mode} server should answer:\n{from_php}");
        }
    }
    let _ = std::fs::remove_file(&script);
    let _ = std::fs::remove_dir(&dir);
}

/// A one-connection TLS server: `$argv[1]` is the mode, `$argv[2]` the
/// fixture directory, `$argv[3]` where to publish the port.
const SERVER: &str = r#"<?php
[$mode, $dir, $portFile] = [$argv[1], $argv[2], $argv[3]];
$ssl = [
    'local_cert' => $mode === 'badcert' ? '/nonexistent/cert.pem' : "$dir/cert.pem",
    'local_pk' => $mode === 'badkey' ? '/nonexistent/key.pem' : "$dir/key.pem",
];
if ($mode === 'nocert') $ssl = [];
$ctx = stream_context_create(['ssl' => $ssl]);
$proto = $mode === 'upgrade' ? 'tcp' : 'tls';
$srv = stream_socket_server("$proto://127.0.0.1:0", $errno, $errstr,
    STREAM_SERVER_BIND | STREAM_SERVER_LISTEN, $ctx);
var_dump($srv !== false, $errno, $errstr);
$name = stream_socket_get_name($srv, false);
file_put_contents($portFile, substr($name, strrpos($name, ':') + 1));
$c = stream_socket_accept($srv, 10);
var_dump(is_resource($c));
if (!$c) exit;
if ($mode === 'upgrade') {
    var_dump(stream_socket_enable_crypto($c, true, STREAM_CRYPTO_METHOD_TLS_SERVER));
}
$m = stream_get_meta_data($c);
echo $m['stream_type'], "
";
var_dump($m['crypto']['protocol'] ?? null, array_keys($m['crypto'] ?? []));
echo 'server heard: ', fgets($c);
fwrite($c, "pong
");
fclose($c);
"#;

/// The cases, as a php script taking `$argv[1]` = port and `$argv[2]` = the
/// CA to trust.
const CASES: &str = r#"<?php
[$port, $cafile] = [$argv[1], $argv[2]];
$trusted = fn(array $extra = []) => stream_context_create(
    ['ssl' => ['cafile' => $cafile] + $extra]
);

echo "--- https with the fixture as its cafile ---\n";
var_dump(file_get_contents("https://localhost:$port/", false, $trusted()));
$h = http_get_last_response_headers();
var_dump($h[0], in_array('X-Transport: tls', $h, true));

echo "--- the stream it opened ---\n";
$s = fopen("https://localhost:$port/", 'r', false, $trusted());
$m = stream_get_meta_data($s);
var_dump($m['wrapper_type'], $m['stream_type'], $m['mode'], $m['seekable']);
var_dump($m['crypto']);
var_dump(stream_get_contents($s));
fclose($s);

echo "--- ssl:// straight to the transport ---\n";
$c = stream_socket_client("ssl://localhost:$port", $e, $es, 10,
    STREAM_CLIENT_CONNECT, $trusted());
var_dump($c !== false, $e, $es);
fwrite($c, "GET / HTTP/1.0\r\nHost: localhost\r\nConnection: close\r\n\r\n");
$body = stream_get_contents($c);
var_dump(substr($body, 0, 15), str_contains($body, 'OVER TLS'));
var_dump(stream_get_meta_data($c)['stream_type']);
fclose($c);

echo "--- STARTTLS: upgrading a plain socket in place ---\n";
$p = stream_socket_client("tcp://localhost:$port", $e2, $es2, 10,
    STREAM_CLIENT_CONNECT, $trusted());
var_dump($p !== false);
var_dump(stream_socket_enable_crypto($p, true, STREAM_CRYPTO_METHOD_TLS_CLIENT));
fwrite($p, "GET / HTTP/1.0\r\nHost: localhost\r\nConnection: close\r\n\r\n");
var_dump(str_contains(stream_get_contents($p), 'OVER TLS'));
fclose($p);

echo "--- an untrusted certificate is refused ---\n";
echo 'untrusted refused: '; var_dump(@file_get_contents("https://localhost:$port/") === false);

echo "--- the wrong name is refused, and allowed when asked ---\n";
// The leaf is good for `localhost` and nothing else, so the address form
// is a name mismatch even though it reaches the same server.
echo 'mismatch refused: ';
var_dump(@file_get_contents("https://127.0.0.1:$port/", false, $trusted()) === false);
var_dump(file_get_contents("https://127.0.0.1:$port/", false,
    $trusted(['verify_peer_name' => false])) === 'OVER TLS');

echo "--- verification off entirely ---\n";
$open = stream_context_create(['ssl' => ['verify_peer' => false, 'verify_peer_name' => false]]);
var_dump(file_get_contents("https://localhost:$port/", false, $open));

echo "--- crypto on a stream that has none ---\n";
$mem = fopen('php://memory', 'r+');
var_dump(@stream_socket_enable_crypto($mem, true, STREAM_CRYPTO_METHOD_TLS_CLIENT));
fclose($mem);
try {
    stream_socket_enable_crypto(fopen('php://memory', 'r+'), true);
} catch (ValueError $e) {
    echo get_class($e), ': ', $e->getMessage(), "\n";
}
"#;
