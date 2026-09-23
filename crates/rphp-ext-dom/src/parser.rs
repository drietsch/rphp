//! An XML 1.0 parser building a [`DocData`] tree, with namespaces, the
//! internal DTD subset's entity declarations, and libxml's diagnostics —
//! their codes, levels and messages — so `libxml_get_errors()` reports what
//! php's would. Byte-oriented; input is taken as UTF-8 (an ISO-8859-1
//! declaration is transcoded, anything else is passed through as bytes).
//!
//! Recovery follows libxml's `XML_PARSE_RECOVER` in outline: a fatal error
//! ends the parse (the document loads only under `recover`), a namespace
//! error is reported and the element keeps its prefix without a namespace.

use crate::tree::{
    is_name_char, is_name_start, split_qname, DocData, DocTypeInfo, DtdDecl, EntityDecl, NodeId,
    NodeKind, NsDecl, DOCUMENT,
};

/// One libxml diagnostic (`LibXMLError`).
#[derive(Clone, Debug)]
pub struct XmlError {
    /// 1 warning, 2 error, 3 fatal.
    pub level: i64,
    pub code: i64,
    pub line: i64,
    pub column: i64,
    /// With libxml's trailing newline.
    pub message: String,
}

pub struct Options {
    pub substitute_entities: bool,
    pub recover: bool,
    pub preserve_white_space: bool,
    /// `LIBXML_NOBLANKS`.
    pub no_blanks: bool,
    /// `LIBXML_NOCDATA`: CDATA sections become text nodes.
    pub no_cdata: bool,
}

pub struct Outcome {
    pub errors: Vec<XmlError>,
    /// Whether the document is usable (no fatal error, or `recover`).
    pub ok: bool,
}

struct Parser<'a> {
    src: &'a [u8],
    pos: usize,
    line: i64,
    line_start: usize,
    doc: &'a mut DocData,
    opts: &'a Options,
    errors: Vec<XmlError>,
    fatal: bool,
    /// Declared internal entities, for reference resolution while parsing.
    entities: Vec<(Vec<u8>, Vec<u8>)>,
    /// Namespace scopes: one map per open element.
    ns_stack: Vec<Vec<NsDecl>>,
    /// libxml reports "Premature end of data" for the innermost open tag
    /// only.
    eof_reported: bool,
}

/// Parse `src` into `doc` (which must be a fresh document).
pub fn parse(doc: &mut DocData, src: &[u8], opts: &Options) -> Outcome {
    let src = src.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(src);
    let transcoded;
    let src: &[u8] = if declares_latin1(src) {
        transcoded = latin1_to_utf8(src);
        &transcoded
    } else {
        src
    };
    let mut p = Parser {
        src,
        pos: 0,
        line: 1,
        line_start: 0,
        doc,
        opts,
        errors: Vec::new(),
        fatal: false,
        entities: Vec::new(),
        ns_stack: Vec::new(),
        eof_reported: false,
    };
    p.document();
    let ok = !p.fatal || opts.recover;
    Outcome {
        errors: p.errors,
        ok,
    }
}

fn declares_latin1(src: &[u8]) -> bool {
    let head = &src[..src.len().min(200)];
    let lower = head.to_ascii_lowercase();
    lower.starts_with(b"<?xml")
        && (contains(&lower, b"encoding=\"iso-8859-1\"")
            || contains(&lower, b"encoding='iso-8859-1'"))
}

fn contains(h: &[u8], n: &[u8]) -> bool {
    h.windows(n.len()).any(|w| w == n)
}

fn latin1_to_utf8(src: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(src.len());
    for &b in src {
        if b < 0x80 {
            out.push(b);
        } else {
            out.push(0xC0 | (b >> 6));
            out.push(0x80 | (b & 0x3F));
        }
    }
    out
}

impl<'a> Parser<'a> {
    // ---- input -----------------------------------------------------------

    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    fn peek_at(&self, off: usize) -> Option<u8> {
        self.src.get(self.pos + off).copied()
    }

    fn starts_with(&self, s: &[u8]) -> bool {
        self.src[self.pos..].starts_with(s)
    }

    fn bump(&mut self) -> Option<u8> {
        let b = self.peek()?;
        self.pos += 1;
        if b == b'\n' {
            self.line += 1;
            self.line_start = self.pos;
        }
        Some(b)
    }

    fn advance(&mut self, n: usize) {
        for _ in 0..n {
            self.bump();
        }
    }

    fn column(&self) -> i64 {
        (self.pos - self.line_start) as i64 + 1
    }

    fn skip_ws(&mut self) -> bool {
        let start = self.pos;
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
            self.bump();
        }
        self.pos > start
    }

    fn at_end(&self) -> bool {
        self.pos >= self.src.len()
    }

    // ---- diagnostics -----------------------------------------------------

    fn error(&mut self, level: i64, code: i64, message: String) {
        let column = self.column();
        self.errors.push(XmlError {
            level,
            code,
            line: self.line,
            column,
            message: format!("{message}\n"),
        });
    }

    fn error_at(&mut self, level: i64, code: i64, column: i64, message: String) {
        self.errors.push(XmlError {
            level,
            code,
            line: self.line,
            column,
            message: format!("{message}\n"),
        });
    }

    fn fatal(&mut self, code: i64, message: String) {
        self.error(3, code, message);
        self.fatal = true;
    }

    /// libxml's two follow-up errors when a start tag cannot be completed;
    /// the parse goes on from where it stopped (the caller returns).
    fn start_tag_failed(&mut self, qname: &[u8], line: i64) {
        self.fatal(65, "attributes construct error".to_string());
        self.fatal(
            73,
            format!(
                "Couldn't find end of Start Tag {} line {line}",
                String::from_utf8_lossy(qname)
            ),
        );
    }

    // ---- names -----------------------------------------------------------

    fn name(&mut self) -> Option<Vec<u8>> {
        let start = self.pos;
        let rest = std::str::from_utf8(&self.src[self.pos..]).unwrap_or("");
        let mut chars = rest.char_indices();
        let Some((_, first)) = chars.next() else {
            return None;
        };
        if !is_name_start(first) {
            return None;
        }
        let mut end = first.len_utf8();
        for (i, c) in chars {
            if !is_name_char(c) {
                end = i;
                break;
            }
            end = i + c.len_utf8();
        }
        self.advance(end);
        Some(self.src[start..self.pos].to_vec())
    }

    // ---- document --------------------------------------------------------

    fn document(&mut self) {
        if self.at_end() {
            self.fatal(4, "Document is empty".to_string());
            return;
        }
        if !self.src.iter().any(|b| !b.is_ascii_whitespace()) {
            self.skip_ws();
            self.fatal(4, "Start tag expected, '<' not found".to_string());
            return;
        }
        if self.starts_with(b"<?xml")
            && matches!(self.peek_at(5), Some(b' ' | b'\t' | b'\r' | b'\n' | b'?'))
        {
            self.xml_decl();
            if self.fatal {
                return;
            }
        }
        self.misc();
        if self.starts_with(b"<!DOCTYPE") {
            self.doctype();
            if self.fatal {
                return;
            }
            self.misc();
        }
        if self.peek() == Some(b'<') && !self.starts_with(b"<!") && !self.starts_with(b"<?") {
            self.element(DOCUMENT);
        } else {
            self.fatal(4, "Start tag expected, '<' not found".to_string());
            return;
        }
        // libxml keeps scanning after most errors, so what follows a
        // broken element is reported as it would be after a good one.
        self.misc();
        if !self.at_end() {
            self.fatal(5, "Extra content at the end of the document".to_string());
        }
    }

    /// Comments, PIs and whitespace at document level.
    fn misc(&mut self) {
        loop {
            self.skip_ws();
            if self.starts_with(b"<!--") {
                if let Some(id) = self.comment() {
                    self.doc.link_last(DOCUMENT, id);
                }
            } else if self.starts_with(b"<?") {
                if let Some(id) = self.pi() {
                    self.doc.link_last(DOCUMENT, id);
                }
            } else {
                return;
            }
        }
    }

    fn xml_decl(&mut self) {
        self.advance(5);
        let mut version = None;
        let mut encoding = None;
        let mut standalone = None;
        loop {
            let had_ws = self.skip_ws();
            if self.starts_with(b"?>") {
                self.advance(2);
                break;
            }
            if self.at_end() {
                self.fatal(80, "parsing XML declaration: '?>' expected".to_string());
                return;
            }
            if !had_ws {
                self.fatal(65, "Blank needed here".to_string());
                return;
            }
            let Some(key) = self.name() else {
                self.fatal(80, "parsing XML declaration: '?>' expected".to_string());
                return;
            };
            self.skip_ws();
            if self.peek() != Some(b'=') {
                self.fatal(96, "Malformed declaration expecting version".to_string());
                return;
            }
            self.bump();
            self.skip_ws();
            let Some(v) = self.quoted() else {
                self.fatal(96, "Malformed declaration expecting version".to_string());
                return;
            };
            match key.as_slice() {
                b"version" => version = Some(v),
                b"encoding" => encoding = Some(v),
                b"standalone" => standalone = Some(v == b"yes"),
                _ => {
                    self.fatal(80, "parsing XML declaration: '?>' expected".to_string());
                    return;
                }
            }
        }
        if version.is_none() {
            self.fatal(96, "Malformed declaration expecting version".to_string());
            return;
        }
        self.doc.version = version;
        self.doc.encoding = encoding;
        self.doc.standalone = standalone;
    }

    fn quoted(&mut self) -> Option<Vec<u8>> {
        let q = self.peek()?;
        if q != b'"' && q != b'\'' {
            return None;
        }
        self.bump();
        let start = self.pos;
        while let Some(b) = self.peek() {
            if b == q {
                let v = self.src[start..self.pos].to_vec();
                self.bump();
                return Some(v);
            }
            self.bump();
        }
        None
    }

    fn doctype(&mut self) {
        let line = self.line;
        self.advance(9);
        self.skip_ws();
        let Some(name) = self.name() else {
            self.fatal(68, "xmlParseDocTypeDecl : no DOCTYPE name !".to_string());
            return;
        };
        let mut info = DocTypeInfo::default();
        self.skip_ws();
        if self.starts_with(b"PUBLIC") {
            self.advance(6);
            self.skip_ws();
            info.public_id = self.quoted().unwrap_or_default();
            self.skip_ws();
            info.system_id = self.quoted().unwrap_or_default();
        } else if self.starts_with(b"SYSTEM") {
            self.advance(6);
            self.skip_ws();
            info.system_id = self.quoted().unwrap_or_default();
        }
        self.skip_ws();
        if self.peek() == Some(b'[') {
            self.bump();
            let start = self.pos;
            self.internal_subset(&mut info);
            if self.fatal {
                return;
            }
            info.internal_subset = Some(self.src[start..self.pos].to_vec());
            if self.peek() == Some(b']') {
                self.bump();
            }
            self.skip_ws();
        }
        if self.peek() == Some(b'>') {
            self.bump();
        } else {
            self.fatal(78, "DOCTYPE improperly terminated".to_string());
            return;
        }
        self.entities = info.entities.clone();
        let id = self.doc.create_doctype(&name, info);
        self.doc.nodes[id].line = line as u32;
        self.doc.link_last(DOCUMENT, id);
    }

    /// The internal subset up to (not including) the closing `]`.
    fn internal_subset(&mut self, info: &mut DocTypeInfo) {
        loop {
            self.skip_ws();
            match self.peek() {
                None => {
                    self.fatal(78, "DOCTYPE improperly terminated".to_string());
                    return;
                }
                Some(b']') => return,
                Some(b'%') => {
                    // A parameter entity reference: skipped.
                    while let Some(b) = self.bump() {
                        if b == b';' {
                            break;
                        }
                    }
                }
                _ if self.starts_with(b"<!--") => {
                    if let Some(id) = self.comment() {
                        info.decls
                            .push(DtdDecl::Comment(self.doc.nodes[id].value.clone()));
                    }
                }
                _ if self.starts_with(b"<?") => {
                    if let Some(id) = self.pi() {
                        let n = &self.doc.nodes[id];
                        info.decls.push(DtdDecl::Pi {
                            target: n.name.clone(),
                            data: n.value.clone(),
                        });
                    }
                }
                _ if self.starts_with(b"<!ENTITY") => {
                    self.advance(8);
                    self.skip_ws();
                    let parameter = self.peek() == Some(b'%');
                    if parameter {
                        self.bump();
                        self.skip_ws();
                    }
                    let Some(name) = self.name() else {
                        self.fatal(68, "xmlParseEntityDecl: no name".to_string());
                        return;
                    };
                    self.skip_ws();
                    if matches!(self.peek(), Some(b'"' | b'\'')) {
                        let orig = self.quoted().unwrap_or_default();
                        let v = self.entity_literal(&orig);
                        let decl = EntityDecl {
                            name: name.clone(),
                            value: Some(v.clone()),
                            orig,
                            ..EntityDecl::default()
                        };
                        info.decls.push(DtdDecl::Entity {
                            decl: decl.clone(),
                            parameter,
                        });
                        if !parameter {
                            info.entity_decls.push(decl);
                            info.entities.push((name, v));
                        }
                    } else {
                        // SYSTEM/PUBLIC: external, not loaded; recorded.
                        let mut decl = EntityDecl {
                            name: name.clone(),
                            ..EntityDecl::default()
                        };
                        if self.starts_with(b"PUBLIC") {
                            self.advance(6);
                            self.skip_ws();
                            decl.public_id = Some(self.quoted().unwrap_or_default());
                            self.skip_ws();
                            decl.system_id = Some(self.quoted().unwrap_or_default());
                        } else if self.starts_with(b"SYSTEM") {
                            self.advance(6);
                            self.skip_ws();
                            decl.system_id = Some(self.quoted().unwrap_or_default());
                        }
                        self.skip_ws();
                        if self.starts_with(b"NDATA") {
                            self.advance(5);
                            self.skip_ws();
                            decl.notation = self.name();
                        }
                        while let Some(b) = self.peek() {
                            if b == b'>' {
                                break;
                            }
                            self.bump();
                        }
                        info.decls.push(DtdDecl::Entity {
                            decl: decl.clone(),
                            parameter,
                        });
                        if !parameter {
                            info.entity_decls.push(decl);
                        }
                    }
                    self.skip_ws();
                    if self.peek() == Some(b'>') {
                        self.bump();
                    } else {
                        self.fatal(73, "xmlParseEntityDecl: entity not terminated".to_string());
                        return;
                    }
                }
                _ if self.starts_with(b"<!NOTATION") => {
                    self.advance(10);
                    self.skip_ws();
                    let name = self.name().unwrap_or_default();
                    self.skip_ws();
                    let (mut public_id, mut system_id) = (Vec::new(), Vec::new());
                    if self.starts_with(b"PUBLIC") {
                        self.advance(6);
                        self.skip_ws();
                        public_id = self.quoted().unwrap_or_default();
                        self.skip_ws();
                        if matches!(self.peek(), Some(b'"' | b'\'')) {
                            system_id = self.quoted().unwrap_or_default();
                        }
                    } else if self.starts_with(b"SYSTEM") {
                        self.advance(6);
                        self.skip_ws();
                        system_id = self.quoted().unwrap_or_default();
                    }
                    while let Some(b) = self.peek() {
                        if b == b'>' {
                            break;
                        }
                        self.bump();
                    }
                    self.bump();
                    info.notations.push((name, public_id, system_id));
                }
                _ if self.starts_with(b"<!ELEMENT") || self.starts_with(b"<!ATTLIST") => {
                    let element_decl = self.starts_with(b"<!ELEMENT");
                    self.advance(9);
                    self.skip_ws();
                    let name = self.name().unwrap_or_default();
                    self.skip_ws();
                    // The declaration's body up to its `>` (quotes respected).
                    let start = self.pos;
                    let mut quote: Option<u8> = None;
                    while let Some(b) = self.peek() {
                        match quote {
                            Some(q) if b == q => quote = None,
                            Some(_) => {}
                            None if b == b'"' || b == b'\'' => quote = Some(b),
                            None if b == b'>' => break,
                            None => {}
                        }
                        self.bump();
                    }
                    let body = self.src[start..self.pos].to_vec();
                    self.bump();
                    if element_decl {
                        info.decls.push(DtdDecl::Element {
                            name,
                            content: normalize_content_model(&body),
                        });
                    } else {
                        info.decls.push(DtdDecl::Attlist {
                            element: name,
                            attrs: parse_attlist(&body),
                        });
                    }
                }
                _ => {
                    self.fatal(78, "DOCTYPE improperly terminated".to_string());
                    return;
                }
            }
            if self.fatal {
                return;
            }
        }
    }

    /// An entity literal with character references resolved (general entity
    /// references stay, as in libxml).
    fn entity_literal(&self, v: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < v.len() {
            if v[i] == b'&' && v.get(i + 1) == Some(&b'#') {
                if let Some(end) = v[i..].iter().position(|&b| b == b';') {
                    if let Some(c) = char_ref(&v[i + 2..i + end]) {
                        let mut buf = [0u8; 4];
                        out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                        i += end + 1;
                        continue;
                    }
                }
            }
            out.push(v[i]);
            i += 1;
        }
        out
    }

    // ---- content ---------------------------------------------------------

    fn comment(&mut self) -> Option<NodeId> {
        let line = self.line;
        self.advance(4);
        let start = self.pos;
        loop {
            if self.at_end() {
                let shown =
                    String::from_utf8_lossy(&self.src[start..(start + 50).min(self.src.len())])
                        .into_owned();
                self.fatal(57, format!("Comment not terminated \n<!--{shown}"));
                return None;
            }
            if self.starts_with(b"-->") {
                let v = self.src[start..self.pos].to_vec();
                self.advance(3);
                let id = self.doc.create_text(NodeKind::Comment, &v);
                self.doc.nodes[id].line = line as u32;
                return Some(id);
            }
            if self.starts_with(b"--") {
                let shown = String::from_utf8_lossy(&self.src[start..self.pos]).into_owned();
                self.fatal(80, format!("Double hyphen within comment: <!--{shown}"));
                // libxml goes on to the closing `-->`; the node is not built.
                while !self.at_end() && !self.starts_with(b"-->") {
                    self.bump();
                }
                if self.starts_with(b"-->") {
                    self.advance(3);
                }
                return None;
            }
            self.bump();
        }
    }

    fn pi(&mut self) -> Option<NodeId> {
        let line = self.line;
        self.advance(2);
        let Some(target) = self.name() else {
            self.fatal(66, "xmlParsePI : no target name".to_string());
            return None;
        };
        if target.eq_ignore_ascii_case(b"xml") {
            self.fatal(
                64,
                "XML declaration allowed only at the start of the document".to_string(),
            );
            return None;
        }
        self.skip_ws();
        let start = self.pos;
        loop {
            if self.at_end() {
                self.fatal(
                    65,
                    format!(
                        "ParsePI: PI {} never end ...",
                        String::from_utf8_lossy(&target)
                    ),
                );
                return None;
            }
            if self.starts_with(b"?>") {
                let v = self.src[start..self.pos].to_vec();
                self.advance(2);
                let id = self.doc.create_pi(&target, &v);
                self.doc.nodes[id].line = line as u32;
                return Some(id);
            }
            self.bump();
        }
    }

    fn cdata(&mut self) -> Option<NodeId> {
        let line = self.line;
        self.advance(9);
        let start = self.pos;
        loop {
            if self.at_end() {
                let shown =
                    String::from_utf8_lossy(&self.src[start..(start + 50).min(self.src.len())])
                        .into_owned();
                self.fatal(63, format!("CData section not finished\n{shown}"));
                return None;
            }
            if self.starts_with(b"]]>") {
                let v = self.src[start..self.pos].to_vec();
                self.advance(3);
                let kind = if self.opts.no_cdata {
                    NodeKind::Text
                } else {
                    NodeKind::CData
                };
                let id = self.doc.create_text(kind, &v);
                self.doc.nodes[id].line = line as u32;
                return Some(id);
            }
            self.bump();
        }
    }

    /// `<name attrs>content</name>` at the current position, linked under
    /// `parent`.
    fn element(&mut self, parent: NodeId) {
        let line = self.line;
        self.bump(); // '<'
        let Some(qname) = self.name() else {
            self.fatal(68, "StartTag: invalid element name".to_string());
            return;
        };
        let (prefix, local) = split_qname(&qname);
        let (prefix, local) = (prefix.map(<[u8]>::to_vec), local.to_vec());
        // Attributes and namespace declarations.
        let mut attrs: Vec<(Vec<u8>, Vec<u8>, u32)> = Vec::new();
        let mut decls: Vec<NsDecl> = Vec::new();
        let mut self_closing = false;
        let tag_end_col;
        loop {
            let had_ws = self.skip_ws();
            match self.peek() {
                Some(b'>') => {
                    tag_end_col = self.column();
                    self.bump();
                    break;
                }
                Some(b'/') if self.peek_at(1) == Some(b'>') => {
                    tag_end_col = self.column();
                    self.advance(2);
                    self_closing = true;
                    break;
                }
                None => {
                    self.fatal(
                        73,
                        format!(
                            "Couldn't find end of Start Tag {} line {line}",
                            String::from_utf8_lossy(&qname)
                        ),
                    );
                    return;
                }
                _ => {}
            }
            if !had_ws {
                self.start_tag_failed(&qname, line);
                return;
            }
            let aline = self.line;
            let Some(aname) = self.name() else {
                self.start_tag_failed(&qname, line);
                return;
            };
            self.skip_ws();
            if self.peek() != Some(b'=') {
                self.fatal(
                    41,
                    format!(
                        "Specification mandates value for attribute {}",
                        String::from_utf8_lossy(&aname)
                    ),
                );
                self.start_tag_failed(&qname, line);
                return;
            }
            self.bump();
            self.skip_ws();
            let Some(value) = self.attr_value() else {
                self.start_tag_failed(&qname, line);
                return;
            };
            if aname == b"xmlns" {
                // libxml: a namespace name should be an absolute URI.
                if !value.is_empty()
                    && !value.iter().take_while(|b| **b != b'/').any(|&b| b == b':')
                {
                    let col = self.column();
                    self.error_at(
                        1,
                        100,
                        col,
                        format!(
                            "xmlns: URI {} is not absolute",
                            String::from_utf8_lossy(&value)
                        ),
                    );
                }
            }
            if aname == b"xmlns" {
                decls.push(NsDecl {
                    prefix: None,
                    uri: value,
                });
            } else if let Some(p) = aname.strip_prefix(b"xmlns:") {
                decls.push(NsDecl {
                    prefix: Some(p.to_vec()),
                    uri: value,
                });
            } else {
                if attrs.iter().any(|(n, _, _)| *n == aname) {
                    self.fatal(
                        42,
                        format!("Attribute {} redefined", String::from_utf8_lossy(&aname)),
                    );
                    return;
                }
                attrs.push((aname, value, aline as u32));
            }
        }
        self.ns_stack.push(decls.clone());
        let ns = match &prefix {
            Some(p) => {
                let uri = self.resolve_ns(Some(p));
                if uri.is_none() {
                    self.error_at(
                        2,
                        201,
                        tag_end_col,
                        format!(
                            "Namespace prefix {} on {} is not defined",
                            String::from_utf8_lossy(p),
                            String::from_utf8_lossy(&local)
                        ),
                    );
                }
                uri
            }
            None => self.resolve_ns(None).filter(|u| !u.is_empty()),
        };
        let id = self
            .doc
            .create_element(&local, prefix.as_deref(), ns.as_deref());
        self.doc.nodes[id].line = line as u32;
        self.doc.nodes[id].ns_decls = decls;
        for (aname, value, aline) in attrs {
            let (ap, al) = split_qname(&aname);
            let (ap, al) = (ap.map(<[u8]>::to_vec), al.to_vec());
            let ans = match &ap {
                Some(p) => {
                    let uri = self.resolve_ns(Some(p));
                    if uri.is_none() {
                        self.error_at(
                            2,
                            201,
                            tag_end_col,
                            format!(
                                "Namespace prefix {} for {} on {} is not defined",
                                String::from_utf8_lossy(p),
                                String::from_utf8_lossy(&al),
                                String::from_utf8_lossy(&local)
                            ),
                        );
                    }
                    uri
                }
                None => None,
            };
            let a = self
                .doc
                .create_attr(&al, ap.as_deref(), ans.as_deref(), b"");
            self.doc.nodes[a].line = aline;
            self.set_attr_children(a, &value);
            self.doc.nodes[a].parent = Some(id);
            self.doc.nodes[id].attrs.push(a);
        }
        self.doc.link_last(parent, id);
        if self_closing {
            self.doc.nodes[id].self_closing = true;
            self.ns_stack.pop();
            return;
        }
        self.content(id, &qname, line);
        self.ns_stack.pop();
    }

    fn resolve_ns(&self, prefix: Option<&[u8]>) -> Option<Vec<u8>> {
        for scope in self.ns_stack.iter().rev() {
            for d in scope {
                if d.prefix.as_deref() == prefix {
                    return Some(d.uri.clone());
                }
            }
        }
        if prefix == Some(b"xml") {
            return Some(b"http://www.w3.org/XML/1998/namespace".to_vec());
        }
        None
    }

    /// An attribute value with references resolved: text, entity
    /// references kept as nodes (unless substituted).
    fn attr_value(&mut self) -> Option<Vec<u8>> {
        let q = match self.peek() {
            Some(q @ (b'"' | b'\'')) => q,
            _ => {
                self.fatal(39, "AttValue: \" or ' expected".to_string());
                return None;
            }
        };
        self.bump();
        let start = self.pos;
        loop {
            match self.peek() {
                None => {
                    self.fatal(46, format!("AttValue: {} expected", q as char));
                    return None;
                }
                Some(b) if b == q => {
                    let raw = self.src[start..self.pos].to_vec();
                    self.bump();
                    return Some(raw);
                }
                Some(b'<') => {
                    self.fatal(
                        38,
                        "Unescaped '<' not allowed in attributes values".to_string(),
                    );
                    return None;
                }
                Some(_) => {
                    self.bump();
                }
            }
        }
    }

    /// Build an attribute's children from its raw value: text with
    /// character references decoded and whitespace normalized, entity
    /// references as nodes or their text.
    fn set_attr_children(&mut self, attr: NodeId, raw: &[u8]) {
        let mut text: Vec<u8> = Vec::new();
        let mut i = 0;
        while i < raw.len() {
            let b = raw[i];
            if b == b'&' {
                let Some(end) = raw[i..].iter().position(|&c| c == b';') else {
                    self.fatal(23, "EntityRef: expecting ';'".to_string());
                    return;
                };
                let body = &raw[i + 1..i + end];
                i += end + 1;
                if let Some(cr) = body.strip_prefix(b"#") {
                    match char_ref(cr) {
                        Some(c) => {
                            let mut buf = [0u8; 4];
                            text.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                        }
                        None => {
                            self.fatal(9, "xmlParseCharRef: invalid xmlChar value 0".to_string());
                            return;
                        }
                    }
                    continue;
                }
                match self.resolve_entity(body) {
                    EntityKind::Predefined(v) => text.extend_from_slice(v),
                    EntityKind::Internal(v) => {
                        if self.opts.substitute_entities {
                            text.extend_from_slice(&v);
                        } else {
                            if !text.is_empty() {
                                let t = self.doc.create_text(NodeKind::Text, &text);
                                self.doc.link_last(attr, t);
                                text.clear();
                            }
                            let r = self.doc.create_entity_ref(body);
                            self.doc.link_last(attr, r);
                        }
                    }
                    EntityKind::Undeclared => {
                        self.fatal(
                            26,
                            format!("Entity '{}' not defined", String::from_utf8_lossy(body)),
                        );
                        return;
                    }
                }
                continue;
            }
            // Attribute-value normalization: each whitespace char → space.
            text.push(if matches!(b, b'\t' | b'\n' | b'\r') {
                b' '
            } else {
                b
            });
            i += 1;
        }
        if !text.is_empty() {
            let t = self.doc.create_text(NodeKind::Text, &text);
            self.doc.link_last(attr, t);
        }
    }

    fn resolve_entity(&self, name: &[u8]) -> EntityKind {
        match name {
            b"lt" => EntityKind::Predefined(b"<"),
            b"gt" => EntityKind::Predefined(b">"),
            b"amp" => EntityKind::Predefined(b"&"),
            b"quot" => EntityKind::Predefined(b"\""),
            b"apos" => EntityKind::Predefined(b"'"),
            _ => match self.entities.iter().find(|(n, _)| n == name) {
                Some((_, v)) => EntityKind::Internal(v.clone()),
                None => EntityKind::Undeclared,
            },
        }
    }

    /// Children of `element` up to and including its end tag.
    fn content(&mut self, element: NodeId, qname: &[u8], open_line: i64) {
        let mut text: Vec<u8> = Vec::new();
        let mut text_line = self.line;
        loop {
            if self.at_end() {
                self.flush_text(element, &mut text, text_line);
                if !self.eof_reported {
                    self.eof_reported = true;
                    self.fatal(
                        77,
                        format!(
                            "Premature end of data in tag {} line {open_line}",
                            String::from_utf8_lossy(qname)
                        ),
                    );
                }
                return;
            }
            let b = self.peek().unwrap_or(0);
            if b == b'<' {
                self.flush_text(element, &mut text, text_line);
                if self.starts_with(b"</") {
                    self.advance(2);
                    let end_name = self.name().unwrap_or_default();
                    self.skip_ws();
                    if self.peek() == Some(b'>') {
                        self.bump();
                    }
                    if end_name != qname {
                        // libxml reports the mismatch and lets the end tag
                        // close the open element anyway.
                        self.fatal(
                            76,
                            format!(
                                "Opening and ending tag mismatch: {} line {open_line} and {}",
                                String::from_utf8_lossy(qname),
                                String::from_utf8_lossy(&end_name)
                            ),
                        );
                    }
                    return;
                } else if self.starts_with(b"<!--") {
                    if let Some(id) = self.comment() {
                        self.doc.link_last(element, id);
                    }
                } else if self.starts_with(b"<![CDATA[") {
                    if let Some(id) = self.cdata() {
                        self.doc.link_last(element, id);
                    }
                } else if self.starts_with(b"<?") {
                    if let Some(id) = self.pi() {
                        self.doc.link_last(element, id);
                    }
                } else {
                    self.element(element);
                }
                text_line = self.line;
                continue;
            }
            if b == b'&' {
                let Some(end) = self.src[self.pos..].iter().position(|&c| c == b';') else {
                    self.flush_text(element, &mut text, text_line);
                    self.fatal(23, "EntityRef: expecting ';'".to_string());
                    self.bump();
                    continue;
                };
                let body = self.src[self.pos + 1..self.pos + end].to_vec();
                if let Some(cr) = body.strip_prefix(b"#") {
                    match char_ref(cr) {
                        Some(c) => {
                            let mut buf = [0u8; 4];
                            text.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                        }
                        None => {
                            self.flush_text(element, &mut text, text_line);
                            let shown = String::from_utf8_lossy(cr).into_owned();
                            // libxml has consumed the reference by then.
                            self.advance(end + 1);
                            self.fatal(
                                9,
                                format!(
                                    "xmlParseCharRef: invalid xmlChar value {}",
                                    shown.trim_start_matches('x')
                                ),
                            );
                            continue;
                        }
                    }
                    self.advance(end + 1);
                    continue;
                }
                match self.resolve_entity(&body) {
                    EntityKind::Predefined(v) => {
                        text.extend_from_slice(v);
                        self.advance(end + 1);
                    }
                    EntityKind::Internal(v) => {
                        if self.opts.substitute_entities {
                            text.extend_from_slice(&v);
                        } else {
                            self.flush_text(element, &mut text, text_line);
                            let r = self.doc.create_entity_ref(&body);
                            self.doc.nodes[r].line = self.line as u32;
                            self.doc.link_last(element, r);
                        }
                        self.advance(end + 1);
                        text_line = self.line;
                    }
                    EntityKind::Undeclared => {
                        self.flush_text(element, &mut text, text_line);
                        self.advance(end + 1);
                        self.fatal(
                            26,
                            format!("Entity '{}' not defined", String::from_utf8_lossy(&body)),
                        );
                    }
                }
                continue;
            }
            if b == b']' && self.starts_with(b"]]>") {
                self.flush_text(element, &mut text, text_line);
                self.fatal(62, "Sequence ']]>' not allowed in content".to_string());
                self.advance(3);
                continue;
            }
            if b == b'\r' {
                // End-of-line normalization.
                self.bump();
                if self.peek() != Some(b'\n') {
                    text.push(b'\n');
                }
                continue;
            }
            text.push(b);
            self.bump();
        }
    }

    fn flush_text(&mut self, element: NodeId, text: &mut Vec<u8>, line: i64) {
        if text.is_empty() {
            return;
        }
        let blank = text
            .iter()
            .all(|b| matches!(b, b' ' | b'\t' | b'\r' | b'\n'));
        if blank
            && (!self.opts.preserve_white_space || self.opts.no_blanks)
            && self.are_blanks(element)
        {
            text.clear();
            return;
        }
        let id = self.doc.create_text(NodeKind::Text, text);
        self.doc.nodes[id].line = line as u32;
        self.doc.link_last(element, id);
        text.clear();
    }
}

impl<'a> Parser<'a> {
    /// libxml's `areBlanks()` heuristic (no DTD): a blank run before a tag
    /// is ignorable unless it is a leaf's whole content or sits next to
    /// text the element already has.
    fn are_blanks(&self, element: NodeId) -> bool {
        if self.peek() != Some(b'<') {
            return false;
        }
        let children = self.doc.children(element);
        if children.is_empty() && self.starts_with(b"</") {
            return false;
        }
        let is_text =
            |id: NodeId| matches!(self.doc.node(id).kind, NodeKind::Text | NodeKind::CData);
        match (children.first(), children.last()) {
            (Some(&f), Some(&l)) if is_text(l) || is_text(f) => false,
            _ => true,
        }
    }
}

enum EntityKind {
    Predefined(&'static [u8]),
    Internal(Vec<u8>),
    Undeclared,
}

/// `#65` / `#x41` (after the `#`) → the character; `None` for an invalid one.
fn char_ref(body: &[u8]) -> Option<char> {
    let n = if let Some(h) = body.strip_prefix(b"x") {
        u32::from_str_radix(std::str::from_utf8(h).ok()?, 16).ok()?
    } else {
        std::str::from_utf8(body).ok()?.parse::<u32>().ok()?
    };
    if n == 0 {
        return None;
    }
    char::from_u32(n)
}

/// libxml's spelling of an element content model: `(a,b)*` → `(a , b)*`,
/// `(#PCDATA|a)*` → `(#PCDATA | a)*`, `EMPTY`/`ANY` as they are.
fn normalize_content_model(body: &[u8]) -> Vec<u8> {
    let text: Vec<u8> = body
        .iter()
        .copied()
        .filter(|b| !b.is_ascii_whitespace())
        .collect();
    let mut out = Vec::new();
    for &b in &text {
        match b {
            b',' => out.extend_from_slice(b" , "),
            b'|' => out.extend_from_slice(b" | "),
            _ => out.push(b),
        }
    }
    out
}

/// The attributes of an `<!ATTLIST>` body: (name, type, default) per
/// attribute, spelled as libxml prints them.
fn parse_attlist(body: &[u8]) -> Vec<(Vec<u8>, Vec<u8>, Vec<u8>)> {
    let mut out = Vec::new();
    let mut i = 0;
    let ws = |b: u8| b.is_ascii_whitespace();
    let skip = |i: &mut usize| {
        while *i < body.len() && ws(body[*i]) {
            *i += 1;
        }
    };
    let token = |i: &mut usize| -> Vec<u8> {
        let start = *i;
        while *i < body.len()
            && !ws(body[*i])
            && body[*i] != b'('
            && body[*i] != b'"'
            && body[*i] != b'\''
        {
            *i += 1;
        }
        body[start..*i].to_vec()
    };
    loop {
        skip(&mut i);
        if i >= body.len() {
            break;
        }
        let name = token(&mut i);
        if name.is_empty() {
            break;
        }
        skip(&mut i);
        // The type: a keyword, `NOTATION (…)` or an enumeration `(…)`.
        let mut ty;
        if body.get(i) == Some(&b'(') {
            let end = body[i..]
                .iter()
                .position(|&b| b == b')')
                .map_or(body.len(), |e| i + e + 1);
            ty = normalize_content_model(&body[i..end]);
            i = end;
        } else {
            ty = token(&mut i);
            if ty == b"NOTATION" {
                skip(&mut i);
                if body.get(i) == Some(&b'(') {
                    let end = body[i..]
                        .iter()
                        .position(|&b| b == b')')
                        .map_or(body.len(), |e| i + e + 1);
                    ty.push(b' ');
                    ty.extend_from_slice(&normalize_content_model(&body[i..end]));
                    i = end;
                }
            }
        }
        skip(&mut i);
        // The default: #REQUIRED / #IMPLIED / [#FIXED] "value".
        let mut default = Vec::new();
        if body.get(i) == Some(&b'#') {
            let kw = token(&mut i);
            default.extend_from_slice(&kw);
            if kw == b"#FIXED" {
                skip(&mut i);
                default.push(b' ');
            }
        }
        if let Some(&q) = body.get(i).filter(|&&b| b == b'"' || b == b'\'') {
            let end = body[i + 1..]
                .iter()
                .position(|&b| b == q)
                .map_or(body.len(), |e| i + 1 + e);
            default.push(b'"');
            default.extend_from_slice(&body[i + 1..end]);
            default.push(b'"');
            i = (end + 1).min(body.len());
        }
        out.push((name, ty, default));
    }
    out
}
