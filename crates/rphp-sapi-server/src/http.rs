//! The wire side: one HTTP/1.x request read off a socket, and the status
//! line / reason phrases a response starts with.
//!
//! `php -S` answers every request with `Connection: close`, so a connection
//! carries exactly one request and there is no keep-alive to manage.

use std::io::{self, BufRead, BufReader, Read};
use std::net::{SocketAddr, TcpStream};

/// The request methods php's parser knows; anything else is a `400`.
const METHODS: &[&str] = &[
    "DELETE", "GET", "HEAD", "POST", "PUT", "CONNECT", "OPTIONS", "TRACE", "COPY", "LOCK",
    "MKCOL", "MOVE", "PROPFIND", "PROPPATCH", "SEARCH", "UNLOCK", "REPORT", "MKACTIVITY",
    "CHECKOUT", "MERGE", "MSEARCH", "NOTIFY", "SUBSCRIBE", "UNSUBSCRIBE", "PATCH", "PURGE",
];

/// The request head and body, as received.
#[derive(Debug)]
pub struct Request {
    /// `GET`, `POST`, … (upper case, one of [`METHODS`]).
    pub method: String,
    /// The request target as sent (`$_SERVER['REQUEST_URI']`).
    pub uri: String,
    /// The path part of the target, before decoding.
    pub raw_path: String,
    /// The query string, without the `?` (`None` when there was none).
    pub query: Option<String>,
    /// `(major, minor)`.
    pub version: (u8, u8),
    /// Header fields in order, original spelling; a repeated field's values
    /// are joined with `, ` the way php's server does.
    pub headers: Vec<(String, String)>,
    /// The body (`Content-Length` or chunked).
    pub body: Vec<u8>,
    /// The peer.
    pub remote: SocketAddr,
}

impl Request {
    /// A header's value, case-insensitively.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// `HTTP/1.1` as `$_SERVER['SERVER_PROTOCOL']` spells it.
    pub fn protocol(&self) -> String {
        format!("HTTP/{}.{}", self.version.0, self.version.1)
    }
}

/// Why a request could not be read: php answers `400` for a malformed one
/// and drops a connection that closes before a request line.
#[derive(Debug)]
pub enum ReadError {
    /// The peer sent nothing (or hung up).
    Empty,
    /// Malformed request line, header or body.
    Bad(&'static str),
    Io(io::Error),
}

impl From<io::Error> for ReadError {
    fn from(e: io::Error) -> Self {
        ReadError::Io(e)
    }
}

/// The largest request head php's server accepts before giving up.
const MAX_HEAD: usize = 64 * 1024;

/// Read one request from `stream`. Sends `100 Continue` when the client
/// asks for it before reading the body.
pub fn read_request(stream: &mut TcpStream, remote: SocketAddr) -> Result<Request, ReadError> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = Vec::new();
    // Tolerate leading empty lines, as HTTP allows.
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 {
            return Err(ReadError::Empty);
        }
        if !matches!(line.as_slice(), b"\r\n" | b"\n") {
            break;
        }
    }
    let request_line = String::from_utf8_lossy(trim_eol(&line)).into_owned();
    let mut parts = request_line.split(' ');
    let method = parts.next().unwrap_or("").to_string();
    let uri = parts.next().ok_or(ReadError::Bad("request line"))?.to_string();
    let version = match parts.next() {
        Some("HTTP/1.1") => (1, 1),
        Some("HTTP/1.0") => (1, 0),
        Some(v) if v.starts_with("HTTP/") => {
            let mut it = v[5..].split('.');
            let major = it.next().and_then(|s| s.parse().ok()).ok_or(ReadError::Bad("version"))?;
            let minor = it.next().and_then(|s| s.parse().ok()).ok_or(ReadError::Bad("version"))?;
            (major, minor)
        }
        // A bare `GET /path` is HTTP/0.9; php's parser treats it as 1.0.
        None => (1, 0),
        Some(_) => return Err(ReadError::Bad("version")),
    };
    if !METHODS.contains(&method.as_str()) {
        return Err(ReadError::Bad("method"));
    }
    let mut headers: Vec<(String, String)> = Vec::new();
    let mut total = line.len();
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 {
            return Err(ReadError::Bad("truncated head"));
        }
        total += line.len();
        if total > MAX_HEAD {
            return Err(ReadError::Bad("head too large"));
        }
        let l = trim_eol(&line);
        if l.is_empty() {
            break;
        }
        let text = String::from_utf8_lossy(l);
        let Some((name, value)) = text.split_once(':') else {
            return Err(ReadError::Bad("header"));
        };
        let name = name.trim().to_string();
        let value = value.trim().to_string();
        match headers.iter_mut().find(|(n, _)| n.eq_ignore_ascii_case(&name)) {
            Some((_, v)) => {
                v.push_str(", ");
                v.push_str(&value);
            }
            None => headers.push((name, value)),
        }
    }
    let (raw_path, query) = match uri.split_once('?') {
        Some((p, q)) => (p.to_string(), Some(q.to_string())),
        None => (uri.clone(), None),
    };
    let find = |name: &str| {
        headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    };
    if find("Expect").is_some_and(|e| e.eq_ignore_ascii_case("100-continue")) {
        use std::io::Write;
        stream.write_all(b"HTTP/1.1 100 Continue\r\n\r\n")?;
    }
    let body = if find("Transfer-Encoding").is_some_and(|t| t.to_ascii_lowercase().contains("chunked")) {
        read_chunked(&mut reader)?
    } else if let Some(len) = find("Content-Length") {
        let len: usize = len.trim().parse().map_err(|_| ReadError::Bad("content-length"))?;
        let mut body = vec![0u8; len];
        reader.read_exact(&mut body)?;
        body
    } else {
        Vec::new()
    };
    Ok(Request {
        method,
        uri,
        raw_path,
        query,
        version,
        headers,
        body,
        remote,
    })
}

/// A chunked request body, trailers discarded.
fn read_chunked(reader: &mut BufReader<TcpStream>) -> Result<Vec<u8>, ReadError> {
    let mut body = Vec::new();
    let mut line = Vec::new();
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 {
            return Err(ReadError::Bad("truncated chunk"));
        }
        let text = String::from_utf8_lossy(trim_eol(&line)).into_owned();
        let size_text = text.split(';').next().unwrap_or("").trim();
        let size = usize::from_str_radix(size_text, 16).map_err(|_| ReadError::Bad("chunk size"))?;
        if size == 0 {
            // Trailers up to the blank line.
            loop {
                line.clear();
                if reader.read_until(b'\n', &mut line)? == 0 || trim_eol(&line).is_empty() {
                    break;
                }
            }
            return Ok(body);
        }
        let start = body.len();
        body.resize(start + size, 0);
        reader.read_exact(&mut body[start..])?;
        line.clear();
        reader.read_until(b'\n', &mut line)?;
    }
}

fn trim_eol(line: &[u8]) -> &[u8] {
    let mut end = line.len();
    while end > 0 && (line[end - 1] == b'\n' || line[end - 1] == b'\r') {
        end -= 1;
    }
    &line[..end]
}

/// php's reason phrase for a status code (`main/http_status_codes.h`),
/// `Unknown Status Code` for one it does not list.
pub fn reason(code: i64) -> &'static str {
    rphp_embed::cgi::reason(code).unwrap_or("Unknown Status Code")
}

/// `HTTP/1.1 404 Not Found\r\n` (a code of 0 is 200).
pub fn status_line(version: (u8, u8), code: i64) -> String {
    let code = if code == 0 { 200 } else { code };
    format!("HTTP/{}.{} {} {}\r\n", version.0, version.1, code, reason(code))
}

/// `Sun, 20 Sep 2026 09:54:16 GMT` — the `Date` header, now.
pub fn http_date_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    http_date(secs)
}

/// The `D, d M Y H:i:s GMT` of a unix timestamp.
pub fn http_date(ts: i64) -> String {
    const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let days = ts.div_euclid(86_400);
    let secs = ts.rem_euclid(86_400);
    // Howard Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    let wd = (days + 4).rem_euclid(7) as usize;
    format!(
        "{}, {:02} {} {:04} {:02}:{:02}:{:02} GMT",
        DAYS[wd],
        d,
        MONTHS[(m - 1) as usize],
        y,
        secs / 3600,
        (secs / 60) % 60,
        secs % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(http_date(0), "Thu, 01 Jan 1970 00:00:00 GMT");
        assert_eq!(http_date(1_700_000_000), "Tue, 14 Nov 2023 22:13:20 GMT");
        assert_eq!(http_date(1_800_000_000), "Fri, 15 Jan 2027 08:00:00 GMT");
    }

    #[test]
    fn status_lines() {
        assert_eq!(status_line((1, 1), 0), "HTTP/1.1 200 OK\r\n");
        assert_eq!(status_line((1, 0), 404), "HTTP/1.0 404 Not Found\r\n");
        assert_eq!(status_line((1, 1), 299), "HTTP/1.1 299 Unknown Status Code\r\n");
    }
}
