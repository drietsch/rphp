//! The FastCGI wire format (the FastCGI specification, §3 and §8): 8-byte
//! record headers, the name-value pair encoding of `FCGI_PARAMS` and
//! `FCGI_GET_VALUES`, and the record types a responder deals with.

use std::io::{self, Read, Write};

pub const VERSION_1: u8 = 1;

pub const BEGIN_REQUEST: u8 = 1;
pub const ABORT_REQUEST: u8 = 2;
pub const END_REQUEST: u8 = 3;
pub const PARAMS: u8 = 4;
pub const STDIN: u8 = 5;
pub const STDOUT: u8 = 6;
pub const STDERR: u8 = 7;
pub const GET_VALUES: u8 = 9;
pub const GET_VALUES_RESULT: u8 = 10;
pub const UNKNOWN_TYPE: u8 = 11;

/// `FCGI_BEGIN_REQUEST` roles.
pub const ROLE_RESPONDER: u16 = 1;
/// `FCGI_BEGIN_REQUEST` flag: keep the connection after the request.
pub const KEEP_CONN: u8 = 1;

/// `FCGI_END_REQUEST` protocol statuses.
pub const REQUEST_COMPLETE: u8 = 0;
pub const CANT_MPX_CONN: u8 = 1;
pub const UNKNOWN_ROLE: u8 = 3;

/// The management request id.
pub const NULL_REQUEST_ID: u16 = 0;

/// The largest content one record carries.
pub const MAX_CONTENT: usize = 65535;

/// One record as read off the wire.
pub struct Record {
    pub kind: u8,
    pub request_id: u16,
    pub content: Vec<u8>,
}

/// Read one record; `None` at a clean end of stream.
pub fn read_record(r: &mut impl Read) -> io::Result<Option<Record>> {
    let mut head = [0u8; 8];
    let mut got = 0;
    while got < 8 {
        let n = r.read(&mut head[got..])?;
        if n == 0 {
            if got == 0 {
                return Ok(None);
            }
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "truncated record header"));
        }
        got += n;
    }
    if head[0] != VERSION_1 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "unsupported FastCGI version"));
    }
    let kind = head[1];
    let request_id = u16::from_be_bytes([head[2], head[3]]);
    let content_len = u16::from_be_bytes([head[4], head[5]]) as usize;
    let padding = head[6] as usize;
    let mut content = vec![0u8; content_len];
    r.read_exact(&mut content)?;
    if padding > 0 {
        let mut pad = [0u8; 255];
        r.read_exact(&mut pad[..padding])?;
    }
    Ok(Some(Record { kind, request_id, content }))
}

/// Write one record; content over [`MAX_CONTENT`] is split into several.
/// The content is padded to a multiple of 8, as the specification
/// recommends.
pub fn write_record(w: &mut impl Write, kind: u8, request_id: u16, content: &[u8]) -> io::Result<()> {
    let mut chunks = content.chunks(MAX_CONTENT).peekable();
    if chunks.peek().is_none() {
        return write_one(w, kind, request_id, &[]);
    }
    for chunk in chunks {
        write_one(w, kind, request_id, chunk)?;
    }
    Ok(())
}

fn write_one(w: &mut impl Write, kind: u8, request_id: u16, content: &[u8]) -> io::Result<()> {
    let padding = (8 - content.len() % 8) % 8;
    let mut buf = Vec::with_capacity(8 + content.len() + padding);
    buf.push(VERSION_1);
    buf.push(kind);
    buf.extend_from_slice(&request_id.to_be_bytes());
    buf.extend_from_slice(&(content.len() as u16).to_be_bytes());
    buf.push(padding as u8);
    buf.push(0);
    buf.extend_from_slice(content);
    buf.resize(buf.len() + padding, 0);
    w.write_all(&buf)
}

/// The body of `FCGI_END_REQUEST`.
pub fn end_request_body(app_status: u32, protocol_status: u8) -> [u8; 8] {
    let s = app_status.to_be_bytes();
    [s[0], s[1], s[2], s[3], protocol_status, 0, 0, 0]
}

/// Decode a name-value stream (`FCGI_PARAMS`, `FCGI_GET_VALUES`): each
/// length is one byte, or four with the high bit set.
pub fn decode_pairs(mut data: &[u8]) -> Vec<(Vec<u8>, Vec<u8>)> {
    let mut out = Vec::new();
    loop {
        let Some((name_len, rest)) = read_len(data) else { break };
        let Some((value_len, rest)) = read_len(rest) else { break };
        if rest.len() < name_len + value_len {
            break;
        }
        let (name, rest) = rest.split_at(name_len);
        let (value, rest) = rest.split_at(value_len);
        out.push((name.to_vec(), value.to_vec()));
        data = rest;
    }
    out
}

fn read_len(data: &[u8]) -> Option<(usize, &[u8])> {
    let first = *data.first()?;
    if first < 0x80 {
        return Some((first as usize, &data[1..]));
    }
    if data.len() < 4 {
        return None;
    }
    let n = u32::from_be_bytes([first & 0x7f, data[1], data[2], data[3]]) as usize;
    Some((n, &data[4..]))
}

/// Encode name-value pairs (`FCGI_GET_VALUES_RESULT`).
pub fn encode_pairs(pairs: &[(&[u8], &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    for (name, value) in pairs {
        write_len(&mut out, name.len());
        write_len(&mut out, value.len());
        out.extend_from_slice(name);
        out.extend_from_slice(value);
    }
    out
}

fn write_len(out: &mut Vec<u8>, n: usize) {
    if n < 0x80 {
        out.push(n as u8);
    } else {
        out.extend_from_slice(&((n as u32) | 0x8000_0000).to_be_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairs_round_trip() {
        let long = vec![b'x'; 300];
        let enc = encode_pairs(&[(b"A", b"1"), (b"LONG", &long)]);
        let dec = decode_pairs(&enc);
        assert_eq!(dec, vec![(b"A".to_vec(), b"1".to_vec()), (b"LONG".to_vec(), long)]);
    }

    #[test]
    fn records_round_trip() {
        let mut buf = Vec::new();
        write_record(&mut buf, STDOUT, 7, b"hello").unwrap();
        assert_eq!(buf.len(), 8 + 8);
        let rec = read_record(&mut &buf[..]).unwrap().unwrap();
        assert_eq!((rec.kind, rec.request_id, rec.content.as_slice()), (STDOUT, 7, &b"hello"[..]));
        let mut big = Vec::new();
        write_record(&mut big, STDOUT, 1, &vec![1u8; MAX_CONTENT + 1]).unwrap();
        let first = read_record(&mut &big[..]).unwrap().unwrap();
        assert_eq!(first.content.len(), MAX_CONTENT);
    }
}
