//! XML serialization the way libxml's `xmlNodeDumpOutput` does it — the
//! escaping per context, the `formatOutput` indentation rule (an element
//! whose children include text, CDATA or an entity reference is printed
//! inline), the declaration line, the doctype with its internal subset.

use crate::tree::{DocData, DtdDecl, NodeId, NodeKind, DOCUMENT};

/// XHTML's empty elements (libxml's `XML_SAVE_XHTML` writes `<br />`
/// for these and `<x></x>` for every other empty element).
const XHTML_EMPTY: &[&[u8]] = &[
    b"area",
    b"base",
    b"basefont",
    b"br",
    b"col",
    b"frame",
    b"hr",
    b"img",
    b"input",
    b"isindex",
    b"link",
    b"meta",
    b"param",
];

/// The serializer's state: the declarations it emitted itself on the
/// way down (namespace reconciliation), and the XHTML rules for a modern
/// HTML document.
struct Ser {
    emitted: Vec<(NodeId, Option<Vec<u8>>, Vec<u8>)>,
    xhtml: bool,
    /// The subtree being serialized on its own (php 8.4's well-formed
    /// subtree serialization): declarations above it are not in scope,
    /// so the ones it uses get declared on it.
    scope_root: Option<NodeId>,
}

impl Ser {
    fn new(doc: &DocData) -> Ser {
        Ser {
            emitted: Vec::new(),
            xhtml: doc.modern && doc.is_html,
            scope_root: None,
        }
    }
}

/// `saveXML()` of the whole document.
pub fn document(doc: &DocData) -> Vec<u8> {
    let mut out = Vec::new();
    let mut ser = Ser::new(doc);
    out.extend_from_slice(b"<?xml version=\"");
    out.extend_from_slice(doc.version.as_deref().unwrap_or(b"1.0"));
    out.push(b'"');
    if let Some(enc) = &doc.encoding {
        out.extend_from_slice(b" encoding=\"");
        out.extend_from_slice(enc);
        out.push(b'"');
    }
    if let Some(sa) = doc.standalone {
        out.extend_from_slice(if sa {
            b" standalone=\"yes\""
        } else {
            b" standalone=\"no\""
        });
    }
    out.extend_from_slice(b"?>\n");
    let kids = doc.children(DOCUMENT);
    for (i, &c) in kids.iter().enumerate() {
        node_in(doc, c, doc.format_output, 0, &mut out, &mut ser);
        // php 8.4's `Dom\Document::saveXml()` ends without a newline.
        if !doc.modern || i + 1 < kids.len() {
            out.push(b'\n');
        }
    }
    if let Some(enc) = &doc.encoding {
        if enc.eq_ignore_ascii_case(b"ISO-8859-1") || enc.eq_ignore_ascii_case(b"latin1") {
            return utf8_to_latin1(&out);
        }
    }
    out
}

/// `saveXML($node)`: one node, no declaration, always UTF-8.
pub fn fragment(doc: &DocData, id: NodeId) -> Vec<u8> {
    let mut out = Vec::new();
    let mut ser = Ser::new(doc);
    if doc.modern {
        ser.scope_root = Some(id);
    }
    node_in(doc, id, doc.format_output, 0, &mut out, &mut ser);
    out
}

#[allow(dead_code)]
pub fn node(doc: &DocData, id: NodeId, format: bool, level: usize, out: &mut Vec<u8>) {
    let mut ser = Ser::new(doc);
    node_in(doc, id, format, level, out, &mut ser);
}

fn indent(out: &mut Vec<u8>, level: usize) {
    for _ in 0..level {
        out.extend_from_slice(b"  ");
    }
}

fn node_in(
    doc: &DocData,
    id: NodeId,
    format: bool,
    level: usize,
    out: &mut Vec<u8>,
    ser: &mut Ser,
) {
    let n = doc.node(id);
    match n.kind {
        NodeKind::Document => {
            for c in doc.children(id) {
                node_in(doc, c, format, level, out, ser);
                out.push(b'\n');
            }
        }
        NodeKind::Fragment => {
            for c in doc.children(id) {
                node_in(doc, c, format, level, out, ser);
            }
        }
        NodeKind::Element => element(doc, id, format, level, out, ser),
        NodeKind::Text => escape_text(&n.value, out),
        NodeKind::CData => {
            out.extend_from_slice(b"<![CDATA[");
            out.extend_from_slice(&n.value);
            out.extend_from_slice(b"]]>");
        }
        NodeKind::Comment => {
            out.extend_from_slice(b"<!--");
            out.extend_from_slice(&n.value);
            out.extend_from_slice(b"-->");
        }
        NodeKind::Pi => {
            out.extend_from_slice(b"<?");
            out.extend_from_slice(&n.name);
            if !n.value.is_empty() {
                out.push(b' ');
                out.extend_from_slice(&n.value);
            }
            out.extend_from_slice(b"?>");
        }
        NodeKind::EntityRef => {
            out.push(b'&');
            out.extend_from_slice(&n.name);
            out.push(b';');
        }
        NodeKind::Attribute => {
            // php's `saveXML($attr)` prints the value alone.
            out.extend_from_slice(&doc.attr_value(id));
        }
        NodeKind::DocumentType => {
            doctype(doc, id, out);
            // php 8.4's subtree serialization ends a doctype with a newline.
            if doc.modern && ser.scope_root == Some(id) {
                out.push(b'\n');
            }
        }
        NodeKind::Entity => {
            if let Some(decl) = &n.entity {
                entity_decl(decl, false, out);
            }
        }
        NodeKind::Notation => {}
    }
}

/// `<!ENTITY [%] name …>` and its newline, as libxml prints one.
fn entity_decl(decl: &crate::tree::EntityDecl, parameter: bool, out: &mut Vec<u8>) {
    out.extend_from_slice(b"<!ENTITY ");
    if parameter {
        out.extend_from_slice(b"% ");
    }
    out.extend_from_slice(&decl.name);
    if decl.value.is_some() {
        let orig = &decl.orig;
        let q = if orig.contains(&b'"') && !orig.contains(&b'\'') {
            b'\''
        } else {
            b'"'
        };
        out.push(b' ');
        out.push(q);
        out.extend_from_slice(orig);
        out.push(q);
    } else {
        match (&decl.public_id, &decl.system_id) {
            (Some(p), sys) => {
                out.extend_from_slice(b" PUBLIC \"");
                out.extend_from_slice(p);
                out.extend_from_slice(b"\" \"");
                out.extend_from_slice(sys.as_deref().unwrap_or_default());
                out.push(b'"');
            }
            (None, Some(sys)) => {
                out.extend_from_slice(b" SYSTEM \"");
                out.extend_from_slice(sys);
                out.push(b'"');
            }
            (None, None) => {}
        }
        if let Some(n) = &decl.notation {
            out.extend_from_slice(b" NDATA ");
            out.extend_from_slice(n);
        }
    }
    out.extend_from_slice(b">\n");
}

fn doctype(doc: &DocData, id: NodeId, out: &mut Vec<u8>) {
    let n = doc.node(id);
    out.extend_from_slice(b"<!DOCTYPE ");
    out.extend_from_slice(&n.name);
    if let Some(info) = &n.doctype {
        if !info.public_id.is_empty() {
            out.extend_from_slice(b" PUBLIC \"");
            out.extend_from_slice(&info.public_id);
            out.extend_from_slice(b"\" \"");
            out.extend_from_slice(&info.system_id);
            out.push(b'"');
        } else if !info.system_id.is_empty() {
            out.extend_from_slice(b" SYSTEM \"");
            out.extend_from_slice(&info.system_id);
            out.push(b'"');
        }
        // libxml re-serializes the internal subset from its declarations:
        // notations first, then the rest in order, one per line (a comment
        // or PI without the newline).
        if !info.notations.is_empty() || !info.decls.is_empty() {
            out.extend_from_slice(b" [\n");
            for (name, public_id, system_id) in &info.notations {
                out.extend_from_slice(b"<!NOTATION ");
                out.extend_from_slice(name);
                if !public_id.is_empty() {
                    out.extend_from_slice(b" PUBLIC \"");
                    out.extend_from_slice(public_id);
                    out.push(b'"');
                    if !system_id.is_empty() {
                        out.extend_from_slice(b" \"");
                        out.extend_from_slice(system_id);
                        out.push(b'"');
                    }
                } else {
                    out.extend_from_slice(b" SYSTEM \"");
                    out.extend_from_slice(system_id);
                    out.push(b'"');
                }
                out.extend_from_slice(b" >\n");
            }
            for decl in &info.decls {
                match decl {
                    DtdDecl::Entity { decl, parameter } => entity_decl(decl, *parameter, out),
                    DtdDecl::Element { name, content } => {
                        out.extend_from_slice(b"<!ELEMENT ");
                        out.extend_from_slice(name);
                        out.push(b' ');
                        out.extend_from_slice(content);
                        out.extend_from_slice(b">\n");
                    }
                    DtdDecl::Attlist { element, attrs } => {
                        for (name, ty, default) in attrs {
                            out.extend_from_slice(b"<!ATTLIST ");
                            out.extend_from_slice(element);
                            out.push(b' ');
                            out.extend_from_slice(name);
                            out.push(b' ');
                            out.extend_from_slice(ty);
                            if !default.is_empty() {
                                out.push(b' ');
                                out.extend_from_slice(default);
                            }
                            out.extend_from_slice(b">\n");
                        }
                    }
                    DtdDecl::Comment(text) => {
                        out.extend_from_slice(b"<!--");
                        out.extend_from_slice(text);
                        out.extend_from_slice(b"-->");
                    }
                    DtdDecl::Pi { target, data } => {
                        out.extend_from_slice(b"<?");
                        out.extend_from_slice(target);
                        if !data.is_empty() {
                            out.push(b' ');
                            out.extend_from_slice(data);
                        }
                        out.extend_from_slice(b"?>");
                    }
                }
            }
            out.push(b']');
        }
    }
    out.push(b'>');
}

/// The declaration in scope for `prefix` at `id`: the node's own
/// declarations and its ancestors', including the ones the serializer
/// emitted itself on the way down.
fn in_scope(doc: &DocData, id: NodeId, prefix: Option<&[u8]>, ser: &Ser) -> Option<Vec<u8>> {
    if prefix == Some(b"xml") {
        return None;
    }
    let mut cur = Some(id);
    while let Some(n) = cur {
        let node = doc.node(n);
        if node.kind == NodeKind::Element {
            for d in &node.ns_decls {
                if d.prefix.as_deref() == prefix {
                    return Some(d.uri.clone());
                }
            }
            for (at, p, uri) in ser.emitted.iter().rev() {
                if *at == n && p.as_deref() == prefix {
                    return Some(uri.clone());
                }
            }
        }
        if ser.scope_root == Some(n) {
            break;
        }
        cur = node.parent;
    }
    None
}

fn element(
    doc: &DocData,
    id: NodeId,
    format: bool,
    level: usize,
    out: &mut Vec<u8>,
    ser: &mut Ser,
) {
    let n = doc.node(id);
    let qname = n.qualified_name();
    out.push(b'<');
    out.extend_from_slice(&qname);
    let mut declared: Vec<Option<Vec<u8>>> = Vec::new();
    for d in &n.ns_decls {
        out.extend_from_slice(b" xmlns");
        if let Some(p) = &d.prefix {
            out.push(b':');
            out.extend_from_slice(p);
        }
        out.extend_from_slice(b"=\"");
        escape_attr(&d.uri, out);
        out.push(b'"');
        declared.push(d.prefix.clone());
    }
    // A namespace the node carries without a declaration in scope (an
    // imported subtree) is declared here, as libxml's reconciliation does.
    let mut needed: Vec<(Option<Vec<u8>>, Vec<u8>)> = Vec::new();
    if let Some(uri) = &n.ns {
        if !declared.contains(&n.prefix)
            && in_scope(doc, id, n.prefix.as_deref(), ser).as_deref() != Some(uri.as_slice())
        {
            needed.push((n.prefix.clone(), uri.clone()));
        }
    }
    for &a in &n.attrs {
        let an = doc.node(a);
        if let (Some(uri), Some(p)) = (&an.ns, &an.prefix) {
            if p.as_slice() != b"xml"
                && !declared.contains(&Some(p.clone()))
                && !needed.iter().any(|(np, _)| np.as_ref() == Some(p))
                && in_scope(doc, id, Some(p), ser).as_deref() != Some(uri.as_slice())
            {
                needed.push((Some(p.clone()), uri.clone()));
            }
        }
    }
    let emitted_before = ser.emitted.len();
    for (prefix, uri) in &needed {
        out.extend_from_slice(b" xmlns");
        if let Some(p) = prefix {
            out.push(b':');
            out.extend_from_slice(p);
        }
        out.extend_from_slice(b"=\"");
        escape_attr(uri, out);
        out.push(b'"');
        ser.emitted.push((id, prefix.clone(), uri.clone()));
    }
    for &a in &n.attrs {
        out.push(b' ');
        out.extend_from_slice(&doc.node(a).qualified_name());
        out.extend_from_slice(b"=\"");
        attr_content(doc, a, out);
        out.push(b'"');
    }
    let children = doc.children(id);
    if children.is_empty() {
        if ser.xhtml && !XHTML_EMPTY.contains(&n.name.as_slice()) {
            out.extend_from_slice(b"></");
            out.extend_from_slice(&qname);
            out.push(b'>');
        } else if ser.xhtml {
            out.extend_from_slice(b" />");
        } else {
            out.extend_from_slice(b"/>");
        }
        ser.emitted.truncate(emitted_before);
        return;
    }
    out.push(b'>');
    // libxml: formatting stops at an element with text-like children.
    let format = format
        && !children.iter().any(|&c| {
            matches!(
                doc.node(c).kind,
                NodeKind::Text | NodeKind::CData | NodeKind::EntityRef
            )
        });
    if format {
        out.push(b'\n');
    }
    for c in children {
        if format {
            indent(out, level + 1);
        }
        node_in(doc, c, format, level + 1, out, ser);
        if format {
            out.push(b'\n');
        }
    }
    if format {
        indent(out, level);
    }
    out.extend_from_slice(b"</");
    out.extend_from_slice(&qname);
    out.push(b'>');
    ser.emitted.truncate(emitted_before);
}

/// An attribute's children: text escaped for an attribute value, entity
/// references as written.
fn attr_content(doc: &DocData, attr: NodeId, out: &mut Vec<u8>) {
    for c in doc.children(attr) {
        let n = doc.node(c);
        match n.kind {
            NodeKind::EntityRef => {
                out.push(b'&');
                out.extend_from_slice(&n.name);
                out.push(b';');
            }
            _ => escape_attr(&n.value, out),
        }
    }
}

pub fn escape_text(v: &[u8], out: &mut Vec<u8>) {
    for &b in v {
        match b {
            b'<' => out.extend_from_slice(b"&lt;"),
            b'>' => out.extend_from_slice(b"&gt;"),
            b'&' => out.extend_from_slice(b"&amp;"),
            b'\r' => out.extend_from_slice(b"&#xD;"),
            _ => out.push(b),
        }
    }
}

pub fn escape_attr(v: &[u8], out: &mut Vec<u8>) {
    for &b in v {
        match b {
            b'<' => out.extend_from_slice(b"&lt;"),
            b'>' => out.extend_from_slice(b"&gt;"),
            b'&' => out.extend_from_slice(b"&amp;"),
            b'"' => out.extend_from_slice(b"&quot;"),
            b'\n' => out.extend_from_slice(b"&#10;"),
            b'\r' => out.extend_from_slice(b"&#13;"),
            b'\t' => out.extend_from_slice(b"&#9;"),
            _ => out.push(b),
        }
    }
}

/// UTF-8 → ISO-8859-1, characters past it as decimal character references
/// (libxml's output encoder fallback).
fn utf8_to_latin1(v: &[u8]) -> Vec<u8> {
    let s = String::from_utf8_lossy(v);
    let mut out = Vec::with_capacity(v.len());
    for c in s.chars() {
        let n = c as u32;
        if n < 256 {
            out.push(n as u8);
        } else {
            out.extend_from_slice(format!("&#{n};").as_bytes());
        }
    }
    out
}
