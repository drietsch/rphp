//! ext/standard's socket transports (php-src `ext/standard/streamsfuncs.c`
//! and `main/streams/transports.c`): the `stream_socket_*` family,
//! `fsockopen()`, and the host lookups beside them.
//!
//! php models a socket as an ordinary stream over a transport, and so does
//! this: everything here builds a [`Conn`] and hands it to
//! `file::socket_resource`, after which `fread`/`fwrite`/`fgets`/`feof`/
//! `stream_select`/`fclose` work on it through the same descriptor path as
//! a `proc_open` pipe. Nothing is buffered and the cursor means nothing.
//!
//! **Addresses.** php spells one `transport://host:port`, with `tcp` the
//! default when no transport is given, and `unix://`/`udg://` taking a path
//! instead. A missing port is not a default, it is the parse error
//! `Failed to parse address "…"`, and an unknown transport is its own error
//! naming the transport — both of which land in the `$error_message`
//! out-parameter rather than in an exception.
//!
//! **Timeouts.** A connect timeout is `connect(2)` bounded by the argument
//! (or `default_socket_timeout`); a *read* timeout is `SO_RCVTIMEO`, set by
//! `stream_set_timeout()`. php reports an expired read as `timed_out` in
//! `stream_get_meta_data()` with `eof` still false, which is what the read
//! path in `file.rs` records when a blocking descriptor comes back empty.
//!
//! **Encrypted transports.** `ssl://` and `tls://` (and the version-named
//! aliases) are tcp with a rustls session over it; `tls.rs` has the
//! handshake and the `ssl` context options. A TLS *server* is not
//! implemented, so binding one answers php's unknown-transport error rather
//! than accepting connections it could not finish.
//!
//! **Known divergence.** `pfsockopen()` opens an ordinary connection: php's
//! persistent list keeps one across requests within a worker, which nothing
//! here yet shares. The peer name of an accepted `unix://` connection is
//! `""`; php prints whatever its uninitialised `sockaddr_un` held.

use std::net::{TcpListener, TcpStream, ToSocketAddrs, UdpSocket};
use std::os::unix::net::{UnixDatagram, UnixListener, UnixStream};
use std::time::Duration;

use rphp_runtime::{nf, nf_ref, Ctx, NativeFn, NativeResult, Unwind};
use rphp_value::{Array, ArrayKey, Str, Value};

use crate::file::{socket_resource, with_socket, Conn, SockKind};

/// This extension's registry contribution (see `lib.rs`).
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf_ref!("stream_socket_client", 1, Some(6), 0b110, stream_socket_client),
    nf_ref!("stream_socket_server", 1, Some(5), 0b110, stream_socket_server),
    nf_ref!("stream_socket_accept", 1, Some(3), 0b100, stream_socket_accept),
    nf!("stream_socket_get_name", 2, Some(2), stream_socket_get_name),
    nf!("stream_socket_sendto", 2, Some(4), stream_socket_sendto),
    nf_ref!("stream_socket_recvfrom", 2, Some(4), 0b1000, stream_socket_recvfrom),
    nf!("stream_socket_shutdown", 2, Some(2), stream_socket_shutdown),
    nf!("stream_socket_enable_crypto", 2, Some(4), stream_socket_enable_crypto),
    nf!("stream_socket_pair", 3, Some(3), stream_socket_pair),
    nf_ref!("fsockopen", 1, Some(5), 0b1100, fsockopen),
    nf_ref!("pfsockopen", 1, Some(5), 0b1100, fsockopen),
    nf!("stream_set_timeout", 2, Some(3), stream_set_timeout),
    nf!("socket_set_timeout", 2, Some(3), stream_set_timeout),
    nf!("stream_get_transports", 0, Some(0), stream_get_transports),
    nf!("stream_get_wrappers", 0, Some(0), stream_get_wrappers),
    nf!("gethostname", 0, Some(0), gethostname),
    nf!("gethostbyname", 1, Some(1), gethostbyname),
    nf!("gethostbyaddr", 1, Some(1), gethostbyaddr),
    nf!("gethostbynamel", 1, Some(1), gethostbynamel),
];

// ---- addresses ---------------------------------------------------------------------

/// The transports this build registers, in the order
/// `stream_get_transports()` lists them.
const TRANSPORTS: &[&str] = &[
    "tcp", "udp", "unix", "udg", "ssl", "tls", "tlsv1.0", "tlsv1.1", "tlsv1.2", "tlsv1.3",
];

/// The wrappers this build registers, in the order `stream_get_wrappers()`
/// lists them. php's own list is longer (`https`, `ftp`, `phar`, `zip`,
/// `compress.*`); naming one here that `fopen()` cannot open would be worse
/// than leaving it out, since the whole point of the call is a feature test.
const WRAPPERS: &[&str] = &["php", "file", "glob", "data", "http", "https"];

/// Which transport an address named, and what it addresses.
enum Target {
    /// `tcp://host:port`
    Tcp(String, u16),
    /// `udp://host:port`
    Udp(String, u16),
    /// `unix:///path`
    Unix(String),
    /// `udg:///path`
    Udg(String),
    /// `ssl://host:port`, `tls://…`, `tlsv1.2://…` — tcp with a TLS
    /// session over it. rustls negotiates the version, so the four
    /// version-named transports differ from `tls` only in name.
    Tls(String, u16),
}

impl Target {
    fn kind(&self) -> SockKind {
        match self {
            Target::Tcp(..) => SockKind::Tcp,
            Target::Udp(..) => SockKind::Udp,
            Target::Unix(_) => SockKind::Unix,
            Target::Udg(_) => SockKind::UnixDgram,
            // php names the transport after the module that registered it,
            // and openssl registers over tcp — an encrypted socket reports
            // the same `tcp_socket/ssl` a plain one does.
            Target::Tls(..) => SockKind::Tcp,
        }
    }
}

/// Why an address could not be used, as php words it into `$error_message`.
enum AddrErr {
    /// A transport nothing registered. php's client path reports errno 1
    /// for this and its server path reports 0 — a difference in where each
    /// leaves the variable, kept because both are observable.
    Transport(String),
    /// A well-formed transport whose address php could not read, which for
    /// an internet transport means "no port".
    Parse(String),
}

impl AddrErr {
    fn text(&self) -> String {
        match self {
            AddrErr::Transport(t) => format!(
                "Unable to find the socket transport \"{t}\" - did you forget to enable it when you configured PHP?"
            ),
            AddrErr::Parse(a) => format!("Failed to parse address \"{a}\""),
        }
    }
}

/// php's `php_stream_xport_create` address grammar: an optional
/// `transport://` (`tcp` when absent), then either a path or `host:port`,
/// with a bracketed host for IPv6.
fn parse_address(addr: &str) -> Result<Target, AddrErr> {
    let (scheme, rest) = match addr.find("://") {
        Some(i) => (&addr[..i], &addr[i + 3..]),
        None => ("tcp", addr),
    };
    match scheme {
        "unix" => Ok(Target::Unix(rest.to_string())),
        "udg" => Ok(Target::Udg(rest.to_string())),
        "tcp" | "udp" | "ssl" | "tls" | "tlsv1.0" | "tlsv1.1" | "tlsv1.2" | "tlsv1.3" => {
            let (host, port) = split_host_port(rest).ok_or_else(|| AddrErr::Parse(rest.to_string()))?;
            Ok(match scheme {
                "tcp" => Target::Tcp(host, port),
                "udp" => Target::Udp(host, port),
                _ => Target::Tls(host, port),
            })
        }
        other => Err(AddrErr::Transport(other.to_string())),
    }
}

/// `host:port`, or `[v6addr]:port`. A missing or unreadable port is a
/// parse failure, never a default.
fn split_host_port(s: &str) -> Option<(String, u16)> {
    if let Some(end) = s.strip_prefix('[').and_then(|r| r.find(']').map(|i| i + 1)) {
        let host = &s[1..end - 1];
        let port = s[end..].strip_prefix(':')?;
        return Some((host.to_string(), port.parse().ok()?));
    }
    let (host, port) = s.rsplit_once(':')?;
    if host.is_empty() {
        return None;
    }
    Some((host.to_string(), port.parse().ok()?))
}

/// The address a *listening* socket binds, where php accepts a wildcard
/// host: `0.0.0.0` and `[::]` are already what `to_socket_addrs` wants.
fn resolve(host: &str, port: u16) -> std::io::Result<Vec<std::net::SocketAddr>> {
    // A bracketed IPv6 literal reaches here unbracketed.
    let addrs: Vec<_> = (host, port).to_socket_addrs()?.collect();
    if addrs.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no addresses",
        ));
    }
    Ok(addrs)
}

/// php's message for a name that does not resolve, which names the call it
/// came from rather than the errno.
fn getaddrinfo_text(host: &str) -> String {
    format!(
        "php_network_getaddresses: getaddrinfo for {host} failed: nodename nor servname provided, or not known"
    )
}

/// The platform's `strerror` text for an I/O error, which is what php shows
/// — taken from the error itself rather than a table, so the numbering
/// stays the platform's.
fn os_text(e: &std::io::Error) -> String {
    let s = e.to_string();
    match s.find(" (os error ") {
        Some(i) => s[..i].to_string(),
        None => s,
    }
}

/// The errno php reports beside that text; 0 when the failure never
/// reached a system call.
fn os_errno(e: &std::io::Error) -> i64 {
    e.raw_os_error().unwrap_or(0) as i64
}

/// `default_socket_timeout`, the wait php uses when a call takes none.
fn default_timeout(ctx: &mut Ctx) -> Duration {
    let secs = ctx
        .ini_get("default_socket_timeout")
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(60.0);
    duration_of(secs)
}

/// A php timeout in (possibly fractional) seconds as a `Duration`; php
/// treats a negative wait as "do not wait".
fn duration_of(secs: f64) -> Duration {
    if !secs.is_finite() || secs <= 0.0 {
        return Duration::ZERO;
    }
    Duration::from_secs_f64(secs.min(86_400.0))
}

// ---- connecting --------------------------------------------------------------------

/// Open a connection to `host:port` for the http wrapper — encrypted when
/// the url said `https`.
pub(crate) fn connect_for_wrapper(
    host: &str,
    port: u16,
    secure: bool,
    timeout: Duration,
    ssl: &crate::tls::SslOptions,
) -> Result<Conn, (i64, String)> {
    let target = if secure {
        Target::Tls(host.to_string(), port)
    } else {
        Target::Tcp(host.to_string(), port)
    };
    connect(&target, timeout, ssl)
}

/// `default_socket_timeout`, for a wrapper that was given no timeout.
pub(crate) fn wrapper_timeout(ctx: &mut Ctx, given: Option<f64>) -> Duration {
    match given {
        Some(secs) => duration_of(secs),
        None => default_timeout(ctx),
    }
}

/// The `ssl` section of a stream context, as php spells its options.
pub(crate) fn ssl_options_of(ctx: &mut Ctx, context: Option<&Value>) -> crate::tls::SslOptions {
    let mut o = crate::tls::SslOptions::default();
    let Some(v) = context else { return o };
    let Some(all) = crate::file::context_options_of(ctx, v) else { return o };
    let Some(ssl) = all.get(&ArrayKey::str(b"ssl")) else { return o };
    let Value::Array(ssl) = &*ssl.deref() else { return o };
    for (k, v) in ssl.iter() {
        let ArrayKey::Str(name) = k else { continue };
        let v = v.deref().into_owned();
        let text = || String::from_utf8_lossy(&v.to_php_bytes()).into_owned();
        match name.as_ref() {
            b"verify_peer" => o.verify_peer = v.to_bool(),
            b"verify_peer_name" => o.verify_peer_name = v.to_bool(),
            b"allow_self_signed" => o.allow_self_signed = v.to_bool(),
            b"cafile" => o.cafile = Some(text()),
            b"peer_name" => o.peer_name = Some(text()),
            b"SNI_enabled" => o.sni_enabled = v.to_bool(),
            _ => {}
        }
    }
    o
}

/// Open a client connection, as `(errno, error text)` when it fails.
fn connect(
    target: &Target,
    timeout: Duration,
    ssl: &crate::tls::SslOptions,
) -> Result<Conn, (i64, String)> {
    match target {
        Target::Tls(host, port) => {
            // The handshake happens now, so a certificate that will not do
            // is reported by the call that opened the stream.
            let plain = connect(&Target::Tcp(host.clone(), *port), timeout, ssl)?;
            let Conn::Tcp(sock) = plain else {
                return Err((0, "tcp connection expected".to_string()));
            };
            match crate::tls::handshake(sock, host, ssl) {
                Ok(t) => Ok(Conn::Tls(Box::new(t))),
                Err((_, text)) => Err((0, text)),
            }
        }
        Target::Tcp(host, port) => {
            let addrs = resolve(host, *port).map_err(|_| (0, getaddrinfo_text(host)))?;
            // php walks the resolved list and reports the last failure.
            let mut last = None;
            for a in addrs {
                let r = if timeout.is_zero() {
                    TcpStream::connect(a)
                } else {
                    TcpStream::connect_timeout(&a, timeout)
                };
                match r {
                    Ok(s) => return Ok(Conn::Tcp(s)),
                    Err(e) => last = Some(e),
                }
            }
            Err(last.map_or((0, "Connection failed".into()), |e| (os_errno(&e), os_text(&e))))
        }
        Target::Udp(host, port) => {
            let addrs = resolve(host, *port).map_err(|_| (0, getaddrinfo_text(host)))?;
            let peer = addrs[0];
            // The local end is bound in the peer's family, unspecified.
            let any = if peer.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" };
            let sock = UdpSocket::bind(any).map_err(|e| (os_errno(&e), os_text(&e)))?;
            sock.connect(peer).map_err(|e| (os_errno(&e), os_text(&e)))?;
            Ok(Conn::Udp(sock))
        }
        Target::Unix(path) => UnixStream::connect(path)
            .map(Conn::Unix)
            .map_err(|e| (os_errno(&e), os_text(&e))),
        Target::Udg(path) => {
            let sock = UnixDatagram::unbound().map_err(|e| (os_errno(&e), os_text(&e)))?;
            sock.connect(path).map_err(|e| (os_errno(&e), os_text(&e)))?;
            Ok(Conn::UnixDgram(sock))
        }
    }
}

/// Bind (and, for a stream transport, listen), as `(errno, error text)`
/// when it fails.
fn bind(target: &Target, listen: bool) -> Result<Conn, (i64, String)> {
    match target {
        Target::Tcp(host, port) => {
            let addrs = resolve(host, *port).map_err(|_| (0, getaddrinfo_text(host)))?;
            let l = TcpListener::bind(&addrs[..]).map_err(|e| (os_errno(&e), os_text(&e)))?;
            let _ = listen;
            Ok(Conn::TcpListen(l))
        }
        Target::Udp(host, port) => {
            let addrs = resolve(host, *port).map_err(|_| (0, getaddrinfo_text(host)))?;
            let s = UdpSocket::bind(&addrs[..]).map_err(|e| (os_errno(&e), os_text(&e)))?;
            Ok(Conn::Udp(s))
        }
        Target::Unix(path) => {
            if listen {
                UnixListener::bind(path)
                    .map(Conn::UnixListen)
                    .map_err(|e| (os_errno(&e), os_text(&e)))
            } else {
                UnixDatagram::bind(path)
                    .map(Conn::UnixDgram)
                    .map_err(|e| (os_errno(&e), os_text(&e)))
            }
        }
        Target::Udg(path) => UnixDatagram::bind(path)
            .map(Conn::UnixDgram)
            .map_err(|e| (os_errno(&e), os_text(&e))),
        // A TLS *server* needs a certificate to present; `local_cert` is
        // not implemented, so php's "transport not found" is the honest
        // answer rather than a socket that cannot complete a handshake.
        Target::Tls(..) => Err((0, AddrErr::Transport("tls".to_string()).text())),
    }
}

/// Write php's `$error_code`/`$error_message` out-parameters, which it
/// fills on success too (`0` and `""`).
fn set_err(args: &mut [Value], at: usize, code: i64, text: &str) {
    if at < args.len() {
        args[at] = Value::Int(code);
    }
    if at + 1 < args.len() {
        args[at + 1] = Value::string(text.as_bytes());
    }
}

// ---- the transport functions -------------------------------------------------------

/// `stream_socket_client(string $address, &$error_code, &$error_message, ?float $timeout, int $flags, ?StreamContext $context): resource|false`
fn stream_socket_client(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let addr = String::from_utf8_lossy(&args[0].to_php_bytes()).into_owned();
    let timeout = match args.get(3) {
        Some(Value::Null) | None => default_timeout(ctx),
        Some(v) => duration_of(v.to_float()),
    };
    let target = match parse_address(&addr) {
        Ok(t) => t,
        Err(e) => {
            // php's client path leaves errno at 1 for an unknown transport.
            let code = match e {
                AddrErr::Transport(_) => 1,
                AddrErr::Parse(_) => 0,
            };
            let text = e.text();
            set_err(args, 1, code, &text);
            ctx.warn(&format!("stream_socket_client(): Unable to connect to {addr} ({text})"))?;
            return Ok(Value::Bool(false));
        }
    };
    let ssl = ssl_options_of(ctx, args.get(5));
    match connect(&target, timeout, &ssl) {
        Ok(conn) => {
            set_err(args, 1, 0, "");
            let handle = socket_resource(ctx, conn, target.kind(), Some(addr));
            crate::file::set_socket_ssl(ctx, &handle, ssl);
            Ok(handle)
        }
        Err((code, text)) => {
            set_err(args, 1, code, &text);
            ctx.warn(&format!("stream_socket_client(): Unable to connect to {addr} ({text})"))?;
            Ok(Value::Bool(false))
        }
    }
}

/// `stream_socket_server(string $address, &$error_code, &$error_message, int $flags, ?StreamContext $context): resource|false`
///
/// `$flags` defaults to `STREAM_SERVER_BIND|STREAM_SERVER_LISTEN`; a
/// datagram transport is bound and never listened on.
fn stream_socket_server(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    const LISTEN: i64 = 8; // STREAM_SERVER_LISTEN
    let addr = String::from_utf8_lossy(&args[0].to_php_bytes()).into_owned();
    let flags = args.get(3).map_or(12, Value::to_int);
    let target = match parse_address(&addr) {
        Ok(t) => t,
        Err(e) => {
            let text = e.text();
            set_err(args, 1, 0, &text);
            ctx.warn(&format!("stream_socket_server(): Unable to connect to {addr} ({text})"))?;
            return Ok(Value::Bool(false));
        }
    };
    let stream_transport = matches!(target, Target::Tcp(..) | Target::Unix(_));
    match bind(&target, stream_transport && flags & LISTEN != 0) {
        Ok(conn) => {
            set_err(args, 1, 0, "");
            Ok(socket_resource(ctx, conn, target.kind(), Some(addr)))
        }
        Err((code, text)) => {
            set_err(args, 1, code, &text);
            ctx.warn(&format!("stream_socket_server(): Unable to connect to {addr} ({text})"))?;
            Ok(Value::Bool(false))
        }
    }
}

/// `stream_socket_accept(resource $socket, ?float $timeout, &$peer_name): resource|false`
fn stream_socket_accept(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let sock = args[0].clone();
    let timeout = match args.get(1) {
        Some(Value::Null) | None => default_timeout(ctx),
        Some(v) => duration_of(v.to_float()),
    };
    // Wait for the listener to have something, so the timeout is php's and
    // not the kernel's; then take it.
    let waited = with_socket(ctx, &sock, "stream_socket_accept", |conn, _| {
        if !poll_readable(conn, timeout) {
            return None;
        }
        Some(match conn {
            Conn::TcpListen(l) => l.accept().map(|(s, peer)| (Conn::Tcp(s), peer.to_string())),
            Conn::UnixListen(l) => l.accept().map(|(s, _)| (Conn::Unix(s), String::new())),
            // Readable, but not a listener: `accept(2)` says `EINVAL`.
            _ => Err(std::io::Error::from_raw_os_error(22)),
        })
    })?;
    match waited {
        Some(Some(Ok((conn, peer)))) => {
            if args.len() > 2 {
                args[2] = Value::string(peer.as_bytes());
            }
            Ok(socket_resource(ctx, conn, SockKind::Tcp, None))
        }
        Some(Some(Err(e))) => {
            ctx.warn(&format!("stream_socket_accept(): Accept failed: {}", os_text(&e)))?;
            Ok(Value::Bool(false))
        }
        // Nothing arrived in time — which is also what php says about a
        // connected socket, since it polls before it accepts.
        Some(None) => {
            ctx.warn("stream_socket_accept(): Accept failed: Operation timed out")?;
            Ok(Value::Bool(false))
        }
        // Not a socket at all: php has no errno for this and shows the
        // text its `strerror(0)` gives.
        None => {
            ctx.warn("stream_socket_accept(): Accept failed: Unknown error")?;
            Ok(Value::Bool(false))
        }
    }
}

/// `poll(2)` for readability, which is how a php-level accept timeout is
/// kept independent of the listening socket's own blocking mode.
fn poll_readable(conn: &Conn, timeout: Duration) -> bool {
    use rustix::event::{poll, PollFd, PollFlags};
    let Some(fd) = conn.fd() else { return false };
    let mut fds = [PollFd::from_borrowed_fd(fd, PollFlags::IN)];
    let spec = rustix::fs::Timespec {
        tv_sec: timeout.as_secs() as i64,
        tv_nsec: timeout.subsec_nanos() as i64,
    };
    loop {
        match poll(&mut fds, Some(&spec)) {
            Ok(0) => return false,
            Ok(_) => return true,
            Err(rustix::io::Errno::INTR) => continue,
            Err(_) => return false,
        }
    }
}

/// php's `PHP_STREAM_OPTION_CHECK_LIVENESS`, which is what makes `feof()`
/// on a socket answer before anything has read short.
///
/// A socket is at its end when the peer has gone, and that is not the same
/// as having nothing to read: php polls with no wait, and only when the
/// descriptor *is* readable does a one-byte `MSG_PEEK` say which of the two
/// it is — data waiting, or a closed peer. Nothing pending at all means the
/// stream is simply not finished yet.
pub(crate) fn socket_eof(conn: &Conn) -> bool {
    use rustix::net::{recv, RecvFlags};
    if !poll_readable(conn, Duration::ZERO) {
        return false;
    }
    let Some(fd) = conn.fd() else { return false };
    let mut buf = [0u8; 1];
    match recv(fd, &mut buf[..], RecvFlags::PEEK) {
        Ok((_, n)) => n == 0,
        Err(rustix::io::Errno::INTR) | Err(rustix::io::Errno::AGAIN) => false,
        Err(_) => true,
    }
}

/// `stream_socket_get_name(resource $socket, bool $remote): string|false`
fn stream_socket_get_name(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let sock = args[0].clone();
    let remote = args[1].to_bool();
    let name = with_socket(ctx, &sock, "stream_socket_get_name", |conn, _| {
        socket_name(conn, remote)
    })?;
    Ok(match name.flatten() {
        Some(n) => Value::string(n.as_bytes()),
        None => Value::Bool(false),
    })
}

/// The local or remote name of a socket, in php's `host:port` (or path)
/// spelling.
fn socket_name(conn: &Conn, remote: bool) -> Option<String> {
    fn path_of(a: &std::os::unix::net::SocketAddr) -> Option<String> {
        a.as_pathname().map(|p| p.to_string_lossy().into_owned())
    }
    match (conn, remote) {
        (Conn::Tcp(s), true) => s.peer_addr().ok().map(|a| a.to_string()),
        (Conn::Tcp(s), false) => s.local_addr().ok().map(|a| a.to_string()),
        (Conn::TcpListen(l), false) => l.local_addr().ok().map(|a| a.to_string()),
        (Conn::Udp(s), true) => s.peer_addr().ok().map(|a| a.to_string()),
        (Conn::Udp(s), false) => s.local_addr().ok().map(|a| a.to_string()),
        (Conn::Unix(s), true) => s.peer_addr().ok().and_then(|a| path_of(&a)),
        (Conn::Unix(s), false) => s.local_addr().ok().and_then(|a| path_of(&a)),
        (Conn::UnixListen(l), false) => l.local_addr().ok().and_then(|a| path_of(&a)),
        (Conn::UnixDgram(s), true) => s.peer_addr().ok().and_then(|a| path_of(&a)),
        (Conn::UnixDgram(s), false) => s.local_addr().ok().and_then(|a| path_of(&a)),
        (Conn::Tls(t), true) => t.socket().peer_addr().ok().map(|a| a.to_string()),
        (Conn::Tls(t), false) => t.socket().local_addr().ok().map(|a| a.to_string()),
        _ => None,
    }
}

/// `stream_socket_sendto(resource $socket, string $data, int $flags = 0, string $address = ""): int|false`
fn stream_socket_sendto(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let sock = args[0].clone();
    let data = args[1].to_php_bytes().to_vec();
    let to = args.get(3).map(|v| String::from_utf8_lossy(&v.to_php_bytes()).into_owned());
    let to = to.filter(|s| !s.is_empty());
    let sent = with_socket(ctx, &sock, "stream_socket_sendto", |conn, _| {
        send_to(conn, &data, to.as_deref())
    })?;
    Ok(match sent {
        Some(Ok(n)) => Value::Int(n as i64),
        Some(Err(_)) => Value::Bool(false),
        // php's `sendto` on a stream with no socket falls through its
        // transport layer and reports the `-1` the call would have.
        None => Value::Int(-1),
    })
}

/// One datagram (or one write on a connected stream), optionally to an
/// address other than the one the socket is connected to.
fn send_to(conn: &mut Conn, data: &[u8], to: Option<&str>) -> std::io::Result<usize> {
    use std::io::Write;
    match (conn, to) {
        (Conn::Udp(s), Some(a)) => {
            let target = parse_address(a)
                .ok()
                .and_then(|t| match t {
                    Target::Udp(h, p) | Target::Tcp(h, p) => resolve(&h, p).ok(),
                    _ => None,
                })
                .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
            s.send_to(data, target[0])
        }
        (Conn::Udp(s), None) => s.send(data),
        (Conn::UnixDgram(s), Some(a)) => s.send_to(data, a.strip_prefix("udg://").unwrap_or(a)),
        (Conn::UnixDgram(s), None) => s.send(data),
        (Conn::Tcp(s), _) => s.write(data),
        (Conn::Unix(s), _) => s.write(data),
        (Conn::File(f), _) => f.write(data),
        // An encrypted stream has no datagram to address: the bytes go
        // through the session like any other write.
        (c @ Conn::Tls(_), _) => c.write_all(data).map(|()| data.len()),
        (Conn::TcpListen(_) | Conn::UnixListen(_) | Conn::Taken, _) => {
            Err(std::io::Error::from(std::io::ErrorKind::InvalidInput))
        }
    }
}

/// `stream_socket_recvfrom(resource $socket, int $length, int $flags = 0, &$address): string|false`
fn stream_socket_recvfrom(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    const PEEK: i64 = 2;
    let sock = args[0].clone();
    let len = args[1].to_int();
    if len <= 0 {
        return Err(Unwind::value_error(
            "stream_socket_recvfrom(): Argument #2 ($length) must be greater than 0",
        ));
    }
    let peek = args.get(2).map_or(0, Value::to_int) & PEEK != 0;
    let got = with_socket(ctx, &sock, "stream_socket_recvfrom", |conn, _| {
        recv_from(conn, len as usize, peek)
    })?;
    match got {
        Some(Ok((data, from))) => {
            if args.len() > 3 {
                args[3] = Value::string(from.as_bytes());
            }
            Ok(Value::Str(Str::from_vec(data)))
        }
        _ => Ok(Value::Bool(false)),
    }
}

/// One receive, reporting the sender's address the way php spells it.
fn recv_from(conn: &mut Conn, len: usize, peek: bool) -> std::io::Result<(Vec<u8>, String)> {
    use std::io::Read;
    let mut buf = vec![0u8; len];
    // `MSG_PEEK` leaves the datagram queued; std has it only through
    // `peek`/`peek_from`, which is exactly what php's `STREAM_PEEK` is.
    let (n, from) = match conn {
        Conn::Udp(s) if peek => s.peek_from(&mut buf).map(|(n, a)| (n, a.to_string()))?,
        Conn::Udp(s) => s.recv_from(&mut buf).map(|(n, a)| (n, a.to_string()))?,
        Conn::UnixDgram(s) => {
            let (n, a) = s.recv_from(&mut buf)?;
            let name = a.as_pathname().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
            (n, name)
        }
        Conn::Tcp(s) => {
            let peer = s.peer_addr().map(|a| a.to_string()).unwrap_or_default();
            let n = if peek { s.peek(&mut buf)? } else { s.read(&mut buf)? };
            (n, peer)
        }
        Conn::Unix(s) => (s.read(&mut buf)?, String::new()),
        Conn::File(f) => (f.read(&mut buf)?, String::new()),
        // `MSG_PEEK` has no meaning through a TLS session — the bytes on
        // the socket are ciphertext — so a peek reads for real.
        Conn::Tls(t) => {
            let peer = t.socket().peer_addr().map(|a| a.to_string()).unwrap_or_default();
            (t.read_once(&mut buf)?, peer)
        }
        Conn::TcpListen(_) | Conn::UnixListen(_) | Conn::Taken => {
            return Err(std::io::Error::from(std::io::ErrorKind::InvalidInput))
        }
    };
    buf.truncate(n);
    Ok((buf, from))
}

/// `stream_socket_shutdown(resource $stream, int $mode): bool`
fn stream_socket_shutdown(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    use std::net::Shutdown;
    let sock = args[0].clone();
    let how = match args[1].to_int() {
        0 => Shutdown::Read,
        1 => Shutdown::Write,
        _ => Shutdown::Both,
    };
    let done = with_socket(ctx, &sock, "stream_socket_shutdown", |conn, _| match conn {
        Conn::Tcp(s) => s.shutdown(how).is_ok(),
        Conn::Unix(s) => s.shutdown(how).is_ok(),
        Conn::UnixDgram(s) => s.shutdown(how).is_ok(),
        Conn::Tls(t) => t.socket().shutdown(how).is_ok(),
        Conn::Udp(_) | Conn::TcpListen(_) | Conn::UnixListen(_) | Conn::File(_) | Conn::Taken => false,
    })?;
    Ok(Value::Bool(done.unwrap_or(false)))
}

/// `stream_socket_enable_crypto(resource $stream, bool $enable, ?int $method = null, ?resource $session = null): int|bool`
///
/// Turns a connected tcp stream into an encrypted one in place — what an
/// SMTP `STARTTLS` and php's own `https` wrapper do. The handshake runs to
/// completion here, so the answer is `true` or `false` and never php's `0`,
/// which only a non-blocking stream mid-handshake returns.
///
/// **Known divergence.** Turning crypto *off* on a stream that has it is
/// `false` here: a rustls session cannot be unwrapped back to the socket
/// underneath. (On a stream that never had it, `false` is also php's
/// answer.) `$method` is required, as php requires it, but its value is
/// ignored beyond that: rustls negotiates the version.
fn stream_socket_enable_crypto(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let stream = args[0].clone();
    let enable = args[1].to_bool();
    let method = args.get(2).filter(|v| !matches!(v, Value::Null));
    if enable && method.is_none() {
        return Err(Unwind::value_error(
            "stream_socket_enable_crypto(): Argument #3 ($crypto_method) must be specified when enabling encryption",
        ));
    }
    // A stream with no socket under it — a file, a memory buffer, a pipe —
    // and a socket that is not tcp are the same thing to php: it says so,
    // and then answers `false` for an enable and `true` for a disable,
    // since there was nothing to take off.
    let upgradable = with_socket(ctx, &stream, "stream_socket_enable_crypto", |conn, _| {
        matches!(conn, Conn::Tcp(_) | Conn::Tls(_))
    })?;
    if upgradable != Some(true) {
        ctx.warn("stream_socket_enable_crypto(): This stream does not support SSL/crypto")?;
        return Ok(Value::Bool(!enable));
    }
    if !enable {
        return Ok(Value::Bool(false));
    }
    // The name to verify against: php takes the `peer_name` option when
    // there is one, and the host the stream was opened to otherwise.
    let host = with_socket(ctx, &stream, "stream_socket_enable_crypto", |_, sock| {
        sock.ssl.peer_name.clone().or_else(|| {
            sock.uri.as_deref().and_then(|u| match parse_address(u) {
                Ok(Target::Tcp(h, _) | Target::Tls(h, _)) => Some(h),
                _ => None,
            })
        })
    })?;
    let Some(host) = host.flatten() else {
        ctx.warn("stream_socket_enable_crypto(): Unable to determine the peer name")?;
        return Ok(Value::Bool(false));
    };
    let outcome = with_socket(ctx, &stream, "stream_socket_enable_crypto", |conn, sock| {
        if matches!(conn, Conn::Tls(_)) {
            // Already encrypted: php does not handshake a second time.
            return Ok(());
        }
        // Taking the socket out and putting it back is the whole of the
        // upgrade; anything that is not a plain tcp socket goes back
        // untouched rather than leaving the stream without a descriptor.
        let socket = match std::mem::replace(conn, Conn::Taken) {
            Conn::Tcp(s) => s,
            other => {
                *conn = other;
                return Err("the stream is not a tcp socket".to_string());
            }
        };
        match crate::tls::handshake(socket, &host, &sock.ssl) {
            Ok(t) => {
                *conn = Conn::Tls(Box::new(t));
                Ok(())
            }
            Err((socket, text)) => {
                // php leaves the stream usable when the upgrade is refused.
                *conn = Conn::Tcp(socket);
                Err(text)
            }
        }
    })?;
    match outcome {
        Some(Ok(())) => Ok(Value::Bool(true)),
        Some(Err(text)) => {
            ctx.warn(&format!("stream_socket_enable_crypto(): {text}"))?;
            Ok(Value::Bool(false))
        }
        None => Ok(Value::Bool(false)),
    }
}

/// `stream_socket_pair(int $domain, int $type, int $protocol): array|false`
///
/// Only the unix-domain pair php can actually make on every platform; the
/// arguments are accepted and the datagram type honoured.
fn stream_socket_pair(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    const SOCK_DGRAM: i64 = 2;
    let dgram = args[1].to_int() == SOCK_DGRAM;
    let pair = if dgram {
        UnixDatagram::pair().map(|(a, b)| (Conn::UnixDgram(a), Conn::UnixDgram(b)))
    } else {
        UnixStream::pair().map(|(a, b)| (Conn::Unix(a), Conn::Unix(b)))
    };
    match pair {
        Ok((a, b)) => {
            // A pair has no address, so php's metadata carries no `uri`.
            let va = socket_resource(ctx, a, SockKind::Generic, None);
            let vb = socket_resource(ctx, b, SockKind::Generic, None);
            let mut out = Array::new();
            out.set(ArrayKey::Int(0), va);
            out.set(ArrayKey::Int(1), vb);
            Ok(Value::Array(out))
        }
        Err(e) => {
            ctx.warn(&format!("stream_socket_pair(): Failed to create sockets: {}", os_text(&e)))?;
            Ok(Value::Bool(false))
        }
    }
}

/// `fsockopen(string $hostname, int $port = -1, &$error_code, &$error_message, ?float $timeout): resource|false`
///
/// php's older spelling of `stream_socket_client`: the transport and host
/// come in one argument and the port in another, and the warning names the
/// pair rather than a URL.
fn fsockopen(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let host = String::from_utf8_lossy(&args[0].to_php_bytes()).into_owned();
    let port = args.get(1).map_or(-1, Value::to_int);
    let timeout = match args.get(4) {
        Some(Value::Null) | None => default_timeout(ctx),
        Some(v) => duration_of(v.to_float()),
    };
    // A host that already names a path transport keeps its own address;
    // anything else gets the port appended, as php's `fsockopen` does.
    let addr = if host.starts_with("unix://") || host.starts_with("udg://") {
        host.clone()
    } else if port >= 0 {
        format!("{host}:{port}")
    } else {
        host.clone()
    };
    let shown = if port >= 0 && !addr.starts_with("unix://") && !addr.starts_with("udg://") {
        format!("{host}:{port}")
    } else {
        host.clone()
    };
    let target = match parse_address(&addr) {
        Ok(t) => t,
        Err(e) => {
            let text = e.text();
            set_err(args, 2, 0, &text);
            ctx.warn(&format!("fsockopen(): Unable to connect to {shown} ({text})"))?;
            return Ok(Value::Bool(false));
        }
    };
    match connect(&target, timeout, &crate::tls::SslOptions::default()) {
        Ok(conn) => {
            set_err(args, 2, 0, "");
            Ok(socket_resource(ctx, conn, target.kind(), Some(addr)))
        }
        Err((code, text)) => {
            set_err(args, 2, code, &text);
            ctx.warn(&format!("fsockopen(): Unable to connect to {shown} ({text})"))?;
            Ok(Value::Bool(false))
        }
    }
}

/// `stream_set_timeout(resource $stream, int $seconds, int $microseconds = 0): bool`
///
/// php's read timeout, which is `SO_RCVTIMEO` on the descriptor. A wait of
/// zero means "do not wait", which is a very short timeout rather than no
/// timeout at all — `SO_RCVTIMEO` of zero would mean "block forever".
fn stream_set_timeout(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let sock = args[0].clone();
    let secs = args[1].to_int();
    let usecs = args.get(2).map_or(0, Value::to_int);
    let set = with_socket(ctx, &sock, "stream_set_timeout", |conn, meta| {
        let d = if secs < 0 {
            None
        } else {
            let total = Duration::from_secs(secs.max(0) as u64)
                + Duration::from_micros(usecs.max(0) as u64);
            Some(total.max(Duration::from_nanos(1)))
        };
        meta.timeout = d;
        set_read_timeout(conn, d)
    })?;
    Ok(Value::Bool(set.unwrap_or(false)))
}

/// `SO_RCVTIMEO` on a connection a wrapper owns.
pub(crate) fn set_read_timeout_on(conn: &Conn, d: Option<Duration>) {
    // A zero wait means "no timeout" to the kernel, which is the opposite
    // of what it means to php, so it is left alone.
    if d.is_some_and(|d| d.is_zero()) {
        return;
    }
    set_read_timeout(conn, d);
}

/// `SO_RCVTIMEO` on whichever socket this is.
fn set_read_timeout(conn: &Conn, d: Option<Duration>) -> bool {
    match conn {
        Conn::Tcp(s) => s.set_read_timeout(d).is_ok(),
        Conn::Udp(s) => s.set_read_timeout(d).is_ok(),
        Conn::Unix(s) => s.set_read_timeout(d).is_ok(),
        Conn::UnixDgram(s) => s.set_read_timeout(d).is_ok(),
        Conn::Tls(t) => t.socket().set_read_timeout(d).is_ok(),
        Conn::TcpListen(_) | Conn::UnixListen(_) | Conn::File(_) | Conn::Taken => false,
    }
}

/// `stream_get_transports(): array`
fn stream_get_transports(_ctx: &mut Ctx, _args: &mut [Value]) -> NativeResult {
    let mut out = Array::new();
    for (i, t) in TRANSPORTS.iter().enumerate() {
        out.set(ArrayKey::Int(i as i64), Value::string(t.as_bytes()));
    }
    Ok(Value::Array(out))
}

/// `stream_get_wrappers(): array`
fn stream_get_wrappers(_ctx: &mut Ctx, _args: &mut [Value]) -> NativeResult {
    let mut out = Array::new();
    for (i, w) in WRAPPERS.iter().enumerate() {
        out.set(ArrayKey::Int(i as i64), Value::string(w.as_bytes()));
    }
    Ok(Value::Array(out))
}

// ---- host lookups ------------------------------------------------------------------

/// `gethostname(): string|false`
fn gethostname(_ctx: &mut Ctx, _args: &mut [Value]) -> NativeResult {
    Ok(match hostname() {
        Some(h) => Value::string(h.as_bytes()),
        None => Value::Bool(false),
    })
}

/// The machine's name, which has no std API.
fn hostname() -> Option<String> {
    // `uname(3)`'s node name is what `gethostname(2)` answers with.
    rustix::system::uname()
        .nodename()
        .to_str()
        .ok()
        .map(str::to_string)
        .filter(|s| !s.is_empty())
}

/// `gethostbyname(string $hostname): string` — the first IPv4 address, or
/// the name back unchanged when it does not resolve, as php does.
fn gethostbyname(_ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let name = String::from_utf8_lossy(&args[0].to_php_bytes()).into_owned();
    let first = lookup(&name).into_iter().find(|a| a.contains('.'));
    Ok(Value::string(first.unwrap_or(name).as_bytes()))
}

/// `gethostbynamel(string $hostname): array|false` — every IPv4 address.
fn gethostbynamel(_ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let name = String::from_utf8_lossy(&args[0].to_php_bytes()).into_owned();
    let v4: Vec<String> = lookup(&name).into_iter().filter(|a| a.contains('.')).collect();
    if v4.is_empty() {
        return Ok(Value::Bool(false));
    }
    let mut out = Array::new();
    for (i, a) in v4.iter().enumerate() {
        out.set(ArrayKey::Int(i as i64), Value::string(a.as_bytes()));
    }
    Ok(Value::Array(out))
}

/// `gethostbyaddr(string $ip): string|false` — php answers the address
/// back when there is no reverse record, and `false` only for one that is
/// not an address at all.
///
/// **Known divergence.** The resolver's `PTR` lookup has no std API, so
/// this reads the hosts file, which is where the system resolver looks
/// first and where the answers that matter (`127.0.0.1` → `localhost`)
/// live. An address named only in the DNS comes back as itself, which is
/// also php's answer when a reverse record is missing.
fn gethostbyaddr(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let ip = String::from_utf8_lossy(&args[0].to_php_bytes()).into_owned();
    let Ok(addr) = ip.parse::<std::net::IpAddr>() else {
        ctx.warn("gethostbyaddr(): Address is not a valid IPv4 or IPv6 address")?;
        return Ok(Value::Bool(false));
    };
    Ok(Value::string(hosts_file_name(addr).unwrap_or(ip).as_bytes()))
}

/// The first name the hosts file gives an address. Its lines are
/// `<address> <name> [alias…]`, with `#` starting a comment.
fn hosts_file_name(addr: std::net::IpAddr) -> Option<String> {
    let text = std::fs::read_to_string("/etc/hosts").ok()?;
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("");
        let mut parts = line.split_whitespace();
        let Some(first) = parts.next() else { continue };
        if first.parse::<std::net::IpAddr>() != Ok(addr) {
            continue;
        }
        if let Some(name) = parts.next() {
            return Some(name.to_string());
        }
    }
    None
}

/// Every address a name resolves to, as text.
fn lookup(name: &str) -> Vec<String> {
    match (name, 0u16).to_socket_addrs() {
        Ok(it) => it.map(|a| a.ip().to_string()).collect(),
        Err(_) => Vec::new(),
    }
}
