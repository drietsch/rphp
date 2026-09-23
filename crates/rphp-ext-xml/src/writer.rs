//! `ext/xmlwriter`: `XMLWriter` and the `xmlwriter_*` functions, over a
//! replica of libxml2's `xmlTextWriter` — its node stack and states, when
//! a start tag is closed, the indentation rules (`indent` writes the
//! nesting depth before a start tag or comment, a newline after an end
//! tag; text switches the next end tag's indent off), the namespace
//! declarations queued until a start tag closes (written last-in first),
//! the escaping of text (`xmlEncodeSpecialChars`) and attribute values
//! (`xmlBufAttrSerializeTxtContent`, non-ASCII as hex references), and the
//! 4000-byte output buffer in front of a URI.

use std::io::Write as _;

use rphp_runtime::{Ctx, NativeFn, NativeMethod, NativeResult, Registry, Unwind};
use rphp_value::{Object, Payload, Value};

use crate::sax::utf8_at;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum S {
    Name,
    Attribute,
    Text,
    Pi,
    PiText,
    CData,
    Dtd,
    DtdText,
    DtdElem,
    DtdElemText,
    DtdAttl,
    DtdAttlText,
    DtdEnty,
    DtdPent,
    DtdEntyText,
    Comment,
}

struct Node {
    name: Vec<u8>,
    state: S,
    id: u64,
}

struct NsEnt {
    /// `xmlns` or `xmlns:p`.
    prefix: Vec<u8>,
    uri: Vec<u8>,
    elem: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Enc {
    Utf8,
    Latin1,
    Ascii,
}

enum Target {
    Memory(Vec<u8>),
    File(std::fs::File),
    Stream(Value),
}

/// A libxml error: `Some(message)` warns, `None` fails silently.
type R = Result<(), Option<&'static str>>;

const MINLEN: usize = 4000;

pub struct Writer {
    nodes: Vec<Node>,
    ns: Vec<NsEnt>,
    indent: bool,
    doindent: bool,
    ichar: Vec<u8>,
    qchar: u8,
    enc: Option<Enc>,
    /// Output not yet handed to a URI target.
    pending: Vec<u8>,
    target: Target,
    next_id: u64,
}

impl Drop for Writer {
    /// libxml flushes what is buffered when the writer is freed; a php
    /// stream cannot be reached from here, a file can.
    fn drop(&mut self) {
        if let Target::File(f) = &mut self.target {
            let _ = f.write_all(&self.pending);
            self.pending.clear();
        }
    }
}

fn is_name(name: &[u8]) -> bool {
    let Ok(s) = std::str::from_utf8(name) else { return false };
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if rphp_ext_dom::is_name_start(c) => chars.all(rphp_ext_dom::is_name_char),
        _ => false,
    }
}

/// `xmlEncodeSpecialChars`.
fn escape_text(s: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    for &b in s {
        match b {
            b'<' => out.extend_from_slice(b"&lt;"),
            b'>' => out.extend_from_slice(b"&gt;"),
            b'&' => out.extend_from_slice(b"&amp;"),
            b'"' => out.extend_from_slice(b"&quot;"),
            b'\r' => out.extend_from_slice(b"&#13;"),
            _ => out.push(b),
        }
    }
    out
}

/// `xmlBufAttrSerializeTxtContent` without a document: non-ASCII as hex
/// character references.
fn escape_attr(s: &[u8], hex: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        let b = s[i];
        match b {
            b'<' => out.extend_from_slice(b"&lt;"),
            b'>' => out.extend_from_slice(b"&gt;"),
            b'&' => out.extend_from_slice(b"&amp;"),
            b'"' => out.extend_from_slice(b"&quot;"),
            b'\n' => out.extend_from_slice(b"&#10;"),
            b'\r' => out.extend_from_slice(b"&#13;"),
            b'\t' => out.extend_from_slice(b"&#9;"),
            0x80.. if hex => {
                if let Some((c, l)) = utf8_at(&s[i..]) {
                    out.extend_from_slice(format!("&#x{c:X};").as_bytes());
                    i += l;
                    continue;
                }
                out.push(b);
            }
            _ => out.push(b),
        }
        i += 1;
    }
    out
}

impl Writer {
    fn new(target: Target) -> Writer {
        Writer {
            nodes: Vec::new(),
            ns: Vec::new(),
            indent: false,
            doindent: true,
            ichar: b" ".to_vec(),
            qchar: b'"',
            enc: None,
            pending: Vec::new(),
            target,
            next_id: 0,
        }
    }

    // ---- output ----------------------------------------------------------

    fn out(&mut self, s: &[u8]) {
        let bytes = match self.enc {
            None | Some(Enc::Utf8) => s.to_vec(),
            Some(e) => {
                let max = if e == Enc::Latin1 { 0xFF } else { 0x7F };
                let mut o = Vec::with_capacity(s.len());
                let mut i = 0;
                while i < s.len() {
                    match utf8_at(&s[i..]) {
                        Some((c, l)) => {
                            if c <= max {
                                o.push(c as u8);
                            } else {
                                o.extend_from_slice(format!("&#{c};").as_bytes());
                            }
                            i += l;
                        }
                        None => {
                            o.push(s[i]);
                            i += 1;
                        }
                    }
                }
                o
            }
        };
        match &mut self.target {
            Target::Memory(m) => m.extend_from_slice(&bytes),
            _ => self.pending.extend_from_slice(&bytes),
        }
    }

    fn indent_now(&mut self) {
        let n = self.nodes.len().saturating_sub(1);
        let ichar = self.ichar.clone();
        for _ in 0..n {
            self.out(&ichar);
        }
    }

    fn front(&self) -> Option<S> {
        self.nodes.last().map(|n| n.state)
    }

    fn set_front(&mut self, s: S) {
        if let Some(n) = self.nodes.last_mut() {
            n.state = s;
        }
    }

    fn push(&mut self, name: &[u8], state: S) {
        self.next_id += 1;
        self.nodes.push(Node {
            name: name.to_vec(),
            state,
            id: self.next_id,
        });
    }

    fn quote(&mut self) {
        let q = [self.qchar];
        self.out(&q);
    }

    /// `xmlTextWriterOutputNSDecl`.
    fn output_ns_decl(&mut self) {
        while let Some(np) = self.ns.pop() {
            self.out(b" ");
            self.out(&np.prefix);
            self.out(b"=");
            self.quote();
            let uri = escape_attr(&np.uri, self.enc.is_none());
            self.out(&uri);
            self.quote();
        }
    }

    /// Close a start tag that is still open (`NAME` or `ATTRIBUTE`):
    /// namespaces, `>`, the newline under indentation.
    fn close_start_tag(&mut self, newline: bool) {
        if self.front() == Some(S::Attribute) {
            let _ = self.end_attribute();
        }
        self.output_ns_decl();
        self.out(b">");
        if newline && self.indent {
            self.out(b"\n");
        }
        self.set_front(S::Text);
    }

    /// `xmlTextWriterHandleStateDependencies`.
    fn state_deps(&mut self) {
        let Some(s) = self.front() else { return };
        match s {
            S::Name => {
                self.output_ns_decl();
                self.out(b">");
                self.set_front(S::Text);
            }
            S::Pi => {
                self.out(b" ");
                self.set_front(S::PiText);
            }
            S::Dtd => {
                self.out(b" [");
                self.set_front(S::DtdText);
            }
            S::DtdElem => {
                self.out(b" ");
                self.set_front(S::DtdElemText);
            }
            S::DtdAttl => {
                self.out(b" ");
                self.set_front(S::DtdAttlText);
            }
            S::DtdEnty | S::DtdPent => {
                self.out(b" ");
                self.quote();
                self.set_front(S::DtdEntyText);
            }
            _ => {}
        }
    }

    // ---- the xmlTextWriter* operations -----------------------------------

    fn write_raw(&mut self, content: &[u8]) -> R {
        self.state_deps();
        if self.indent {
            self.doindent = false;
        }
        self.out(content);
        Ok(())
    }

    fn write_string(&mut self, content: &[u8]) -> R {
        match self.front() {
            Some(S::Name) | Some(S::Text) => {
                let e = escape_text(content);
                self.write_raw(&e)
            }
            Some(S::Attribute) => {
                // Without an output encoding non-ASCII becomes a hex
                // reference; with one the encoder handles it.
                let e = escape_attr(content, self.enc.is_none());
                self.out(&e);
                Ok(())
            }
            _ => self.write_raw(content),
        }
    }

    fn start_document(&mut self, version: Option<&[u8]>, encoding: Option<&[u8]>, standalone: Option<&[u8]>) -> R {
        if !self.nodes.is_empty() {
            return Err(Some("xmlTextWriterStartDocument : not allowed in this context!"));
        }
        let mut enc_name: Option<&'static str> = None;
        if let Some(e) = encoding {
            let (enc, name) = match e.to_ascii_uppercase().as_slice() {
                b"UTF-8" => (Enc::Utf8, "UTF-8"),
                b"ISO-8859-1" => (Enc::Latin1, "ISO-8859-1"),
                b"ASCII" => (Enc::Ascii, "ASCII"),
                b"US-ASCII" => (Enc::Ascii, "US-ASCII"),
                _ => return Err(Some("xmlTextWriterStartDocument : unsupported encoding")),
            };
            self.enc = Some(enc);
            enc_name = Some(name);
        } else {
            self.enc = None;
        }
        self.out(b"<?xml version=");
        self.quote();
        self.out(version.unwrap_or(b"1.0"));
        self.quote();
        if let Some(n) = enc_name {
            self.out(b" encoding=");
            self.quote();
            self.out(n.as_bytes());
            self.quote();
        }
        if let Some(s) = standalone {
            self.out(b" standalone=");
            self.quote();
            self.out(s);
            self.quote();
        }
        self.out(b"?>\n");
        Ok(())
    }

    fn end_document(&mut self) -> R {
        while let Some(s) = self.front() {
            let r = match s {
                S::Name | S::Attribute | S::Text => self.end_element(),
                S::Pi | S::PiText => self.end_pi(),
                S::CData => self.end_cdata(),
                S::Comment => self.end_comment(),
                _ => self.end_dtd(),
            };
            if r.is_err() {
                break;
            }
        }
        if !self.indent {
            self.out(b"\n");
        }
        Ok(())
    }

    fn start_element(&mut self, name: &[u8]) -> R {
        match self.front() {
            Some(S::Pi) | Some(S::PiText) => return Err(None),
            Some(S::Attribute) | Some(S::Name) => self.close_start_tag(true),
            _ => {}
        }
        self.push(name, S::Name);
        if self.indent {
            self.indent_now();
        }
        self.out(b"<");
        self.out(name);
        Ok(())
    }

    fn start_element_ns(&mut self, prefix: Option<&[u8]>, name: &[u8], uri: Option<&[u8]>) -> R {
        let q = match prefix {
            Some(p) => [p, b":", name].concat(),
            None => name.to_vec(),
        };
        self.start_element(&q)?;
        if let Some(u) = uri {
            let mut decl = b"xmlns".to_vec();
            if let Some(p) = prefix {
                decl.push(b':');
                decl.extend_from_slice(p);
            }
            let elem = self.nodes.last().map_or(0, |n| n.id);
            self.ns.push(NsEnt {
                prefix: decl,
                uri: u.to_vec(),
                elem,
            });
        }
        Ok(())
    }

    fn end_element(&mut self) -> R {
        let Some(s) = self.front() else {
            self.ns.clear();
            return Err(None);
        };
        match s {
            S::Attribute | S::Name => {
                if s == S::Attribute {
                    self.end_attribute()?;
                }
                self.output_ns_decl();
                if self.indent {
                    self.doindent = true;
                }
                self.out(b"/>");
            }
            S::Text => {
                if self.indent && self.doindent {
                    self.indent_now();
                }
                self.doindent = true;
                let name = self.nodes.last().map(|n| n.name.clone()).unwrap_or_default();
                self.out(b"</");
                self.out(&name);
                self.out(b">");
            }
            _ => return Err(None),
        }
        if self.indent {
            self.out(b"\n");
        }
        self.nodes.pop();
        Ok(())
    }

    fn full_end_element(&mut self) -> R {
        let Some(s) = self.front() else { return Err(None) };
        match s {
            S::Attribute | S::Name => {
                if s == S::Attribute {
                    self.end_attribute()?;
                }
                self.output_ns_decl();
                self.out(b">");
                if self.indent {
                    self.doindent = false;
                }
            }
            S::Text => {}
            _ => return Err(None),
        }
        if self.indent && self.doindent {
            self.indent_now();
        }
        self.doindent = true;
        let name = self.nodes.last().map(|n| n.name.clone()).unwrap_or_default();
        self.out(b"</");
        self.out(&name);
        self.out(b">");
        if self.indent {
            self.out(b"\n");
        }
        self.nodes.pop();
        Ok(())
    }

    fn start_attribute(&mut self, name: &[u8]) -> R {
        match self.front() {
            Some(S::Attribute) | Some(S::Name) => {
                if self.front() == Some(S::Attribute) {
                    self.end_attribute()?;
                }
                self.out(b" ");
                self.out(name);
                self.out(b"=");
                self.quote();
                self.set_front(S::Attribute);
                Ok(())
            }
            _ => Err(None),
        }
    }

    fn start_attribute_ns(&mut self, prefix: Option<&[u8]>, name: &[u8], uri: Option<&[u8]>) -> R {
        if let Some(u) = uri {
            let mut decl = b"xmlns".to_vec();
            if let Some(p) = prefix {
                decl.push(b':');
                decl.extend_from_slice(p);
            }
            let elem = self.nodes.last().map_or(0, |n| n.id);
            match self.ns.iter().find(|e| e.prefix == decl && e.elem == elem) {
                Some(e) if e.uri.as_slice() == u => {}
                Some(_) => return Err(None),
                None => self.ns.push(NsEnt {
                    prefix: decl,
                    uri: u.to_vec(),
                    elem,
                }),
            }
        }
        let q = match prefix {
            Some(p) => [p, b":", name].concat(),
            None => name.to_vec(),
        };
        self.start_attribute(&q)
    }

    fn end_attribute(&mut self) -> R {
        match self.front() {
            Some(S::Attribute) => {
                self.set_front(S::Name);
                self.quote();
                Ok(())
            }
            _ => Err(None),
        }
    }

    fn write_attribute(&mut self, name: &[u8], value: &[u8]) -> R {
        self.start_attribute(name)?;
        self.write_string(value)?;
        self.end_attribute()
    }

    fn write_attribute_ns(&mut self, prefix: Option<&[u8]>, name: &[u8], uri: Option<&[u8]>, value: &[u8]) -> R {
        self.start_attribute_ns(prefix, name, uri)?;
        self.write_string(value)?;
        self.end_attribute()
    }

    fn start_comment(&mut self) -> R {
        match self.front() {
            None | Some(S::Text) => {}
            Some(S::Name) => self.close_start_tag(true),
            _ => return Err(None),
        }
        self.push(b"", S::Comment);
        if self.indent {
            self.indent_now();
        }
        self.out(b"<!--");
        Ok(())
    }

    fn end_comment(&mut self) -> R {
        match self.front() {
            Some(S::Comment) => {
                self.out(b"-->");
            }
            _ => return Err(None),
        }
        if self.indent {
            self.out(b"\n");
        }
        self.nodes.pop();
        Ok(())
    }

    fn start_pi(&mut self, target: &[u8]) -> R {
        if target.eq_ignore_ascii_case(b"xml") {
            return Err(Some(
                "xmlTextWriterStartPI : target name [Xx][Mm][Ll] is reserved for xml standardization!",
            ));
        }
        match self.front() {
            Some(S::Attribute) | Some(S::Name) => self.close_start_tag(false),
            None | Some(S::Text) | Some(S::Dtd) => {}
            Some(S::Pi) | Some(S::PiText) => return Err(Some("xmlTextWriterStartPI : nested PI!")),
            _ => return Err(None),
        }
        self.push(target, S::Pi);
        self.out(b"<?");
        self.out(target);
        Ok(())
    }

    fn end_pi(&mut self) -> R {
        match self.front() {
            Some(S::Pi) | Some(S::PiText) => self.out(b"?>"),
            _ => return Err(None),
        }
        if self.indent {
            self.out(b"\n");
        }
        self.nodes.pop();
        Ok(())
    }

    fn start_cdata(&mut self) -> R {
        match self.front() {
            None | Some(S::Text) | Some(S::Pi) | Some(S::PiText) => {}
            Some(S::Attribute) | Some(S::Name) => self.close_start_tag(false),
            Some(S::CData) => {
                return Err(Some("xmlTextWriterStartCDATA : CDATA not allowed in this context!"))
            }
            _ => return Err(None),
        }
        self.push(b"", S::CData);
        self.out(b"<![CDATA[");
        Ok(())
    }

    fn end_cdata(&mut self) -> R {
        match self.front() {
            Some(S::CData) => self.out(b"]]>"),
            _ => return Err(None),
        }
        self.nodes.pop();
        Ok(())
    }

    fn start_dtd(&mut self, name: &[u8], pubid: Option<&[u8]>, sysid: Option<&[u8]>) -> R {
        if name.is_empty() {
            return Err(None);
        }
        if !self.nodes.is_empty() {
            return Err(Some("xmlTextWriterStartDTD : DTD allowed only in prolog!"));
        }
        self.push(name, S::Dtd);
        self.out(b"<!DOCTYPE ");
        self.out(name);
        if let Some(p) = pubid {
            if sysid.is_none() {
                return Err(Some("xmlTextWriterStartDTD : system identifier needed!"));
            }
            self.out(if self.indent { b"\n" } else { b" " });
            self.out(b"PUBLIC ");
            self.quote();
            self.out(p);
            self.quote();
        }
        if let Some(s) = sysid {
            if pubid.is_none() {
                self.out(if self.indent { b"\n" } else { b" " });
                self.out(b"SYSTEM ");
            } else if self.indent {
                self.out(b"\n       ");
            } else {
                self.out(b" ");
            }
            self.quote();
            self.out(s);
            self.quote();
        }
        Ok(())
    }

    fn end_dtd(&mut self) -> R {
        while let Some(s) = self.front() {
            let r = match s {
                S::DtdText | S::Dtd => {
                    if s == S::DtdText {
                        self.out(b"]");
                    }
                    self.out(b">");
                    if self.indent {
                        self.out(b"\n");
                    }
                    self.nodes.pop();
                    Ok(())
                }
                S::DtdElem | S::DtdElemText => self.end_dtd_element(),
                S::DtdAttl | S::DtdAttlText => self.end_dtd_attlist(),
                S::DtdEnty | S::DtdPent | S::DtdEntyText => self.end_dtd_entity(),
                S::Comment => self.end_comment(),
                _ => break,
            };
            r?;
        }
        Ok(())
    }

    fn write_dtd(&mut self, name: &[u8], pubid: Option<&[u8]>, sysid: Option<&[u8]>, subset: Option<&[u8]>) -> R {
        self.start_dtd(name, pubid, sysid)?;
        if let Some(s) = subset {
            self.write_string(s)?;
        }
        self.end_dtd()
    }

    /// Open the internal subset (` [`) before a declaration; outside any
    /// node the declaration is written on its own.
    fn dtd_decl_start(&mut self) -> R {
        match self.front() {
            Some(S::Dtd) => {
                self.out(b" [");
                if self.indent {
                    self.out(b"\n");
                }
                self.set_front(S::DtdText);
            }
            Some(S::DtdText) | None => {}
            _ => return Err(None),
        }
        Ok(())
    }

    fn start_dtd_element(&mut self, name: &[u8]) -> R {
        self.dtd_decl_start()?;
        self.push(name, S::DtdElem);
        if self.indent {
            self.indent_now();
        }
        self.out(b"<!ELEMENT ");
        self.out(name);
        Ok(())
    }

    fn end_dtd_element(&mut self) -> R {
        match self.front() {
            Some(S::DtdElem) | Some(S::DtdElemText) => self.out(b">"),
            _ => return Err(None),
        }
        if self.indent {
            self.out(b"\n");
        }
        self.nodes.pop();
        Ok(())
    }

    fn write_dtd_element(&mut self, name: &[u8], content: &[u8]) -> R {
        self.start_dtd_element(name)?;
        self.write_string(content)?;
        self.end_dtd_element()
    }

    fn start_dtd_attlist(&mut self, name: &[u8]) -> R {
        self.dtd_decl_start()?;
        self.push(name, S::DtdAttl);
        if self.indent {
            self.indent_now();
        }
        self.out(b"<!ATTLIST ");
        self.out(name);
        Ok(())
    }

    fn end_dtd_attlist(&mut self) -> R {
        match self.front() {
            Some(S::DtdAttl) | Some(S::DtdAttlText) => self.out(b">"),
            _ => return Err(None),
        }
        if self.indent {
            self.out(b"\n");
        }
        self.nodes.pop();
        Ok(())
    }

    fn write_dtd_attlist(&mut self, name: &[u8], content: &[u8]) -> R {
        self.start_dtd_attlist(name)?;
        self.write_string(content)?;
        self.end_dtd_attlist()
    }

    fn start_dtd_entity(&mut self, pe: bool, name: &[u8]) -> R {
        self.dtd_decl_start()?;
        self.push(name, if pe { S::DtdPent } else { S::DtdEnty });
        if self.indent {
            self.indent_now();
        }
        self.out(b"<!ENTITY ");
        if pe {
            self.out(b"% ");
        }
        self.out(name);
        Ok(())
    }

    fn end_dtd_entity(&mut self) -> R {
        match self.front() {
            Some(S::DtdEntyText) => {
                self.quote();
                self.out(b">");
            }
            Some(S::DtdEnty) | Some(S::DtdPent) => self.out(b">"),
            _ => return Err(None),
        }
        if self.indent {
            self.out(b"\n");
        }
        self.nodes.pop();
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn write_dtd_entity(
        &mut self,
        pe: bool,
        name: &[u8],
        pubid: Option<&[u8]>,
        sysid: Option<&[u8]>,
        ndata: Option<&[u8]>,
        content: Option<&[u8]>,
    ) -> R {
        if content.is_none() && pubid.is_none() && sysid.is_none() {
            return Err(None);
        }
        if pe && ndata.is_some() {
            return Err(None);
        }
        if pubid.is_none() && sysid.is_none() {
            self.start_dtd_entity(pe, name)?;
            self.write_string(content.unwrap_or_default())?;
            return self.end_dtd_entity();
        }
        self.start_dtd_entity(pe, name)?;
        if let Some(p) = pubid {
            if sysid.is_none() {
                return Err(Some("xmlTextWriterWriteDTDExternalEntityContents: system identifier needed!"));
            }
            self.out(b" PUBLIC ");
            self.quote();
            self.out(p);
            self.quote();
        }
        if let Some(s) = sysid {
            if pubid.is_none() {
                self.out(b" SYSTEM");
            }
            self.out(b" ");
            self.quote();
            self.out(s);
            self.quote();
        }
        if let Some(n) = ndata {
            self.out(b" NDATA ");
            self.out(n);
        }
        self.end_dtd_entity()
    }

    /// `xmlTextWriterFlush` for a URI target: the bytes handed over.
    fn flush_to(&mut self, ctx: &mut Ctx) -> Result<i64, Unwind> {
        let data = std::mem::take(&mut self.pending);
        match &mut self.target {
            Target::Memory(_) => Ok(0),
            Target::File(f) => {
                let _ = f.write_all(&data);
                Ok(data.len() as i64)
            }
            Target::Stream(s) => {
                if !data.is_empty() {
                    let s = s.clone();
                    ctx.call_function(b"fwrite", &[s, Value::string(&data)])?;
                }
                Ok(data.len() as i64)
            }
        }
    }
}

// ---- the php surface -------------------------------------------------------

type Shared = std::rc::Rc<std::cell::RefCell<Option<Writer>>>;

fn shared_of(o: &Object) -> Shared {
    if let Some(s) = o.with_payload::<Shared, _>(|s| s.clone()) {
        return s;
    }
    let s: Shared = Default::default();
    o.set_payload(Payload::Native(Box::new(s.clone())));
    s
}

fn writer_value_arg(ctx: &mut Ctx, v: Option<&Value>) -> Result<Object, Unwind> {
    let v = v.map(|v| v.deref().into_owned()).unwrap_or(Value::Null);
    if let Value::Object(o) = &v {
        if let Some(cid) = ctx.class_by_name(b"XMLWriter") {
            if ctx.object_instanceof(o, cid) {
                return Ok(o.clone());
            }
        }
    }
    Err(Unwind::type_error(format!(
        "{}(): Argument #1 ($writer) must be of type XMLWriter, {} given",
        ctx.active_function_name(),
        rphp_runtime::value_name(&v)
    )))
}

/// The operations, each with its method arguments (the procedural form
/// passes the writer first).
#[derive(Clone, Copy)]
enum Op {
    SetIndent,
    SetIndentString,
    StartComment,
    EndComment,
    StartAttribute,
    EndAttribute,
    WriteAttribute,
    StartAttributeNs,
    WriteAttributeNs,
    StartElement,
    EndElement,
    FullEndElement,
    StartElementNs,
    WriteElement,
    WriteElementNs,
    StartPi,
    EndPi,
    WritePi,
    StartCdata,
    EndCdata,
    WriteCdata,
    Text,
    WriteRaw,
    StartDocument,
    EndDocument,
    WriteComment,
    StartDtd,
    EndDtd,
    WriteDtd,
    StartDtdElement,
    EndDtdElement,
    WriteDtdElement,
    StartDtdAttlist,
    EndDtdAttlist,
    WriteDtdAttlist,
    StartDtdEntity,
    EndDtdEntity,
    WriteDtdEntity,
    OutputMemory,
    Flush,
}

/// method name, procedural name, op, min, max, method parameter names.
type Row = (&'static str, &'static str, Op, u8, u8, &'static [&'static str]);

static OPS: &[Row] = &[
    ("setIndent", "xmlwriter_set_indent", Op::SetIndent, 1, 1, &["enable"]),
    ("setIndentString", "xmlwriter_set_indent_string", Op::SetIndentString, 1, 1, &["indentation"]),
    ("startComment", "xmlwriter_start_comment", Op::StartComment, 0, 0, &[]),
    ("endComment", "xmlwriter_end_comment", Op::EndComment, 0, 0, &[]),
    ("startAttribute", "xmlwriter_start_attribute", Op::StartAttribute, 1, 1, &["name"]),
    ("endAttribute", "xmlwriter_end_attribute", Op::EndAttribute, 0, 0, &[]),
    ("writeAttribute", "xmlwriter_write_attribute", Op::WriteAttribute, 2, 2, &["name", "value"]),
    ("startAttributeNs", "xmlwriter_start_attribute_ns", Op::StartAttributeNs, 3, 3, &["prefix", "name", "namespace"]),
    ("writeAttributeNs", "xmlwriter_write_attribute_ns", Op::WriteAttributeNs, 4, 4, &["prefix", "name", "namespace", "value"]),
    ("startElement", "xmlwriter_start_element", Op::StartElement, 1, 1, &["name"]),
    ("endElement", "xmlwriter_end_element", Op::EndElement, 0, 0, &[]),
    ("fullEndElement", "xmlwriter_full_end_element", Op::FullEndElement, 0, 0, &[]),
    ("startElementNs", "xmlwriter_start_element_ns", Op::StartElementNs, 3, 3, &["prefix", "name", "namespace"]),
    ("writeElement", "xmlwriter_write_element", Op::WriteElement, 1, 2, &["name", "content"]),
    ("writeElementNs", "xmlwriter_write_element_ns", Op::WriteElementNs, 3, 4, &["prefix", "name", "namespace", "content"]),
    ("startPi", "xmlwriter_start_pi", Op::StartPi, 1, 1, &["target"]),
    ("endPi", "xmlwriter_end_pi", Op::EndPi, 0, 0, &[]),
    ("writePi", "xmlwriter_write_pi", Op::WritePi, 2, 2, &["target", "content"]),
    ("startCdata", "xmlwriter_start_cdata", Op::StartCdata, 0, 0, &[]),
    ("endCdata", "xmlwriter_end_cdata", Op::EndCdata, 0, 0, &[]),
    ("writeCdata", "xmlwriter_write_cdata", Op::WriteCdata, 1, 1, &["content"]),
    ("text", "xmlwriter_text", Op::Text, 1, 1, &["content"]),
    ("writeRaw", "xmlwriter_write_raw", Op::WriteRaw, 1, 1, &["content"]),
    ("startDocument", "xmlwriter_start_document", Op::StartDocument, 0, 3, &["version", "encoding", "standalone"]),
    ("endDocument", "xmlwriter_end_document", Op::EndDocument, 0, 0, &[]),
    ("writeComment", "xmlwriter_write_comment", Op::WriteComment, 1, 1, &["content"]),
    ("startDtd", "xmlwriter_start_dtd", Op::StartDtd, 1, 3, &["qualifiedName", "publicId", "systemId"]),
    ("endDtd", "xmlwriter_end_dtd", Op::EndDtd, 0, 0, &[]),
    ("writeDtd", "xmlwriter_write_dtd", Op::WriteDtd, 1, 4, &["name", "publicId", "systemId", "content"]),
    ("startDtdElement", "xmlwriter_start_dtd_element", Op::StartDtdElement, 1, 1, &["qualifiedName"]),
    ("endDtdElement", "xmlwriter_end_dtd_element", Op::EndDtdElement, 0, 0, &[]),
    ("writeDtdElement", "xmlwriter_write_dtd_element", Op::WriteDtdElement, 2, 2, &["name", "content"]),
    ("startDtdAttlist", "xmlwriter_start_dtd_attlist", Op::StartDtdAttlist, 1, 1, &["name"]),
    ("endDtdAttlist", "xmlwriter_end_dtd_attlist", Op::EndDtdAttlist, 0, 0, &[]),
    ("writeDtdAttlist", "xmlwriter_write_dtd_attlist", Op::WriteDtdAttlist, 2, 2, &["name", "content"]),
    ("startDtdEntity", "xmlwriter_start_dtd_entity", Op::StartDtdEntity, 2, 2, &["name", "isParam"]),
    ("endDtdEntity", "xmlwriter_end_dtd_entity", Op::EndDtdEntity, 0, 0, &[]),
    (
        "writeDtdEntity",
        "xmlwriter_write_dtd_entity",
        Op::WriteDtdEntity,
        2,
        6,
        &["name", "content", "isParam", "publicId", "systemId", "notationData"],
    ),
    ("outputMemory", "xmlwriter_output_memory", Op::OutputMemory, 0, 1, &["flush"]),
    ("flush", "xmlwriter_flush", Op::Flush, 0, 1, &["empty"]),
];

fn s_arg(args: &[Value], i: usize) -> Vec<u8> {
    args.get(i).map(|v| v.deref().to_php_bytes()).unwrap_or_default()
}

fn opt_arg(args: &[Value], i: usize) -> Option<Vec<u8>> {
    match args.get(i).map(|v| v.deref().into_owned()) {
        None | Some(Value::Null) => None,
        Some(v) => Some(v.to_php_bytes()),
    }
}

fn b_arg(args: &[Value], i: usize, default: bool) -> bool {
    args.get(i).map_or(default, |v| v.deref().to_bool())
}

/// `XMLW_NAME_CHK`: the argument number is the procedural one, the name
/// the one the running function has at that position.
fn name_check(ctx: &Ctx, name: &[u8], argn: usize, subject: &str, params: &[&str], proc: bool) -> Result<(), Unwind> {
    if is_name(name) {
        return Ok(());
    }
    let who = ctx.active_function_name();
    let pname = if proc {
        if argn == 1 { Some("writer") } else { params.get(argn - 2).copied() }
    } else {
        params.get(argn - 1).copied()
    };
    let arg = match pname {
        Some(p) => format!("Argument #{argn} (${p})"),
        None => format!("Argument #{argn}"),
    };
    Err(Unwind::value_error(format!(
        "{who}(): {arg} must be a valid {subject}, \"{}\" given",
        String::from_utf8_lossy(name)
    )))
}

fn run_op(ctx: &mut Ctx, o: &Object, a: &[Value], row: &Row, proc: bool) -> NativeResult {
    let (_, _, op, _, _, params) = *row;
    let shared = shared_of(o);
    if shared.borrow().is_none() {
        return Err(Unwind::error("Invalid or uninitialized XMLWriter object"));
    }
    let chk = |ctx: &Ctx, name: &[u8], argn: usize, subject: &str| name_check(ctx, name, argn, subject, params, proc);
    let r: R = {
        match op {
            Op::StartAttribute => chk(ctx, &s_arg(a, 0), 2, "attribute name")?,
            Op::WriteAttribute => chk(ctx, &s_arg(a, 0), 2, "attribute name")?,
            Op::StartAttributeNs | Op::WriteAttributeNs => chk(ctx, &s_arg(a, 1), 3, "attribute name")?,
            Op::StartElement | Op::WriteElement => chk(ctx, &s_arg(a, 0), 2, "element name")?,
            Op::StartElementNs | Op::WriteElementNs => chk(ctx, &s_arg(a, 1), 3, "element name")?,
            Op::StartPi | Op::WritePi => chk(ctx, &s_arg(a, 0), 2, "PI target")?,
            Op::StartDtdElement | Op::WriteDtdElement | Op::StartDtdAttlist | Op::WriteDtdAttlist => {
                chk(ctx, &s_arg(a, 0), 2, "element name")?
            }
            Op::StartDtdEntity => chk(ctx, &s_arg(a, 0), 2, "attribute name")?,
            Op::WriteDtdEntity => chk(ctx, &s_arg(a, 0), 2, "element name")?,
            _ => {}
        }
        let mut g = shared.borrow_mut();
        let w = g.as_mut().expect("writer");
        match op {
            Op::SetIndent => {
                w.indent = b_arg(a, 0, false);
                Ok(())
            }
            Op::SetIndentString => {
                w.ichar = s_arg(a, 0);
                Ok(())
            }
            Op::StartComment => w.start_comment(),
            Op::EndComment => w.end_comment(),
            Op::StartAttribute => w.start_attribute(&s_arg(a, 0)),
            Op::EndAttribute => w.end_attribute(),
            Op::WriteAttribute => w.write_attribute(&s_arg(a, 0), &s_arg(a, 1)),
            Op::StartAttributeNs => w.start_attribute_ns(opt_arg(a, 0).as_deref(), &s_arg(a, 1), opt_arg(a, 2).as_deref()),
            Op::WriteAttributeNs => w.write_attribute_ns(
                opt_arg(a, 0).as_deref(),
                &s_arg(a, 1),
                opt_arg(a, 2).as_deref(),
                &s_arg(a, 3),
            ),
            Op::StartElement => w.start_element(&s_arg(a, 0)),
            Op::EndElement => w.end_element(),
            Op::FullEndElement => w.full_end_element(),
            Op::StartElementNs => w.start_element_ns(opt_arg(a, 0).as_deref(), &s_arg(a, 1), opt_arg(a, 2).as_deref()),
            Op::WriteElement => {
                let name = s_arg(a, 0);
                match opt_arg(a, 1) {
                    Some(c) => w.start_element(&name).and_then(|_| w.write_string(&c)).and_then(|_| w.end_element()),
                    None => w.start_element(&name).and_then(|_| w.end_element()),
                }
            }
            Op::WriteElementNs => {
                let (p, name, u) = (opt_arg(a, 0), s_arg(a, 1), opt_arg(a, 2));
                w.start_element_ns(p.as_deref(), &name, u.as_deref())
                    .and_then(|_| match opt_arg(a, 3) {
                        Some(c) => w.write_string(&c),
                        None => Ok(()),
                    })
                    .and_then(|_| w.end_element())
            }
            Op::StartPi => w.start_pi(&s_arg(a, 0)),
            Op::EndPi => w.end_pi(),
            Op::WritePi => w.start_pi(&s_arg(a, 0)).and_then(|_| w.write_string(&s_arg(a, 1))).and_then(|_| w.end_pi()),
            Op::StartCdata => w.start_cdata(),
            Op::EndCdata => w.end_cdata(),
            Op::WriteCdata => w.start_cdata().and_then(|_| w.write_string(&s_arg(a, 0))).and_then(|_| w.end_cdata()),
            Op::Text => w.write_string(&s_arg(a, 0)),
            Op::WriteRaw => w.write_raw(&s_arg(a, 0)),
            Op::StartDocument => {
                let version = match a.first().map(|v| v.deref().into_owned()) {
                    None => Some(b"1.0".to_vec()),
                    Some(Value::Null) => None,
                    Some(v) => Some(v.to_php_bytes()),
                };
                w.start_document(version.as_deref(), opt_arg(a, 1).as_deref(), opt_arg(a, 2).as_deref())
            }
            Op::EndDocument => {
                let r = w.end_document();
                drop(g);
                flush_writer(ctx, &shared)?;
                r
            }
            Op::WriteComment => {
                w.start_comment().and_then(|_| w.write_string(&s_arg(a, 0))).and_then(|_| w.end_comment())
            }
            Op::StartDtd => w.start_dtd(&s_arg(a, 0), opt_arg(a, 1).as_deref(), opt_arg(a, 2).as_deref()),
            Op::EndDtd => w.end_dtd(),
            Op::WriteDtd => w.write_dtd(
                &s_arg(a, 0),
                opt_arg(a, 1).as_deref(),
                opt_arg(a, 2).as_deref(),
                opt_arg(a, 3).as_deref(),
            ),
            Op::StartDtdElement => w.start_dtd_element(&s_arg(a, 0)),
            Op::EndDtdElement => w.end_dtd_element(),
            Op::WriteDtdElement => w.write_dtd_element(&s_arg(a, 0), &s_arg(a, 1)),
            Op::StartDtdAttlist => w.start_dtd_attlist(&s_arg(a, 0)),
            Op::EndDtdAttlist => w.end_dtd_attlist(),
            Op::WriteDtdAttlist => w.write_dtd_attlist(&s_arg(a, 0), &s_arg(a, 1)),
            Op::StartDtdEntity => w.start_dtd_entity(b_arg(a, 1, false), &s_arg(a, 0)),
            Op::EndDtdEntity => w.end_dtd_entity(),
            Op::WriteDtdEntity => w.write_dtd_entity(
                b_arg(a, 2, false),
                &s_arg(a, 0),
                opt_arg(a, 3).as_deref(),
                opt_arg(a, 4).as_deref(),
                opt_arg(a, 5).as_deref(),
                Some(&s_arg(a, 1)),
            ),
            Op::OutputMemory | Op::Flush => {
                let empty = b_arg(a, 0, true);
                let is_mem = matches!(w.target, Target::Memory(_));
                if is_mem {
                    let Target::Memory(m) = &mut w.target else { unreachable!() };
                    let out = if empty { std::mem::take(m) } else { m.clone() };
                    // `RETVAL_STRING` of the buffer's C string.
                    let end = out.iter().position(|&b| b == 0).unwrap_or(out.len());
                    return Ok(Value::string(&out[..end]));
                }
                // `outputMemory()` on a URI writer answers "" without
                // flushing.
                if matches!(op, Op::OutputMemory) {
                    return Ok(Value::string(b""));
                }
                drop(g);
                return Ok(Value::Int(flush_writer(ctx, &shared)?));
            }
        }
    };
    auto_flush(ctx, &shared)?;
    match r {
        Ok(()) => Ok(Value::Bool(true)),
        Err(None) => Ok(Value::Bool(false)),
        Err(Some(m)) => {
            let who = ctx.active_function_name();
            ctx.warn(&format!("{who}(): {m}"))?;
            Ok(Value::Bool(false))
        }
    }
}

fn flush_writer(ctx: &mut Ctx, shared: &Shared) -> Result<i64, Unwind> {
    // Take the writer out while a stream write may call back into php.
    let Some(mut w) = shared.borrow_mut().take() else { return Ok(0) };
    let r = w.flush_to(ctx);
    *shared.borrow_mut() = Some(w);
    r
}

/// libxml hands a URI target its buffer once 4000 bytes are pending.
fn auto_flush(ctx: &mut Ctx, shared: &Shared) -> Result<(), Unwind> {
    let big = shared.borrow().as_ref().is_some_and(|w| w.pending.len() >= MINLEN);
    if big {
        flush_writer(ctx, shared)?;
    }
    Ok(())
}

/// Open a writer on `o` (`openMemory` / `openUri` / `toStream`).
fn open(ctx: &mut Ctx, o: &Object, target: Target) {
    let shared = shared_of(o);
    let old = shared.borrow_mut().take();
    if let Some(mut w) = old {
        let _ = w.flush_to(ctx);
    }
    *shared.borrow_mut() = Some(Writer::new(target));
}

/// Resolve and create a URI target: a local file, or a php stream for a
/// wrapper URL.
fn uri_target(ctx: &mut Ctx, uri: &[u8]) -> Result<Option<Target>, Unwind> {
    let s = String::from_utf8_lossy(uri).into_owned();
    if s.contains("://") && !s.starts_with("file://") {
        let v = ctx.call_function(b"fopen", &[Value::string(uri), Value::string(b"wb")])?;
        return Ok(match v {
            Value::Resource(_) => Some(Target::Stream(v)),
            _ => None,
        });
    }
    let path = s.strip_prefix("file://").unwrap_or(&s);
    let full = if std::path::Path::new(path).is_absolute() {
        std::path::PathBuf::from(path)
    } else {
        ctx.cwd.join(path)
    };
    match std::fs::File::create(&full) {
        Ok(f) => {
            ctx.ext.stat_cache = None;
            Ok(Some(Target::File(f)))
        }
        Err(_) => Ok(None),
    }
}

fn open_uri_common(ctx: &mut Ctx, o: &Object, args: &[Value]) -> Result<bool, Unwind> {
    let uri = s_arg(args, 0);
    let who = ctx.active_function_name();
    if uri.is_empty() {
        return Err(Unwind::value_error(format!("{who}(): Argument #1 ($uri) must not be empty")));
    }
    match uri_target(ctx, &uri)? {
        Some(t) => {
            open(ctx, o, t);
            Ok(true)
        }
        None => {
            ctx.warn(&format!("{who}(): Unable to resolve file path"))?;
            Ok(false)
        }
    }
}

fn m_open_memory(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    open(ctx, o, Target::Memory(Vec::new()));
    Ok(Value::Bool(true))
}

fn m_open_uri(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Bool(open_uri_common(ctx, o, args)?))
}

fn new_instance(ctx: &mut Ctx) -> Result<Object, Unwind> {
    // A native static method does not see the late-static-bound class,
    // so a subclass's `toMemory()` makes an `XMLWriter`.
    let cid = ctx.lookup_class_or_error(b"XMLWriter")?;
    Ok(ctx.instantiate(cid))
}

fn m_to_memory(ctx: &mut Ctx, _: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = new_instance(ctx)?;
    open(ctx, &o, Target::Memory(Vec::new()));
    Ok(Value::Object(o))
}

fn m_to_uri(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let uri = s_arg(args, 0);
    if uri.is_empty() {
        return Err(Unwind::value_error("XMLWriter::toUri(): Argument #1 ($uri) must not be empty"));
    }
    let Some(t) = uri_target(ctx, &uri)? else {
        return Err(Unwind::exception("Exception", "XMLWriter::toUri(): Unable to resolve file path"));
    };
    let o = new_instance(ctx)?;
    open(ctx, &o, t);
    Ok(Value::Object(o))
}

fn m_to_stream(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let v = args.first().map(|v| v.deref().into_owned()).unwrap_or(Value::Null);
    if !matches!(v, Value::Resource(_)) {
        return Err(Unwind::type_error(format!(
            "XMLWriter::toStream(): Argument #1 ($stream) must be of type resource, {} given",
            rphp_runtime::value_name(&v)
        )));
    }
    let o = new_instance(ctx)?;
    open(ctx, &o, Target::Stream(v));
    Ok(Value::Object(o))
}

fn f_open_memory(ctx: &mut Ctx, _: &mut [Value]) -> NativeResult {
    let cid = ctx.lookup_class_or_error(b"XMLWriter")?;
    let o = ctx.instantiate(cid);
    open(ctx, &o, Target::Memory(Vec::new()));
    Ok(Value::Object(o))
}

fn f_open_uri(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let cid = ctx.lookup_class_or_error(b"XMLWriter")?;
    let o = ctx.instantiate(cid);
    Ok(if open_uri_common(ctx, &o, args)? { Value::Object(o) } else { Value::Bool(false) })
}

fn this(o: Option<&Object>) -> Result<&Object, Unwind> {
    o.ok_or_else(|| Unwind::error("Non-static method called statically"))
}

macro_rules! ops {
    ($($i:literal => $m:ident, $f:ident;)*) => {
        $(
            fn $m(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
                let o = this(o)?;
                run_op(ctx, o, args, &OPS[$i], false)
            }
            fn $f(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
                let o = writer_value_arg(ctx, args.first())?;
                run_op(ctx, &o, &args[1..], &OPS[$i], true)
            }
        )*
        static METHOD_FNS: &[rphp_runtime::NativeMethodHandler] = &[$($m),*];
        static PROC_FNS: &[rphp_runtime::NativeHandler] = &[$($f),*];
    };
}

ops! {
    0 => m0, p0; 1 => m1, p1; 2 => m2, p2; 3 => m3, p3; 4 => m4, p4; 5 => m5, p5; 6 => m6, p6;
    7 => m7, p7; 8 => m8, p8; 9 => m9, p9; 10 => m10, p10; 11 => m11, p11; 12 => m12, p12;
    13 => m13, p13; 14 => m14, p14; 15 => m15, p15; 16 => m16, p16; 17 => m17, p17; 18 => m18, p18;
    19 => m19, p19; 20 => m20, p20; 21 => m21, p21; 22 => m22, p22; 23 => m23, p23; 24 => m24, p24;
    25 => m25, p25; 26 => m26, p26; 27 => m27, p27; 28 => m28, p28; 29 => m29, p29; 30 => m30, p30;
    31 => m31, p31; 32 => m32, p32; 33 => m33, p33; 34 => m34, p34; 35 => m35, p35; 36 => m36, p36;
    37 => m37, p37; 38 => m38, p38; 39 => m39, p39;
}

pub fn register(r: &mut Registry) {
    let mut b = r
        .class("XMLWriter")
        .method("openMemory", rphp_runtime::nm!(0, Some(0), m_open_memory))
        .method("openUri", rphp_runtime::nm!(1, Some(1), m_open_uri))
        .method("toMemory", static_m(0, 0, m_to_memory))
        .method("toUri", static_m(1, 1, m_to_uri))
        .method("toStream", static_m(1, 1, m_to_stream));
    for (i, row) in OPS.iter().enumerate() {
        b = b.method(
            row.0,
            NativeMethod {
                handler: METHOD_FNS[i],
                min_args: row.3,
                max_args: Some(row.4),
                params: row.5,
                by_ref: 0,
                is_static: false,
                is_final: false,
            },
        );
    }
    b.finish();
    let mut fns = vec![
        NativeFn {
            name: "xmlwriter_open_memory",
            min_args: 0,
            max_args: Some(0),
            by_ref: 0,
            params: &[],
            flags: rphp_runtime::FnFlags::EMPTY,
            handler: f_open_memory,
        },
        NativeFn {
            name: "xmlwriter_open_uri",
            min_args: 1,
            max_args: Some(1),
            by_ref: 0,
            params: &["uri"],
            flags: rphp_runtime::FnFlags::EMPTY,
            handler: f_open_uri,
        },
    ];
    for (i, row) in OPS.iter().enumerate() {
        fns.push(NativeFn {
            name: row.1,
            min_args: row.3 + 1,
            max_args: Some(row.4 + 1),
            by_ref: 0,
            params: &[],
            flags: rphp_runtime::FnFlags::EMPTY,
            handler: PROC_FNS[i],
        });
    }
    for f in fns {
        r.function(f);
    }
}

fn static_m(min: u8, max: u8, f: rphp_runtime::NativeMethodHandler) -> NativeMethod {
    NativeMethod {
        handler: f,
        min_args: min,
        max_args: Some(max),
        params: &[],
        by_ref: 0,
        is_static: true,
        is_final: false,
    }
}
