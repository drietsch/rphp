//! A `multipart/form-data` body (RFC 1867 / RFC 7578) split into its parts.

/// One part: a form field (`filename` absent) or an upload.
#[derive(Debug, PartialEq, Eq)]
pub struct Part {
    /// The `name` of the `Content-Disposition`.
    pub name: Vec<u8>,
    /// The `filename`, when this is a file.
    pub filename: Option<String>,
    /// The part's `Content-Type`, when given.
    pub content_type: Option<String>,
    pub body: Vec<u8>,
}

/// Split `body` at `boundary`. Parts without a `name` are skipped; a body
/// that does not start with the boundary yields nothing, as php's parser
/// gives up on it.
pub fn parse(body: &[u8], boundary: &[u8]) -> Vec<Part> {
    let mut delim = b"--".to_vec();
    delim.extend_from_slice(boundary);
    let mut parts = Vec::new();
    // The first delimiter may be preceded by a preamble; find it.
    let Some(mut pos) = find(body, &delim, 0) else {
        return parts;
    };
    pos += delim.len();
    loop {
        // After a delimiter: `--` closes, else CRLF then the part.
        if body[pos..].starts_with(b"--") {
            break;
        }
        pos = skip_eol(body, pos);
        // Headers up to the blank line.
        let mut name = None;
        let mut filename = None;
        let mut content_type = None;
        loop {
            let Some(eol) = find(body, b"\n", pos) else {
                return parts;
            };
            let line = trim_cr(&body[pos..eol]);
            pos = eol + 1;
            if line.is_empty() {
                break;
            }
            let text = String::from_utf8_lossy(line).into_owned();
            let Some((hname, hvalue)) = text.split_once(':') else {
                continue;
            };
            if hname.trim().eq_ignore_ascii_case("Content-Disposition") {
                for param in hvalue.split(';').skip(1) {
                    let param = param.trim();
                    if let Some((k, v)) = param.split_once('=') {
                        let v = v.trim().trim_matches('"').to_string();
                        match k.trim().to_ascii_lowercase().as_str() {
                            "name" => name = Some(v.into_bytes()),
                            "filename" => filename = Some(v),
                            _ => {}
                        }
                    }
                }
            } else if hname.trim().eq_ignore_ascii_case("Content-Type") {
                content_type = Some(hvalue.trim().to_string());
            }
        }
        // The body runs to the next delimiter, less the CRLF before it.
        let Some(next) = find(body, &delim, pos) else {
            return parts;
        };
        let mut end = next;
        if end >= 2 && &body[end - 2..end] == b"\r\n" {
            end -= 2;
        } else if end >= 1 && body[end - 1] == b'\n' {
            end -= 1;
        }
        if let Some(name) = name {
            parts.push(Part {
                name,
                filename,
                content_type,
                body: body[pos..end].to_vec(),
            });
        }
        pos = next + delim.len();
    }
    parts
}

fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if from > hay.len() || needle.is_empty() {
        return None;
    }
    hay[from..].windows(needle.len()).position(|w| w == needle).map(|i| i + from)
}

fn skip_eol(body: &[u8], mut pos: usize) -> usize {
    if body[pos..].starts_with(b"\r\n") {
        pos += 2;
    } else if body[pos..].starts_with(b"\n") {
        pos += 1;
    }
    pos
}

fn trim_cr(line: &[u8]) -> &[u8] {
    line.strip_suffix(b"\r").unwrap_or(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_fields_and_files() {
        let b = b"--XX\r\nContent-Disposition: form-data; name=\"a\"\r\n\r\n1\r\n--XX\r\nContent-Disposition: form-data; name=\"f\"; filename=\"up.txt\"\r\nContent-Type: text/plain\r\n\r\nfile content\r\n--XX--\r\n";
        let parts = parse(b, b"XX");
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].name, b"a");
        assert_eq!(parts[0].body, b"1");
        assert_eq!(parts[0].filename, None);
        assert_eq!(parts[1].filename.as_deref(), Some("up.txt"));
        assert_eq!(parts[1].content_type.as_deref(), Some("text/plain"));
        assert_eq!(parts[1].body, b"file content");
    }

    #[test]
    fn empty_and_broken_bodies() {
        assert!(parse(b"", b"XX").is_empty());
        assert!(parse(b"garbage", b"XX").is_empty());
        assert!(parse(b"--XX\r\nContent-Disposition: form-data; name=\"a\"\r\n\r\nunterminated", b"XX").is_empty());
    }
}
