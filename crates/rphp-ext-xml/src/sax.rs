//! The event tokenizer behind `ext/xml`: a replica of libxml2's *push*
//! parser (`xmlParseChunk` / `xmlParseTryOrFinish`) as php's expat-compat
//! layer drives it, because what php reports is shaped by that parser's
//! steps rather than by the XML grammar alone:
//!
//! - which chunk of a document a call to `xml_parse()` may consume (a
//!   start tag waits for its `>`, text for the next `<`, a lone trailing
//!   byte is never looked at);
//! - how character data is split into callbacks (the fast ASCII path stops
//!   at `&`, `<`, a bare CR and the first non-ASCII byte; the slow path
//!   delivers 300-byte pieces);
//! - the line, column and byte index a handler sees (the column is past
//!   the text, the byte index sometimes before it; a start tag reports at
//!   its `>`);
//! - libxml2's error numbers (`xml_get_error_code()`), the last one of a
//!   step winning, and where parsing stopped after a fatal one (a halted
//!   parser reports byte index 0).
//!
//! The tokenizer only records events with their positions; `xml.rs`
//! dispatches them to the php handlers.

use std::collections::HashMap;

use rphp_ext_dom::{is_name_char, is_name_start};

/// libxml2's `XML_PARSER_BIG_BUFFER_SIZE`.
const BIG: usize = 300;

/// A parser position as `xml_get_current_*()` report it.
#[derive(Clone, Copy, Debug, Default)]
pub struct Pos {
    pub line: i64,
    pub col: i64,
    pub byte: i64,
}

#[derive(Debug)]
pub enum Ev {
    /// A start tag: the name (a qname, or `uri<sep>local` in namespace
    /// mode), the attributes, and (namespace mode) the declarations it
    /// carries.
    Start {
        name: Vec<u8>,
        attrs: Vec<(Vec<u8>, Vec<u8>)>,
        ns: Vec<(Option<Vec<u8>>, Vec<u8>)>,
    },
    End(Vec<u8>),
    Chars(Vec<u8>),
    Pi(Vec<u8>, Option<Vec<u8>>),
    Comment(Vec<u8>),
    /// A reference to a declared internal entity (its replacement text) or
    /// to an undeclared one (`None`).
    EntityRef(Vec<u8>, Option<Vec<u8>>),
    /// A reference to an external parsed entity: name, system id, public id.
    ExternalRef(Vec<u8>, Vec<u8>, Option<Vec<u8>>),
    /// `<!ENTITY n SYSTEM ... NDATA notation>`: name, system id, public id,
    /// notation.
    Unparsed(Vec<u8>, Vec<u8>, Option<Vec<u8>>, Vec<u8>),
    /// `<!NOTATION n ...>`: name, system id, public id.
    Notation(Vec<u8>, Option<Vec<u8>>, Option<Vec<u8>>),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum St {
    Start,
    Misc,
    Prolog,
    Dtd,
    StartTag,
    Content,
    CData,
    EndTag,
    Epilog,
    Eof,
}

#[derive(Clone)]
enum Entity {
    Internal(Vec<u8>),
    External(Vec<u8>, Option<Vec<u8>>),
    Unparsed,
}

pub struct Sax {
    buf: Vec<u8>,
    cur: usize,
    line: i64,
    col: i64,
    halted: bool,
    /// Where a switch to a declared 8-bit encoding happened: libxml counts
    /// its byte index from the re-made input buffer.
    byte_base: usize,
    state: St,
    /// `ctxt->errNo`: the last error raised.
    pub err: i64,
    /// A fatal error disabled the SAX callbacks (`disableSAX`).
    fatal: bool,
    charset_known: bool,
    /// The document declared ISO-8859-1 (or ASCII): the rest of the input
    /// is transcoded as it arrives.
    latin1: bool,
    ns_mode: bool,
    sep: Vec<u8>,
    /// Open elements: the qname, the reported name, the namespace
    /// declarations it pushed.
    stack: Vec<(Vec<u8>, Vec<u8>, usize)>,
    ns: Vec<(Option<Vec<u8>>, Vec<u8>)>,
    entities: HashMap<Vec<u8>, Entity>,
    has_ext_subset: bool,
    has_perefs: bool,
    standalone: bool,
    pub events: Vec<(Ev, Pos)>,
}

fn is_blank(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r')
}

/// libxml2's `test_char_data`: the bytes the fast text path runs over.
fn test_char_data(b: u8) -> bool {
    b == 0x09 || ((0x20..=0x7F).contains(&b) && b != b'<' && b != b'&' && b != b']')
}

pub fn is_xml_char(c: u32) -> bool {
    matches!(c, 0x9 | 0xA | 0xD | 0x20..=0xD7FF | 0xE000..=0xFFFD | 0x10000..=0x10FFFF)
}

/// Decode one UTF-8 character at the start of `s`: (code point, length),
/// `None` for a malformed sequence.
pub fn utf8_at(s: &[u8]) -> Option<(u32, usize)> {
    let b0 = *s.first()?;
    if b0 < 0x80 {
        return Some((b0 as u32, 1));
    }
    let (n, init) = match b0 {
        0xC2..=0xDF => (2, (b0 & 0x1F) as u32),
        0xE0..=0xEF => (3, (b0 & 0x0F) as u32),
        0xF0..=0xF4 => (4, (b0 & 0x07) as u32),
        _ => return None,
    };
    if s.len() < n {
        return None;
    }
    let mut c = init;
    for &b in &s[1..n] {
        if b & 0xC0 != 0x80 {
            return None;
        }
        c = (c << 6) | (b & 0x3F) as u32;
    }
    let min = [0, 0, 0x80, 0x800, 0x10000][n];
    if c < min || c > 0x10FFFF || (0xD800..=0xDFFF).contains(&c) {
        return None;
    }
    Some((c, n))
}

pub fn push_utf8(out: &mut Vec<u8>, c: u32) {
    let mut tmp = [0u8; 4];
    if let Some(ch) = char::from_u32(c) {
        out.extend_from_slice(ch.encode_utf8(&mut tmp).as_bytes());
    }
}

fn latin1_to_utf8(src: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(src.len());
    for &b in src {
        push_utf8(&mut out, b as u32);
    }
    out
}

const XML_NS: &[u8] = b"http://www.w3.org/XML/1998/namespace";

impl Sax {
    pub fn new(ns_mode: bool, sep: &[u8]) -> Sax {
        Sax {
            buf: Vec::new(),
            cur: 0,
            line: 1,
            col: 1,
            halted: false,
            byte_base: 0,
            state: St::Start,
            err: 0,
            fatal: false,
            charset_known: false,
            latin1: false,
            ns_mode,
            // php's compat layer keeps only the separator's first byte.
            sep: sep.iter().take(1).copied().collect(),
            stack: Vec::new(),
            ns: vec![(Some(b"xml".to_vec()), XML_NS.to_vec())],
            entities: HashMap::new(),
            has_ext_subset: false,
            has_perefs: false,
            standalone: false,
            events: Vec::new(),
        }
    }

    /// The position `xml_get_current_*()` answer with now.
    pub fn pos(&self) -> Pos {
        Pos {
            line: self.line,
            col: self.col,
            byte: if self.halted { 0 } else { (self.cur - self.byte_base) as i64 },
        }
    }

    /// `xmlParseChunk`: add `data` and parse what can be parsed. Returns
    /// php's `xml_parse()` answer (1 when no error is recorded).
    pub fn feed(&mut self, data: &[u8], terminate: bool) -> bool {
        if self.err != 0 && self.fatal {
            return false;
        }
        if self.state == St::Eof {
            return false;
        }
        if self.latin1 {
            let t = latin1_to_utf8(data);
            self.buf.extend_from_slice(&t);
        } else {
            self.buf.extend_from_slice(data);
        }
        self.run(terminate);
        if self.state == St::Eof {
            return self.err == 0;
        }
        if self.err != 0 && self.fatal {
            return false;
        }
        if terminate {
            // Not in the epilog, or junk left in it: the document did not
            // end where it should.
            if self.state != St::Epilog || self.buf.len() > self.cur {
                self.fatal_err(5);
            }
            self.state = St::Eof;
        }
        self.err == 0
    }

    // ---- input ---------------------------------------------------------

    fn at(&self, i: usize) -> u8 {
        self.buf.get(i).copied().unwrap_or(0)
    }

    fn raw(&self) -> u8 {
        self.at(self.cur)
    }

    fn nxt(&self, n: usize) -> u8 {
        self.at(self.cur + n)
    }

    fn avail(&self) -> usize {
        self.buf.len() - self.cur
    }

    fn starts(&self, s: &[u8]) -> bool {
        self.buf[self.cur..].starts_with(s)
    }

    /// `SKIP(n)` over ASCII: the column moves by bytes.
    fn skip(&mut self, n: usize) {
        self.cur += n;
        self.col += n as i64;
    }

    /// `SKIPL(n)`: line-aware, a column per byte.
    fn skipl(&mut self, n: usize) {
        for _ in 0..n {
            if self.raw() == b'\n' {
                self.line += 1;
                self.col = 1;
            } else {
                self.col += 1;
            }
            self.cur += 1;
        }
    }

    /// `NEXTL` over one character of `len` bytes.
    fn nextl(&mut self, len: usize) {
        if self.raw() == b'\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        self.cur += len;
    }

    /// `CUR_CHAR`: the character at the cursor, CR/LF folded (a CR before
    /// an LF is stepped over). `0` at the end; a malformed byte raises the
    /// encoding error and reads as Latin-1.
    fn cur_char(&mut self) -> (u32, usize) {
        let b = self.raw();
        if self.cur >= self.buf.len() {
            return (0, 0);
        }
        if b == b'\r' {
            if self.nxt(1) == b'\n' {
                self.cur += 1;
            }
            return (0xA, 1);
        }
        if b < 0x80 {
            return (b as u32, 1);
        }
        match utf8_at(&self.buf[self.cur..]) {
            Some(cl) => cl,
            None => {
                self.fatal_err(9);
                (b as u32, 1)
            }
        }
    }

    fn skip_blanks(&mut self) -> usize {
        let mut n = 0;
        while is_blank(self.raw()) && self.cur < self.buf.len() {
            let (_, l) = self.cur_char();
            self.nextl(l);
            n += 1;
        }
        n
    }

    fn find(&self, from: usize, needle: &[u8]) -> Option<usize> {
        if from > self.buf.len() {
            return None;
        }
        self.buf[from..]
            .windows(needle.len())
            .position(|w| w == needle)
            .map(|p| p + from)
    }

    // ---- errors and events -------------------------------------------------

    fn fatal_err(&mut self, code: i64) {
        self.err = code;
        self.fatal = true;
    }

    /// A namespace error or a non-fatal error: recorded, parsing goes on.
    fn soft_err(&mut self, code: i64) {
        self.err = code;
    }

    fn halt(&mut self) {
        self.state = St::Eof;
        self.fatal = true;
        self.halted = true;
    }

    fn emit(&mut self, ev: Ev) {
        if !self.fatal {
            let p = self.pos();
            self.events.push((ev, p));
        }
    }

    fn emit_at(&mut self, ev: Ev, p: Pos) {
        if !self.fatal {
            self.events.push((ev, p));
        }
    }

    // ---- the push loop ---------------------------------------------------

    fn run(&mut self, terminate: bool) {
        loop {
            if self.state == St::Eof {
                return;
            }
            if self.err != 0 && self.fatal {
                return;
            }
            let avail = self.avail();
            if avail < 1 {
                return;
            }
            match self.state {
                St::Eof => return,
                St::Start => {
                    if !self.charset_known {
                        if avail < 4 {
                            return;
                        }
                        self.charset_known = true;
                        if self.starts(b"\xEF\xBB\xBF") {
                            self.cur += 3;
                        }
                        continue;
                    }
                    if avail < 2 {
                        return;
                    }
                    if self.raw() == 0 {
                        self.fatal_err(4);
                        self.halt();
                        return;
                    }
                    if self.starts(b"<?") {
                        if avail < 5 {
                            return;
                        }
                        if !terminate && self.find(self.cur, b"?>").is_none() {
                            return;
                        }
                        if self.nxt(2) == b'x'
                            && self.nxt(3) == b'm'
                            && self.nxt(4) == b'l'
                            && is_blank(self.nxt(5))
                        {
                            self.xml_decl();
                            if self.err == 32 {
                                self.halt();
                                return;
                            }
                        }
                    }
                    self.state = St::Misc;
                }
                St::Misc | St::Prolog | St::Epilog => {
                    self.skip_blanks();
                    let avail = self.avail();
                    if avail < 2 {
                        return;
                    }
                    if self.starts(b"<?") {
                        if !terminate && self.find(self.cur, b"?>").is_none() {
                            return;
                        }
                        self.pi();
                    } else if self.starts(b"<!--") {
                        if !terminate && self.find(self.cur, b"-->").is_none() {
                            return;
                        }
                        self.comment();
                    } else if self.state == St::Misc && self.starts(b"<!DOCTYPE") {
                        if !terminate && self.find(self.cur, b">").is_none() {
                            return;
                        }
                        self.doctype();
                        if self.state == St::Eof {
                            return;
                        }
                        self.state = if self.raw() == b'[' { St::Dtd } else { St::Prolog };
                    } else if self.raw() == b'<' && self.nxt(1) == b'!' && avail < 9 {
                        return;
                    } else if self.state == St::Epilog {
                        self.fatal_err(5);
                        self.halt();
                        return;
                    } else {
                        self.state = St::StartTag;
                    }
                }
                St::Dtd => {
                    if !terminate && !self.internal_subset_complete() {
                        return;
                    }
                    self.internal_subset();
                    if self.state == St::Eof {
                        return;
                    }
                    self.state = St::Prolog;
                }
                St::StartTag => {
                    if avail < 2 {
                        return;
                    }
                    if self.raw() != b'<' {
                        self.fatal_err(4);
                        self.halt();
                        return;
                    }
                    if !terminate && self.find(self.cur, b">").is_none() {
                        return;
                    }
                    let Some((qname, name, nsn)) = self.start_tag() else {
                        self.halt();
                        return;
                    };
                    if self.raw() == b'/' && self.nxt(1) == b'>' {
                        self.skip(2);
                        self.emit(Ev::End(name));
                        self.ns.truncate(self.ns.len() - nsn);
                        self.state = if self.stack.is_empty() { St::Epilog } else { St::Content };
                        continue;
                    }
                    if self.raw() == b'>' {
                        self.skip(1);
                    } else {
                        self.fatal_err(73);
                    }
                    self.stack.push((qname, name, nsn));
                    self.state = St::Content;
                }
                St::Content => {
                    if avail < 2 {
                        return;
                    }
                    let (c, n) = (self.raw(), self.nxt(1));
                    let before = self.cur;
                    if c == b'<' && n == b'/' {
                        self.state = St::EndTag;
                        continue;
                    } else if c == b'<' && n == b'?' {
                        if !terminate && self.find(self.cur, b"?>").is_none() {
                            return;
                        }
                        self.pi();
                    } else if c == b'<' && n != b'!' {
                        self.state = St::StartTag;
                        continue;
                    } else if self.starts(b"<!--") {
                        if !terminate && self.find(self.cur + 4, b"-->").is_none() {
                            return;
                        }
                        self.comment();
                    } else if self.starts(b"<![CDATA[") {
                        self.skip(9);
                        self.state = St::CData;
                        continue;
                    } else if c == b'<' && n == b'!' && avail < 9 {
                        return;
                    } else if c == b'&' {
                        if !terminate && self.find(self.cur, b";").is_none() {
                            return;
                        }
                        self.reference();
                    } else {
                        if avail < BIG && !terminate && self.find(self.cur, b"<").is_none() {
                            return;
                        }
                        self.char_data();
                    }
                    if self.cur == before && self.state == St::Content {
                        self.fatal_err(1);
                        self.halt();
                        return;
                    }
                }
                St::CData => {
                    match self.find(self.cur, b"]]>") {
                        None => {
                            if avail >= BIG + 2 {
                                let mut n = BIG;
                                while n > 0 && self.buf[self.cur + n] & 0xC0 == 0x80 {
                                    n -= 1;
                                }
                                let text = self.buf[self.cur..self.cur + n].to_vec();
                                self.emit(Ev::Chars(text));
                                self.skipl(n);
                                continue;
                            }
                            return;
                        }
                        Some(end) => {
                            let text = self.buf[self.cur..end].to_vec();
                            if std::str::from_utf8(&text).is_err() {
                                self.fatal_err(9);
                                self.halt();
                                return;
                            }
                            self.emit(Ev::Chars(text));
                            let n = end - self.cur + 3;
                            self.skipl(n);
                            self.state = St::Content;
                        }
                    }
                }
                St::EndTag => {
                    if avail < 2 {
                        return;
                    }
                    if !terminate && self.find(self.cur, b">").is_none() {
                        return;
                    }
                    self.end_tag();
                    if self.state == St::Eof {
                        return;
                    }
                    self.state = if self.stack.is_empty() { St::Epilog } else { St::Content };
                }
            }
        }
    }

    // ---- names -------------------------------------------------------------

    /// `xmlParseName` (with `:`), or an NCName when `nc`.
    fn parse_name(&mut self, nc: bool) -> Option<Vec<u8>> {
        let start = self.cur;
        let mut p = self.cur;
        let mut first = true;
        let mut chars = 0;
        while let Some((c, l)) = utf8_at(&self.buf[p..]) {
            let ch = char::from_u32(c).unwrap_or('\0');
            let ok = if nc && ch == ':' {
                false
            } else if first {
                is_name_start(ch)
            } else {
                is_name_char(ch)
            };
            if !ok {
                break;
            }
            first = false;
            p += l;
            chars += 1;
        }
        if p == start {
            return None;
        }
        self.cur = p;
        self.col += chars;
        Some(self.buf[start..p].to_vec())
    }

    /// A QName in namespace mode: (prefix, local).
    fn parse_qname(&mut self) -> Option<(Option<Vec<u8>>, Vec<u8>)> {
        let first = self.parse_name(true)?;
        if self.raw() == b':' {
            let save = (self.cur, self.col);
            self.skip(1);
            match self.parse_name(true) {
                Some(local) => return Some((Some(first), local)),
                None => {
                    (self.cur, self.col) = save;
                    // `a:` — libxml takes the whole thing as a name.
                    self.skip(1);
                    let mut n = first;
                    n.push(b':');
                    return Some((None, n));
                }
            }
        }
        Some((None, first))
    }

    // ---- declarations ------------------------------------------------------

    /// `xmlParseXMLDecl`.
    fn xml_decl(&mut self) {
        self.skip(5);
        if !is_blank(self.raw()) {
            self.fatal_err(65);
        }
        self.skip_blanks();
        // VersionInfo.
        let mut version = None;
        if self.starts(b"version") {
            self.skip(7);
            self.skip_blanks();
            if self.raw() != b'=' {
                self.fatal_err(75);
            } else {
                self.skip(1);
                self.skip_blanks();
                version = self.quoted(|b| b.is_ascii_digit() || b == b'.');
            }
        }
        match &version {
            None => self.fatal_err(96),
            Some(v) if v.as_slice() == b"1.0" => {}
            // `XML_WAR_UNKNOWN_VERSION` is a warning: no error number.
            Some(v) if v.starts_with(b"1.") => {}
            Some(_) => self.fatal_err(108),
        }
        if !is_blank(self.raw()) {
            if self.starts(b"?>") {
                self.skip(2);
                return;
            }
            self.fatal_err(65);
        }
        self.skip_blanks();
        let mut has_encoding = false;
        if self.starts(b"encoding") {
            has_encoding = true;
            self.skip(8);
            self.skip_blanks();
            if self.raw() != b'=' {
                self.fatal_err(75);
                return;
            }
            self.skip(1);
            self.skip_blanks();
            let q = self.raw();
            if q != b'"' && q != b'\'' {
                self.fatal_err(33);
            } else {
                self.skip(1);
                let start = self.cur;
                if self.raw().is_ascii_alphabetic() {
                    while matches!(self.raw(), b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'-') {
                        self.skip(1);
                    }
                }
                let enc = self.buf[start..self.cur].to_vec();
                if enc.is_empty() {
                    self.fatal_err(79);
                }
                if self.raw() != q {
                    self.fatal_err(34);
                } else {
                    self.skip(1);
                }
                if !enc.is_empty() {
                    let e = enc.to_ascii_uppercase();
                    match e.as_slice() {
                        b"UTF-8" | b"UTF8" | b"UTF-16" | b"UTF16" => {}
                        b"ISO-8859-1" | b"ISO-LATIN-1" | b"ISO_8859-1" | b"LATIN1" | b"L1" | b"US-ASCII"
                        | b"ASCII" => {
                            self.latin1 = true;
                            self.byte_base = self.cur;
                            let rest = latin1_to_utf8(&self.buf[self.cur..]);
                            self.buf.truncate(self.cur);
                            self.buf.extend_from_slice(&rest);
                        }
                        _ => {
                            self.fatal_err(32);
                            return;
                        }
                    }
                }
            }
        }
        if has_encoding && !is_blank(self.raw()) {
            if self.starts(b"?>") {
                self.skip(2);
                return;
            }
            self.fatal_err(65);
        }
        self.skip_blanks();
        if self.starts(b"standalone") {
            self.skip(10);
            self.skip_blanks();
            if self.raw() == b'=' {
                self.skip(1);
                self.skip_blanks();
                let q = self.raw();
                if q == b'"' || q == b'\'' {
                    self.skip(1);
                    if self.starts(b"no") {
                        self.skip(2);
                    } else if self.starts(b"yes") {
                        self.standalone = true;
                        self.skip(3);
                    } else {
                        self.fatal_err(78);
                    }
                    if self.raw() != q {
                        self.fatal_err(34);
                    } else {
                        self.skip(1);
                    }
                } else {
                    self.fatal_err(33);
                }
            } else {
                self.fatal_err(75);
            }
        }
        self.skip_blanks();
        if self.starts(b"?>") {
            self.skip(2);
        } else if self.raw() == b'>' {
            self.fatal_err(57);
            self.skip(1);
        } else {
            // `MOVETO_ENDTAG` moves the cursor without counting columns.
            self.fatal_err(57);
            while self.cur < self.buf.len() && self.raw() != b'>' {
                self.cur += 1;
            }
            if self.raw() == b'>' {
                self.skip(1);
            }
        }
    }

    /// A quoted token of bytes satisfying `ok`.
    fn quoted(&mut self, ok: impl Fn(u8) -> bool) -> Option<Vec<u8>> {
        let q = self.raw();
        if q != b'"' && q != b'\'' {
            return None;
        }
        self.skip(1);
        let start = self.cur;
        while ok(self.raw()) && self.cur < self.buf.len() {
            self.skip(1);
        }
        let v = self.buf[start..self.cur].to_vec();
        if self.raw() == q {
            self.skip(1);
            Some(v)
        } else {
            None
        }
    }

    /// A system or public literal (any characters up to the quote).
    fn literal(&mut self) -> Option<Vec<u8>> {
        let q = self.raw();
        if q != b'"' && q != b'\'' {
            return None;
        }
        self.skip(1);
        let start = self.cur;
        while self.cur < self.buf.len() && self.raw() != q {
            let (_, l) = self.cur_char();
            self.nextl(l.max(1));
        }
        let v = self.buf[start..self.cur].to_vec();
        if self.raw() == q {
            self.skip(1);
        }
        Some(v)
    }

    /// `ExternalID`: (system, public).
    fn external_id(&mut self) -> (Option<Vec<u8>>, Option<Vec<u8>>) {
        if self.starts(b"SYSTEM") {
            self.skip(6);
            self.skip_blanks();
            return (self.literal(), None);
        }
        if self.starts(b"PUBLIC") {
            self.skip(6);
            self.skip_blanks();
            let public = self.literal();
            let save = (self.cur, self.line, self.col);
            self.skip_blanks();
            let system = if matches!(self.raw(), b'"' | b'\'') {
                self.literal()
            } else {
                (self.cur, self.line, self.col) = save;
                None
            };
            return (system, public);
        }
        (None, None)
    }

    /// `xmlParseDocTypeDecl`.
    fn doctype(&mut self) {
        self.skip(9);
        self.skip_blanks();
        if self.parse_name(false).is_none() {
            self.fatal_err(68);
        }
        self.skip_blanks();
        let (system, public) = self.external_id();
        if system.is_some() || public.is_some() {
            self.has_ext_subset = true;
        }
        self.skip_blanks();
        if self.raw() == b'[' {
            return;
        }
        if self.raw() != b'>' {
            self.fatal_err(61);
        }
        self.skip(1);
    }

    /// Whether the internal subset's closing `]>` has arrived (quotes and
    /// comments skipped).
    fn internal_subset_complete(&self) -> bool {
        let mut i = self.cur + 1;
        let mut quote = 0u8;
        while i < self.buf.len() {
            let b = self.buf[i];
            if quote != 0 {
                if b == quote {
                    quote = 0;
                }
            } else if b == b'"' || b == b'\'' {
                quote = b;
            } else if self.buf[i..].starts_with(b"<!--") {
                match self.find(i + 4, b"-->") {
                    Some(e) => i = e + 2,
                    None => return false,
                }
            } else if b == b']' {
                let mut j = i + 1;
                while j < self.buf.len() && is_blank(self.buf[j]) {
                    j += 1;
                }
                if j < self.buf.len() && self.buf[j] == b'>' {
                    return true;
                }
            }
            i += 1;
        }
        false
    }

    /// `xmlParseInternalSubset`.
    fn internal_subset(&mut self) {
        self.skip(1);
        loop {
            self.skip_blanks();
            if self.raw() == b']' || self.cur >= self.buf.len() {
                break;
            }
            let before = self.cur;
            if self.starts(b"<!ENTITY") {
                self.entity_decl();
            } else if self.starts(b"<!NOTATION") {
                self.notation_decl();
            } else if self.starts(b"<!ELEMENT") || self.starts(b"<!ATTLIST") {
                self.skip_decl();
            } else if self.starts(b"<!--") {
                self.comment();
            } else if self.starts(b"<?") {
                self.pi();
            } else if self.raw() == b'%' {
                self.has_perefs = true;
                self.skip(1);
                let _ = self.parse_name(false);
                if self.raw() == b';' {
                    self.skip(1);
                }
            }
            if self.state == St::Eof {
                return;
            }
            if self.cur == before {
                self.fatal_err(1);
                self.halt();
                return;
            }
        }
        if self.raw() == b']' {
            self.skip(1);
            self.skip_blanks();
        }
        if self.raw() != b'>' {
            self.fatal_err(61);
            return;
        }
        self.skip(1);
    }

    /// Skip a markup declaration to its `>`, quotes respected.
    fn skip_decl(&mut self) {
        let mut quote = 0u8;
        while self.cur < self.buf.len() {
            let b = self.raw();
            if quote != 0 {
                if b == quote {
                    quote = 0;
                }
            } else if b == b'"' || b == b'\'' {
                quote = b;
            } else if b == b'>' {
                self.skip(1);
                return;
            }
            let (_, l) = self.cur_char();
            self.nextl(l.max(1));
        }
    }

    /// `<!ENTITY ...>`.
    fn entity_decl(&mut self) {
        self.skip(8);
        self.skip_blanks();
        let mut parameter = false;
        if self.raw() == b'%' {
            parameter = true;
            self.skip(1);
            self.skip_blanks();
        }
        let Some(name) = self.parse_name(false) else {
            self.fatal_err(68);
            self.skip_decl();
            return;
        };
        self.skip_blanks();
        let def = if matches!(self.raw(), b'"' | b'\'') {
            let lit = self.literal().unwrap_or_default();
            Some(Entity::Internal(decode_char_refs(&lit)))
        } else {
            let (system, public) = self.external_id();
            self.skip_blanks();
            if self.starts(b"NDATA") {
                self.skip(5);
                self.skip_blanks();
                let notation = self.parse_name(false).unwrap_or_default();
                if !parameter {
                    self.skip_blanks();
                    let after = self.raw() == b'>';
                    if after {
                        self.skip(1);
                    }
                    self.emit(Ev::Unparsed(
                        name.clone(),
                        system.clone().unwrap_or_default(),
                        public,
                        notation,
                    ));
                    self.entities.entry(name).or_insert(Entity::Unparsed);
                    if !after {
                        self.skip_decl();
                    }
                    return;
                }
                Some(Entity::Unparsed)
            } else {
                system.map(|s| Entity::External(s, public))
            }
        };
        self.skip_blanks();
        if self.raw() == b'>' {
            self.skip(1);
        } else {
            self.skip_decl();
        }
        if let (false, Some(def)) = (parameter, def) {
            self.entities.entry(name).or_insert(def);
        }
    }

    /// `<!NOTATION ...>`.
    fn notation_decl(&mut self) {
        self.skip(10);
        self.skip_blanks();
        let name = self.parse_name(false).unwrap_or_default();
        self.skip_blanks();
        let (system, public) = self.external_id();
        self.skip_blanks();
        if self.raw() == b'>' {
            self.skip(1);
        } else {
            self.skip_decl();
        }
        self.emit(Ev::Notation(name, system, public));
    }

    // ---- markup ------------------------------------------------------------

    /// `xmlParseComment`: the event carries the whole `<!--…-->` text's
    /// content.
    fn comment(&mut self) {
        let start = self.cur + 4;
        let end = self.find(start, b"-->");
        let stop = end.unwrap_or(self.buf.len());
        // `--` inside is an error of its own.
        if let Some(i) = self.find(start, b"--") {
            if i < stop {
                self.fatal_err(80);
            }
        }
        let content = self.buf[start..stop].to_vec();
        let n = match end {
            Some(e) => e + 3 - self.cur,
            None => self.buf.len() - self.cur,
        };
        self.advance_chars(n);
        if end.is_none() {
            self.fatal_err(45);
            return;
        }
        self.emit(Ev::Comment(content));
    }

    /// Move over `n` bytes counting characters.
    fn advance_chars(&mut self, n: usize) {
        let stop = self.cur + n;
        while self.cur < stop {
            let (_, l) = self.cur_char();
            self.nextl(l.max(1));
        }
    }

    /// `xmlParsePI`.
    fn pi(&mut self) {
        self.skip(2);
        let Some(target) = self.parse_name(false) else {
            self.fatal_err(46);
            return;
        };
        if target.eq_ignore_ascii_case(b"xml") {
            self.fatal_err(64);
        }
        if self.starts(b"?>") {
            self.skip(2);
            self.emit(Ev::Pi(target, None));
            return;
        }
        if self.skip_blanks() == 0 {
            self.fatal_err(65);
        }
        let start = self.cur;
        match self.find(start, b"?>") {
            Some(end) => {
                let data = self.buf[start..end].to_vec();
                self.advance_chars(end - start + 2);
                self.emit(Ev::Pi(target, Some(data)));
            }
            None => {
                self.advance_chars(self.buf.len() - start);
                self.fatal_err(47);
            }
        }
    }

    /// `xmlParseStartTag` / `xmlParseStartTag2`: parse the tag up to its
    /// `>` or `/>` and emit the start event. `None` when the element name
    /// cannot be read (the caller halts).
    fn start_tag(&mut self) -> Option<(Vec<u8>, Vec<u8>, usize)> {
        self.skip(1);
        let (prefix, local, qname) = if self.ns_mode {
            let (p, l) = match self.parse_qname() {
                Some(pl) => pl,
                None => {
                    self.fatal_err(68);
                    return None;
                }
            };
            let q = match &p {
                Some(p) => [p.as_slice(), b":", l.as_slice()].concat(),
                None => l.clone(),
            };
            (p, l, q)
        } else {
            let Some(n) = self.parse_name(false) else {
                self.fatal_err(68);
                return None;
            };
            (None, n.clone(), n)
        };
        self.skip_blanks();
        let mut attrs: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
        while self.cur < self.buf.len()
            && self.raw() != b'>'
            && !(self.raw() == b'/' && self.nxt(1) == b'>')
            && self.state != St::Eof
        {
            let q = self.cur;
            let (name, value) = self.attribute();
            if let (Some(n), Some(v)) = (&name, &value) {
                if attrs.iter().any(|(k, _)| k == n) {
                    self.fatal_err(42);
                } else {
                    attrs.push((n.clone(), v.clone()));
                }
            }
            if self.raw() == b'>' || (self.raw() == b'/' && self.nxt(1) == b'>') {
                break;
            }
            if self.skip_blanks() == 0 {
                self.fatal_err(65);
            }
            if q == self.cur && name.is_none() && value.is_none() {
                self.fatal_err(1);
                break;
            }
        }
        if !self.ns_mode {
            self.emit(Ev::Start {
                name: qname.clone(),
                attrs,
                ns: Vec::new(),
            });
            return Some((qname.clone(), qname, 0));
        }
        // Namespace processing (SAX2).
        let mut decls: Vec<(Option<Vec<u8>>, Vec<u8>)> = Vec::new();
        let mut plain: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
        for (k, v) in attrs {
            if k.as_slice() == b"xmlns" {
                decls.push((None, v));
            } else if let Some(p) = k.strip_prefix(b"xmlns:") {
                if v.is_empty() {
                    self.soft_err(200);
                    continue;
                }
                decls.push((Some(p.to_vec()), v));
            } else {
                plain.push((k, v));
            }
        }
        for d in &decls {
            self.ns.push(d.clone());
        }
        let uri = self.lookup(prefix.as_deref());
        if prefix.is_some() && uri.is_none() {
            self.soft_err(201);
        }
        let name = self.qualify(&local, uri.as_deref());
        let mut out = Vec::new();
        let mut expanded: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
        for (k, v) in plain {
            let (p, l) = match k.iter().position(|&b| b == b':') {
                Some(i) => (Some(k[..i].to_vec()), k[i + 1..].to_vec()),
                None => (None, k.clone()),
            };
            match p {
                None => out.push((l, v)),
                Some(p) => match self.lookup(Some(&p)) {
                    Some(u) => {
                        let q = self.qualify(&l, Some(&u));
                        let dup = expanded.iter().any(|(eu, el): &(Vec<u8>, Vec<u8>)| *eu == u && *el == l);
                        if dup {
                            self.soft_err(203);
                        }
                        expanded.push((u, l));
                        out.push((q, v));
                    }
                    None => {
                        self.soft_err(201);
                        out.push((l, v));
                    }
                },
            }
        }
        let nsn = decls.len();
        self.emit(Ev::Start {
            name: name.clone(),
            attrs: out,
            ns: decls,
        });
        Some((qname, name, nsn))
    }

    fn lookup(&self, prefix: Option<&[u8]>) -> Option<Vec<u8>> {
        for (p, u) in self.ns.iter().rev() {
            if p.as_deref() == prefix {
                return if u.is_empty() { None } else { Some(u.clone()) };
            }
        }
        None
    }

    fn qualify(&self, local: &[u8], uri: Option<&[u8]>) -> Vec<u8> {
        match uri {
            Some(u) => [u, self.sep.as_slice(), local].concat(),
            None => local.to_vec(),
        }
    }

    /// `xmlParseAttribute`: (name, value) — the name `None` when it could
    /// not be read or has no `=`, the value `None` when it is not quoted.
    fn attribute(&mut self) -> (Option<Vec<u8>>, Option<Vec<u8>>) {
        let Some(name) = self.parse_name(false) else {
            self.fatal_err(68);
            return (None, None);
        };
        self.skip_blanks();
        if self.raw() != b'=' {
            self.fatal_err(41);
            return (None, None);
        }
        self.skip(1);
        self.skip_blanks();
        let value = self.att_value();
        (Some(name), value)
    }

    /// `xmlParseAttValueComplex`: the normalized value, references
    /// replaced.
    fn att_value(&mut self) -> Option<Vec<u8>> {
        let q = self.raw();
        if q != b'"' && q != b'\'' {
            self.fatal_err(39);
            return None;
        }
        self.skip(1);
        let mut out = Vec::new();
        loop {
            let b = self.raw();
            if self.cur >= self.buf.len() || b == q || b == b'<' {
                break;
            }
            if b == b'&' {
                if self.nxt(1) == b'#' {
                    let v = self.char_ref();
                    if v != 0 {
                        push_utf8(&mut out, v);
                    }
                } else {
                    self.skip(1);
                    let Some(name) = self.parse_name(false) else {
                        self.fatal_err(68);
                        continue;
                    };
                    if self.raw() != b';' {
                        self.fatal_err(23);
                        continue;
                    }
                    self.skip(1);
                    if let Some(c) = predefined(&name) {
                        out.push(c);
                    } else {
                        match self.entities.get(&name).cloned() {
                            Some(Entity::Internal(v)) => {
                                for &c in &v {
                                    out.push(if is_blank(c) { b' ' } else { c });
                                }
                            }
                            Some(_) => self.fatal_err(16),
                            None => {
                                if self.standalone || (!self.has_ext_subset && !self.has_perefs) {
                                    self.fatal_err(26);
                                } else {
                                    self.soft_err(27);
                                }
                            }
                        }
                    }
                }
                continue;
            }
            let (c, l) = self.cur_char();
            if !is_xml_char(c) {
                break;
            }
            if c == 0x20 || c == 0x9 || c == 0xA || c == 0xD {
                out.push(b' ');
            } else {
                push_utf8(&mut out, c);
            }
            self.nextl(l);
        }
        let b = self.raw();
        if b == b'<' {
            self.fatal_err(38);
        } else if b != q || self.cur >= self.buf.len() {
            let (c, _) = if self.cur < self.buf.len() { self.cur_char() } else { (0, 0) };
            if c != 0 && !is_xml_char(c) {
                self.fatal_err(9);
            } else {
                self.fatal_err(40);
            }
        } else {
            self.skip(1);
        }
        Some(out)
    }

    /// `xmlParseCharRef`: the value, 0 on an error (raised).
    fn char_ref(&mut self) -> u32 {
        let mut val: u32 = 0;
        let mut bad = false;
        if self.starts(b"&#x") {
            self.skip(3);
            while self.raw() != b';' {
                let d = self.raw();
                let Some(v) = (d as char).to_digit(16) else {
                    self.fatal_err(6);
                    bad = true;
                    break;
                };
                val = val.saturating_mul(16).saturating_add(v).min(0x110000);
                self.skip(1);
            }
        } else if self.starts(b"&#") {
            self.skip(2);
            while self.raw() != b';' {
                let d = self.raw();
                let Some(v) = (d as char).to_digit(10) else {
                    self.fatal_err(7);
                    bad = true;
                    break;
                };
                val = val.saturating_mul(10).saturating_add(v).min(0x110000);
                self.skip(1);
            }
        } else {
            self.fatal_err(8);
            return 0;
        }
        // A malformed reference reads as 0, which the character check
        // then rejects too (libxml raises both).
        if bad {
            val = 0;
        }
        if self.raw() == b';' {
            self.skip(1);
        }
        if is_xml_char(val) {
            return val;
        }
        self.fatal_err(9);
        0
    }

    /// `xmlParseReference` in content.
    fn reference(&mut self) {
        if self.nxt(1) == b'#' {
            let v = self.char_ref();
            if v != 0 {
                let mut s = Vec::new();
                push_utf8(&mut s, v);
                self.emit(Ev::Chars(s));
            }
            return;
        }
        self.skip(1);
        let Some(name) = self.parse_name(false) else {
            self.fatal_err(68);
            return;
        };
        if self.raw() != b';' {
            self.fatal_err(23);
            return;
        }
        self.skip(1);
        if let Some(c) = predefined(&name) {
            self.emit(Ev::Chars(vec![c]));
            return;
        }
        match self.entities.get(&name).cloned() {
            Some(Entity::Internal(v)) => self.emit(Ev::EntityRef(name, Some(v))),
            Some(Entity::External(s, p)) => self.emit(Ev::ExternalRef(name, s, p)),
            Some(Entity::Unparsed) => self.fatal_err(28),
            None => {
                self.emit(Ev::EntityRef(name, None));
                if self.standalone || (!self.has_ext_subset && !self.has_perefs) {
                    self.fatal_err(26);
                } else {
                    self.soft_err(27);
                }
            }
        }
    }

    /// `xmlParseEndTag1` / `xmlParseEndTag2`.
    fn end_tag(&mut self) {
        self.skip(2);
        let (qname, name, nsn) = self.stack.pop().unwrap_or_default();
        // `xmlParseNameAndCompare`: the columns of a matching prefix are
        // counted before a mismatch is re-read as a whole name.
        let mut i = 0;
        while i < qname.len() && self.at(self.cur + i) == qname[i] {
            i += 1;
        }
        let matched = i == qname.len() && {
            let b = self.at(self.cur + i);
            b == b'>' || is_blank(b)
        };
        if matched {
            self.cur += i;
            self.col += i as i64;
        } else {
            self.col += i as i64;
        }
        let other = if matched {
            None
        } else {
            match self.parse_name(false) {
                Some(n) if n == qname => None,
                n => Some(n),
            }
        };
        self.skip_blanks();
        if self.raw() != b'>' {
            self.fatal_err(73);
        } else {
            self.skip(1);
        }
        if other.is_some() {
            self.fatal_err(76);
        }
        self.emit(Ev::End(name));
        self.ns.truncate(self.ns.len().saturating_sub(nsn));
    }

    /// `xmlParseCharData(ctxt, 0)`: the fast ASCII path, then
    /// [`Sax::char_data_complex`].
    fn char_data(&mut self) {
        let mut inp = self.cur;
        let mut saved = (self.line, self.col);
        loop {
            // get_more_space
            loop {
                while self.at(inp) == b' ' {
                    inp += 1;
                    self.col += 1;
                }
                if self.at(inp) == b'\n' {
                    while self.at(inp) == b'\n' {
                        self.line += 1;
                        self.col = 1;
                        inp += 1;
                    }
                    continue;
                }
                break;
            }
            if self.at(inp) == b'<' {
                if inp > self.cur {
                    let text = self.buf[self.cur..inp].to_vec();
                    self.cur = inp;
                    self.emit(Ev::Chars(text));
                }
                return;
            }
            // get_more
            loop {
                while test_char_data(self.at(inp)) {
                    inp += 1;
                    self.col += 1;
                }
                if self.at(inp) == b'\n' {
                    while self.at(inp) == b'\n' {
                        self.line += 1;
                        self.col = 1;
                        inp += 1;
                    }
                    continue;
                }
                if self.at(inp) == b']' {
                    if self.at(inp + 1) == b']' && self.at(inp + 2) == b'>' {
                        self.fatal_err(62);
                        self.cur = inp + 1;
                        return;
                    }
                    inp += 1;
                    self.col += 1;
                    continue;
                }
                break;
            }
            if inp > self.cur {
                let text = self.buf[self.cur..inp].to_vec();
                if is_blank(self.buf[self.cur]) {
                    self.cur = inp;
                    self.emit(Ev::Chars(text));
                } else {
                    let p = Pos {
                        line: self.line,
                        col: self.col,
                        byte: (self.cur - self.byte_base) as i64,
                    };
                    self.emit_at(Ev::Chars(text), p);
                }
                saved = (self.line, self.col);
            }
            self.cur = inp;
            if self.at(inp) == b'\r' && self.at(inp + 1) == b'\n' {
                self.cur = inp + 1;
                inp += 2;
                self.line += 1;
                self.col = 1;
                let b = self.at(inp);
                if (0x20..=0x7F).contains(&b) || b == 0x09 || b == 0x0A {
                    continue;
                }
                break;
            }
            let b = self.at(inp);
            if b == b'<' || b == b'&' {
                return;
            }
            inp = self.cur;
            let b = self.at(inp);
            if !((0x20..=0x7F).contains(&b) || b == 0x09 || b == 0x0A) {
                break;
            }
        }
        (self.line, self.col) = saved;
        self.char_data_complex();
    }

    /// `xmlParseCharDataComplex`: character by character, delivered in
    /// 300-byte pieces.
    fn char_data_complex(&mut self) {
        let mut text = Vec::new();
        let (mut c, mut l) = self.cur_char();
        while c != u32::from(b'<') && c != u32::from(b'&') && is_xml_char(c) {
            if c == u32::from(b']') && self.nxt(1) == b']' && self.nxt(2) == b'>' {
                self.fatal_err(62);
            }
            push_utf8(&mut text, c);
            self.nextl(l);
            (c, l) = self.cur_char();
            if text.len() >= BIG {
                let t = std::mem::take(&mut text);
                self.emit(Ev::Chars(t));
            }
        }
        if !text.is_empty() {
            self.emit(Ev::Chars(text));
        }
        if c != 0 && !is_xml_char(c) {
            self.fatal_err(9);
            self.nextl(l.max(1));
        }
    }
}

fn predefined(name: &[u8]) -> Option<u8> {
    Some(match name {
        b"lt" => b'<',
        b"gt" => b'>',
        b"amp" => b'&',
        b"apos" => b'\'',
        b"quot" => b'"',
        _ => return None,
    })
}

/// An entity value's replacement text: character references expanded,
/// entity references kept.
fn decode_char_refs(lit: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < lit.len() {
        if lit[i..].starts_with(b"&#") {
            if let Some(end) = lit[i..].iter().position(|&b| b == b';') {
                let body = &lit[i + 2..i + end];
                let v = if let Some(h) = body.strip_prefix(b"x") {
                    u32::from_str_radix(std::str::from_utf8(h).unwrap_or(""), 16).ok()
                } else {
                    std::str::from_utf8(body).ok().and_then(|s| s.parse().ok())
                };
                if let Some(v) = v {
                    push_utf8(&mut out, v);
                    i += end + 1;
                    continue;
                }
            }
        }
        out.push(lit[i]);
        i += 1;
    }
    out
}
