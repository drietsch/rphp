//! The `ssl`/`tls` transports (php-src `ext/openssl/xp_ssl.c`): what turns
//! a connected tcp socket into an encrypted one, and what `https://` is
//! built on.
//!
//! php puts these under ext/openssl, which is why its `ssl` stream-context
//! options are spelled the way they are. The protocol here is `rustls` and
//! the crypto is `ring`; the trust store is the platform's, which is the
//! one php's openssl reads, so a certificate the system trusts is one this
//! trusts.
//!
//! **Verification.** php's defaults are `verify_peer => true` and
//! `verify_peer_name => true`, and both are honoured. Turning either off,
//! or setting `allow_self_signed`, installs a verifier that skips the
//! corresponding check — which is what php does, and why the snippet that
//! talks to its own self-signed server can do so.
//!
//! **The server side.** A `tls://` (or `ssl://`) listener hands out
//! connections whose handshake has already run, and a plain tcp connection
//! can be upgraded with `stream_socket_enable_crypto()` and a `*_SERVER`
//! method — both present the certificate named by `local_cert`, with the
//! key from `local_pk` or, when that is unset, from the same file, as
//! php's openssl does. A listener without `local_cert` still accepts, and
//! then refuses the handshake with the alert and the warning php gives.
//!
//! **Known divergence.** `capture_peer_cert`, `capture_peer_cert_chain`,
//! `ciphers`, `security_level`, `passphrase` (an encrypted key) and the
//! `crypto_method` narrowing to a single protocol version are accepted and
//! ignored: rustls picks TLS 1.2/1.3 and its own cipher list, and there is
//! no `OpenSSLCertificate` for a captured chain to be. `peer_fingerprint`
//! is not checked, and `SNI_enabled => false` does not suppress the name
//! in the hello. A failed handshake's warning carries rustls's reason, not
//! openssl's `error:0A000…` queue.

use std::io::{Read, Write};
use std::sync::{Arc, OnceLock};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{
    ClientConfig, ClientConnection, ConnectionCommon, DigitallySignedStruct, ServerConfig,
    ServerConnection, SideData, SignatureScheme, StreamOwned,
};

/// A tcp connection with a TLS session over it, from either end.
pub(crate) struct TlsConn {
    stream: Side,
}

/// Which end of the session this is: rustls types the two apart.
enum Side {
    Client(StreamOwned<ClientConnection, std::net::TcpStream>),
    Server(StreamOwned<ServerConnection, std::net::TcpStream>),
}

/// Run `$body` over whichever session `$side` holds, bound as `$s`.
macro_rules! either {
    ($side:expr, $s:ident => $body:expr) => {
        match $side {
            Side::Client($s) => $body,
            Side::Server($s) => $body,
        }
    };
}

impl TlsConn {
    /// The descriptor underneath, for `poll(2)` and `fcntl(2)`.
    ///
    /// Polling the socket is what php does too: a TLS stream's readiness is
    /// its socket's, give or take plaintext rustls has already buffered.
    pub(crate) fn as_fd(&self) -> std::os::fd::BorrowedFd<'_> {
        use std::os::fd::AsFd;
        self.socket().as_fd()
    }

    /// The socket, for the name and shutdown calls that are the transport's
    /// rather than the session's.
    pub(crate) fn socket(&self) -> &std::net::TcpStream {
        either!(&self.stream, s => &s.sock)
    }

    pub(crate) fn read_once(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match either!(&mut self.stream, s => s.read(buf)) {
            // rustls reports a peer that closed without `close_notify` as
            // this; php's openssl transport treats it as end of stream, and
            // so must this or every `file_get_contents()` of a server that
            // just closes would fail.
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => Ok(0),
            other => other,
        }
    }

    pub(crate) fn write_all(&mut self, data: &[u8]) -> std::io::Result<()> {
        either!(&mut self.stream, s => {
            s.write_all(data)?;
            s.flush()
        })
    }

    /// What php's `stream_get_meta_data()` reports under `crypto`: the
    /// protocol, and the cipher by openssl's name for it.
    pub(crate) fn crypto_meta(&self) -> Option<(&'static str, &'static str, i64)> {
        let (version, suite) = either!(&self.stream, s => {
            (s.conn.protocol_version()?, s.conn.negotiated_cipher_suite()?)
        });
        let protocol = match version {
            rustls::ProtocolVersion::TLSv1_3 => "TLSv1.3",
            rustls::ProtocolVersion::TLSv1_2 => "TLSv1.2",
            _ => return None,
        };
        use rustls::CipherSuite as C;
        let (name, bits) = match suite.suite() {
            C::TLS13_AES_256_GCM_SHA384 => ("TLS_AES_256_GCM_SHA384", 256),
            C::TLS13_AES_128_GCM_SHA256 => ("TLS_AES_128_GCM_SHA256", 128),
            C::TLS13_CHACHA20_POLY1305_SHA256 => ("TLS_CHACHA20_POLY1305_SHA256", 256),
            C::TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384 => ("ECDHE-RSA-AES256-GCM-SHA384", 256),
            C::TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256 => ("ECDHE-RSA-AES128-GCM-SHA256", 128),
            C::TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256 => ("ECDHE-RSA-CHACHA20-POLY1305", 256),
            C::TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384 => ("ECDHE-ECDSA-AES256-GCM-SHA384", 256),
            C::TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256 => ("ECDHE-ECDSA-AES128-GCM-SHA256", 128),
            C::TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256 => {
                ("ECDHE-ECDSA-CHACHA20-POLY1305", 256)
            }
            _ => return None,
        };
        Some((protocol, name, bits))
    }
}

/// php's `ssl` stream-context options, as far as they are honoured.
#[derive(Clone)]
pub(crate) struct SslOptions {
    pub(crate) verify_peer: bool,
    pub(crate) verify_peer_name: bool,
    pub(crate) allow_self_signed: bool,
    /// A PEM bundle to trust *instead of* the platform store.
    pub(crate) cafile: Option<String>,
    /// The name to verify against and to send as SNI, when it is not the
    /// host from the address.
    pub(crate) peer_name: Option<String>,
    pub(crate) sni_enabled: bool,
    /// The server's certificate chain (PEM), which may carry the key too.
    pub(crate) local_cert: Option<String>,
    /// The server's private key (PEM), when it is not in `local_cert`.
    pub(crate) local_pk: Option<String>,
}

impl Default for SslOptions {
    fn default() -> SslOptions {
        SslOptions {
            verify_peer: true,
            verify_peer_name: true,
            allow_self_signed: false,
            cafile: None,
            peer_name: None,
            sni_enabled: true,
            local_cert: None,
            local_pk: None,
        }
    }
}

impl SslOptions {
    /// Whether every default is in force, which is the case a shared
    /// configuration can be cached for.
    fn is_default(&self) -> bool {
        self.verify_peer
            && self.verify_peer_name
            && !self.allow_self_signed
            && self.cafile.is_none()
            && self.sni_enabled
    }
}

/// The verifying configuration over the platform trust store, built once.
static DEFAULT_CONFIG: OnceLock<Result<Arc<ClientConfig>, String>> = OnceLock::new();

/// Read the platform's trust store into a rustls root set.
fn platform_roots() -> Result<rustls::RootCertStore, String> {
    let mut roots = rustls::RootCertStore::empty();
    let loaded = rustls_native_certs::load_native_certs();
    for cert in loaded.certs {
        let _ = roots.add(cert);
    }
    if roots.is_empty() {
        return Err(match loaded.errors.first() {
            Some(e) => format!("no trusted certificates: {e}"),
            None => "no trusted certificates in the system store".to_string(),
        });
    }
    Ok(roots)
}

/// A PEM bundle as a root set, for `cafile`.
fn file_roots(path: &str) -> Result<rustls::RootCertStore, String> {
    let pem = std::fs::read(path)
        .map_err(|e| format!("cannot read cafile {path}: {}", strip_os(&e.to_string())))?;
    let mut reader = std::io::BufReader::new(&pem[..]);
    let mut roots = rustls::RootCertStore::empty();
    for cert in rustls_pemfile::certs(&mut reader).flatten() {
        let _ = roots.add(cert);
    }
    if roots.is_empty() {
        return Err(format!("no certificates in cafile {path}"));
    }
    Ok(roots)
}

/// Rust's `io::Error` text without its trailing `(os error N)`.
fn strip_os(s: &str) -> String {
    match s.find(" (os error ") {
        Some(i) => s[..i].to_string(),
        None => s.to_string(),
    }
}

/// The client configuration these options ask for.
fn config_for(o: &SslOptions) -> Result<Arc<ClientConfig>, String> {
    if o.is_default() {
        return DEFAULT_CONFIG
            .get_or_init(|| {
                platform_roots().map(|roots| {
                    Arc::new(ClientConfig::builder().with_root_certificates(roots).with_no_client_auth())
                })
            })
            .clone();
    }
    let roots = match &o.cafile {
        Some(path) => file_roots(path)?,
        None => platform_roots().unwrap_or_else(|_| rustls::RootCertStore::empty()),
    };
    // Anything short of full verification needs a verifier of its own; the
    // chain check and the name check are separately switchable, as php's
    // `verify_peer` and `verify_peer_name` are.
    let cfg = if o.verify_peer && !o.allow_self_signed && o.verify_peer_name {
        ClientConfig::builder().with_root_certificates(roots).with_no_client_auth()
    } else {
        let inner = if o.verify_peer && !o.allow_self_signed {
            rustls::client::WebPkiServerVerifier::builder(Arc::new(roots))
                .build()
                .ok()
        } else {
            None
        };
        ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(RelaxedVerifier {
                inner,
                check_name: o.verify_peer_name,
            }))
            .with_no_client_auth()
    };
    Ok(Arc::new(cfg))
}

/// A verifier that skips the checks php's options switch off.
///
/// With `inner` set the chain is still verified and only the name check is
/// dropped; with it unset nothing about the certificate is checked, which
/// is what `verify_peer => false` and `allow_self_signed` mean.
#[derive(Debug)]
struct RelaxedVerifier {
    inner: Option<Arc<rustls::client::WebPkiServerVerifier>>,
    check_name: bool,
}

impl ServerCertVerifier for RelaxedVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        match &self.inner {
            // The chain is checked; the name only when asked for. There is
            // no way to ask webpki for one without the other, so a skipped
            // name check verifies against the name the certificate itself
            // carries.
            Some(v) if self.check_name => {
                v.verify_server_cert(end_entity, intermediates, server_name, ocsp, now)
            }
            Some(v) => match v.verify_server_cert(end_entity, intermediates, server_name, ocsp, now) {
                Ok(ok) => Ok(ok),
                Err(rustls::Error::InvalidCertificate(
                    rustls::CertificateError::NotValidForName
                    | rustls::CertificateError::NotValidForNameContext { .. },
                )) => Ok(ServerCertVerified::assertion()),
                Err(e) => Err(e),
            },
            None => Ok(ServerCertVerified::assertion()),
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        match &self.inner {
            Some(v) => v.verify_tls12_signature(message, cert, dss),
            None => Ok(HandshakeSignatureValid::assertion()),
        }
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        match &self.inner {
            Some(v) => v.verify_tls13_signature(message, cert, dss),
            None => Ok(HandshakeSignatureValid::assertion()),
        }
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        match &self.inner {
            Some(v) => v.supported_verify_schemes(),
            None => vec![
                SignatureScheme::RSA_PKCS1_SHA256,
                SignatureScheme::RSA_PKCS1_SHA384,
                SignatureScheme::RSA_PKCS1_SHA512,
                SignatureScheme::ECDSA_NISTP256_SHA256,
                SignatureScheme::ECDSA_NISTP384_SHA384,
                SignatureScheme::RSA_PSS_SHA256,
                SignatureScheme::RSA_PSS_SHA384,
                SignatureScheme::RSA_PSS_SHA512,
                SignatureScheme::ED25519,
            ],
        }
    }
}

/// Wrap a connected socket in a TLS session and finish the handshake.
///
/// On failure the socket comes back so the caller can put it where it was —
/// php leaves a stream usable after a refused `stream_socket_enable_crypto`.
pub(crate) fn handshake(
    sock: std::net::TcpStream,
    host: &str,
    o: &SslOptions,
) -> Result<TlsConn, (std::net::TcpStream, String)> {
    let cfg = match config_for(o) {
        Ok(c) => c,
        Err(e) => return Err((sock, e)),
    };
    // SNI is the peer name when one was given, the host otherwise; an
    // address literal has no SNI to send, as php's openssl also leaves it
    // out.
    let name_source = o.peer_name.as_deref().unwrap_or(host);
    let server_name = match ServerName::try_from(name_source.to_string()) {
        Ok(n) => n,
        Err(_) => return Err((sock, format!("invalid peer name `{name_source}`"))),
    };
    let client = match ClientConnection::new(cfg, server_name) {
        Ok(c) => c,
        Err(e) => return Err((sock, tls_text(&e))),
    };
    // Drive the handshake to completion now, so a failure is reported by
    // the call that asked for encryption rather than by the first read.
    let mut stream = StreamOwned::new(client, sock);
    if let Err(e) = complete(&mut stream.conn, &mut stream.sock) {
        let sock = stream.sock;
        return Err((sock, e));
    }
    Ok(TlsConn { stream: Side::Client(stream) })
}

/// Why a server-side handshake did not happen: the warnings php prints
/// before its own `Failed to enable crypto`, one per line.
pub(crate) struct ServerRefusal {
    pub(crate) sock: std::net::TcpStream,
    pub(crate) warnings: Vec<String>,
}

/// Be the server end of a TLS session over an accepted socket, presenting
/// `local_cert`, and finish the handshake.
pub(crate) fn server_handshake(sock: std::net::TcpStream, o: &SslOptions) -> Result<TlsConn, ServerRefusal> {
    let cfg = match server_config_for(o) {
        Ok(c) => c,
        Err(w) => return Err(ServerRefusal { sock, warnings: vec![w] }),
    };
    let server = match ServerConnection::new(cfg) {
        Ok(c) => c,
        Err(e) => return Err(ServerRefusal { sock, warnings: vec![tls_text(&e)] }),
    };
    let mut stream = StreamOwned::new(server, sock);
    if let Err(e) = complete(&mut stream.conn, &mut stream.sock) {
        // With no certificate to present rustls has sent the client its
        // `handshake_failure` alert, and php's words for it are openssl's.
        let text = if o.local_cert.is_none() {
            "SSL_R_NO_SHARED_CIPHER: no suitable shared cipher could be used.  This could be \
             because the server is missing an SSL certificate (local_cert context option)"
                .to_string()
        } else {
            format!("SSL operation failed: {e}")
        };
        return Err(ServerRefusal { sock: stream.sock, warnings: vec![text] });
    }
    Ok(TlsConn { stream: Side::Server(stream) })
}

/// The server configuration `local_cert`/`local_pk` describe, or php's
/// warning for the file that would not load. Without `local_cert` the
/// configuration has no certificate at all, so the handshake fails the way
/// php's does — at the client's hello, with an alert — rather than here.
fn server_config_for(o: &SslOptions) -> Result<Arc<ServerConfig>, String> {
    let builder = ServerConfig::builder().with_no_client_auth();
    let Some(cert_path) = &o.local_cert else {
        return Ok(Arc::new(builder.with_cert_resolver(Arc::new(NoCertificate))));
    };
    let chain_err = || {
        format!(
            "Unable to set local cert chain file `{cert_path}'; Check that your cafile/capath \
             settings include details of your certificate and its issuer"
        )
    };
    let pem = std::fs::read(cert_path).map_err(|_| chain_err())?;
    let certs: Vec<CertificateDer<'static>> =
        rustls_pemfile::certs(&mut &pem[..]).filter_map(Result::ok).collect();
    if certs.is_empty() {
        return Err(chain_err());
    }
    let key_path = o.local_pk.as_deref().unwrap_or(cert_path);
    let key_err = || format!("Unable to set private key file `{key_path}'");
    let key_pem = if key_path == cert_path { pem.clone() } else { std::fs::read(key_path).map_err(|_| key_err())? };
    let key = rustls_pemfile::private_key(&mut &key_pem[..])
        .ok()
        .flatten()
        .ok_or_else(key_err)?;
    // A key that does not belong to the certificate is openssl's
    // `SSL_CTX_check_private_key` failure, which php words this way.
    builder
        .with_single_cert(certs, key)
        .map(Arc::new)
        .map_err(|_| "Private key does not match certificate!".to_string())
}

/// The resolver of a server with no `local_cert`: never a certificate.
#[derive(Debug)]
struct NoCertificate;

impl rustls::server::ResolvesServerCert for NoCertificate {
    fn resolve(&self, _hello: rustls::server::ClientHello<'_>) -> Option<Arc<rustls::sign::CertifiedKey>> {
        None
    }
}

/// Run the handshake until rustls is done or reports why it cannot be.
fn complete<C, S>(conn: &mut C, sock: &mut std::net::TcpStream) -> Result<(), String>
where
    C: std::ops::DerefMut<Target = ConnectionCommon<S>>,
    S: SideData,
{
    while conn.is_handshaking() {
        if conn.wants_write() {
            conn.write_tls(sock).map_err(|e| strip_os(&e.to_string()))?;
            continue;
        }
        if conn.wants_read() {
            match conn.read_tls(sock) {
                Ok(0) => return Err("the peer closed the connection during the handshake".into()),
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(strip_os(&e.to_string())),
            }
            if let Err(e) = conn.process_new_packets() {
                // The alert that says why is still queued: send it, as
                // openssl does, so the peer hears the reason and not a
                // reset.
                let _ = conn.write_tls(sock);
                return Err(tls_text(&e));
            }
            continue;
        }
        break;
    }
    // The server's last flight (its `Finished`, session tickets) may still
    // be queued when the handshake is done from its side.
    while conn.wants_write() {
        conn.write_tls(sock).map_err(|e| strip_os(&e.to_string()))?;
    }
    Ok(())
}

/// A rustls error as php would word the tail of its warning.
fn tls_text(e: &rustls::Error) -> String {
    match e {
        rustls::Error::InvalidCertificate(c) => match c {
            rustls::CertificateError::Expired => "certificate has expired".to_string(),
            rustls::CertificateError::NotValidYet => "certificate is not valid yet".to_string(),
            rustls::CertificateError::NotValidForName
            | rustls::CertificateError::NotValidForNameContext { .. } => {
                "certificate is not valid for this name".to_string()
            }
            rustls::CertificateError::UnknownIssuer => {
                "certificate verify failed: unable to get local issuer certificate".to_string()
            }
            other => format!("certificate verify failed: {other:?}"),
        },
        other => other.to_string(),
    }
}
