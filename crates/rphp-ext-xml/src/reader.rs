//! `ext/xmlreader`: `XMLReader` as a cursor over the tree `rphp-ext-dom`'s
//! parser builds — the node sequence libxml2's `xmlTextReader` produces
//! (an element, its children, its `END_ELEMENT` unless it was written
//! `<e/>`; whitespace-only text as `SIGNIFICANT_WHITESPACE`; an entity
//! reference as one `ENTITY_REF` node unless entities are substituted),
//! the attribute cursor (namespace declarations first, then the
//! attributes), `readInnerXml()` / `readOuterXml()` / `expand()` over a copy
//! that declares the namespaces it uses on its root (`xmlDocCopyNode`), and
//! libxml's diagnostics as php prints them: `file:line: parser error :
//! message`, the offending line, a caret.
//!
//! The whole document is parsed at the first `read()` (php's reader
//! pushes 512-byte chunks, so a small document behaves the same: an error
//! anywhere in it fails the first `read()`).

use std::cell::RefCell;
use std::rc::Rc;

use rphp_ext_dom::{Doc, DocData, NodeId, NodeKind, NsDecl, DOCUMENT};
use rphp_runtime::{Ctx, Interp, NativeMethod, NativeResult, Registry, Unwind};
use rphp_value::{Object, Payload, Value};

const XMLNS_NS: &[u8] = b"http://www.w3.org/2000/xmlns/";
const XML_NS: &[u8] = b"http://www.w3.org/XML/1998/namespace";

const LIBXML_NOENT: i64 = 2;
const LIBXML_NOBLANKS: i64 = 256;
const LIBXML_NOCDATA: i64 = 16384;

const PROP_LOADDTD: i64 = 1;
const PROP_DEFAULTATTRS: i64 = 2;
const PROP_VALIDATE: i64 = 3;
const PROP_SUBST_ENTITIES: i64 = 4;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Cur {
    Start,
    /// A node; `true` for an element's `END_ELEMENT`.
    Node(NodeId, bool),
    /// The `i`-th attribute (namespace declarations first) of an element.
    Attr(NodeId, usize),
    Done,
}

struct Loaded {
    src: Vec<u8>,
    base: Vec<u8>,
    flags: i64,
    props: [bool; 4],
    doc: Option<Doc>,
    failed: bool,
    cur: Cur,
}

#[derive(Default)]
pub struct Reader {
    data: Option<Loaded>,
}

type Shared = Rc<RefCell<Reader>>;

fn shared_of(o: &Object) -> Shared {
    if let Some(s) = o.with_payload::<Shared, _>(|s| s.clone()) {
        return s;
    }
    let s: Shared = Default::default();
    o.set_payload(Payload::Native(Box::new(s.clone())));
    s
}

fn this(o: Option<&Object>) -> Result<&Object, Unwind> {
    o.ok_or_else(|| Unwind::error("Non-static method called statically"))
}

fn s_arg(args: &[Value], i: usize) -> Vec<u8> {
    args.get(i).map(|v| v.deref().to_php_bytes()).unwrap_or_default()
}

fn opt_s(v: Option<Vec<u8>>) -> Value {
    match v {
        Some(s) => Value::string(&s),
        None => Value::Null,
    }
}

// ---- the node model ------------------------------------------------------

/// One entry of an element's attribute list.
enum AttrRef {
    Ns(NsDecl),
    Attr(NodeId),
}

fn attr_list(d: &DocData, elem: NodeId) -> Vec<AttrRef> {
    let n = d.node(elem);
    let mut v: Vec<AttrRef> = n.ns_decls.iter().cloned().map(AttrRef::Ns).collect();
    v.extend(n.attrs.iter().map(|&a| AttrRef::Attr(a)));
    v
}

fn is_blank_text(v: &[u8]) -> bool {
    v.iter().all(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r'))
}

/// The node's element depth (document children are at 0).
fn depth_of(d: &DocData, id: NodeId) -> i64 {
    let mut n = 0;
    let mut p = d.node(id).parent;
    while let Some(pid) = p {
        if pid == DOCUMENT || d.node(pid).kind == NodeKind::Document {
            break;
        }
        n += 1;
        p = d.node(pid).parent;
    }
    n
}

struct Info {
    node_type: i64,
    name: Vec<u8>,
    local: Vec<u8>,
    prefix: Vec<u8>,
    ns: Vec<u8>,
    value: Vec<u8>,
    has_value: bool,
    depth: i64,
    empty: bool,
    has_attrs: bool,
    attr_count: i64,
    lang: Vec<u8>,
}

fn lang_of(d: &DocData, mut id: NodeId) -> Vec<u8> {
    loop {
        let n = d.node(id);
        if n.kind == NodeKind::Element {
            for &a in &n.attrs {
                let an = d.node(a);
                if an.name.as_slice() == b"lang" && an.prefix.as_deref() == Some(b"xml".as_slice()) {
                    return d.attr_value(a);
                }
            }
        }
        match n.parent {
            Some(p) => id = p,
            None => return Vec::new(),
        }
    }
}

fn info(d: &DocData, cur: Cur) -> Option<Info> {
    let (id, end, attr) = match cur {
        Cur::Node(id, end) => (id, end, None),
        Cur::Attr(e, i) => (e, false, Some(i)),
        _ => return None,
    };
    let n = d.node(id);
    let depth = depth_of(d, id);
    let lang = lang_of(d, id);
    if let Some(i) = attr {
        let list = attr_list(d, id);
        let a = list.get(i)?;
        let (name, local, prefix, ns, value) = match a {
            AttrRef::Ns(decl) => match &decl.prefix {
                Some(p) => ([b"xmlns:".as_slice(), p].concat(), p.clone(), b"xmlns".to_vec(), XMLNS_NS.to_vec(), decl.uri.clone()),
                None => (b"xmlns".to_vec(), b"xmlns".to_vec(), Vec::new(), XMLNS_NS.to_vec(), decl.uri.clone()),
            },
            AttrRef::Attr(a) => {
                let an = d.node(*a);
                (an.qualified_name(), an.name.clone(), an.prefix.clone().unwrap_or_default(), an.ns.clone().unwrap_or_default(), d.attr_value(*a))
            }
        };
        return Some(Info {
            node_type: 2,
            name,
            local,
            prefix,
            ns,
            value,
            has_value: true,
            depth: depth + 1,
            empty: false,
            has_attrs: false,
            attr_count: 0,
            lang,
        });
    }
    let named = |t: i64, name: &[u8], value: Vec<u8>, has_value: bool| Info {
        node_type: t,
        name: name.to_vec(),
        local: name.to_vec(),
        prefix: Vec::new(),
        ns: Vec::new(),
        value,
        has_value,
        depth,
        empty: false,
        has_attrs: false,
        attr_count: 0,
        lang: lang.clone(),
    };
    Some(match n.kind {
        NodeKind::Element => {
            let count = (n.attrs.len() + n.ns_decls.len()) as i64;
            Info {
                node_type: if end { 15 } else { 1 },
                name: n.qualified_name(),
                local: n.name.clone(),
                prefix: n.prefix.clone().unwrap_or_default(),
                ns: n.ns.clone().unwrap_or_default(),
                value: Vec::new(),
                has_value: false,
                depth,
                empty: !end && n.self_closing && n.first_child.is_none(),
                has_attrs: count > 0,
                attr_count: if end { 0 } else { count },
                lang: lang.clone(),
            }
        }
        NodeKind::Text if is_blank_text(&n.value) => named(14, b"#text", n.value.clone(), true),
        NodeKind::Text => named(3, b"#text", n.value.clone(), true),
        NodeKind::CData => named(4, b"#cdata-section", n.value.clone(), true),
        NodeKind::EntityRef => named(5, &n.name, Vec::new(), false),
        NodeKind::Pi => named(7, &n.name, n.value.clone(), true),
        NodeKind::Comment => named(8, b"#comment", n.value.clone(), true),
        NodeKind::DocumentType => named(10, &n.name, Vec::new(), false),
        _ => named(0, b"", Vec::new(), false),
    })
}

/// The node after `id` in reading order, when `id` itself is done.
fn after(d: &DocData, id: NodeId) -> Cur {
    let n = d.node(id);
    if let Some(s) = n.next {
        return Cur::Node(s, false);
    }
    match n.parent {
        Some(p) if d.node(p).kind == NodeKind::Element => Cur::Node(p, true),
        _ => Cur::Done,
    }
}

/// `xmlTextReaderRead`: the next position.
fn advance(d: &DocData, cur: Cur) -> Cur {
    match cur {
        Cur::Start => match d.node(DOCUMENT).first_child {
            Some(c) => Cur::Node(c, false),
            None => Cur::Done,
        },
        Cur::Node(id, false) | Cur::Attr(id, _) => {
            let n = d.node(id);
            if n.kind == NodeKind::Element {
                if let Some(c) = n.first_child {
                    return Cur::Node(c, false);
                }
                if !n.self_closing {
                    return Cur::Node(id, true);
                }
            }
            after(d, id)
        }
        Cur::Node(id, true) => after(d, id),
        Cur::Done => Cur::Done,
    }
}

// ---- copies (readInnerXml, readOuterXml, expand) --------------------------

/// `xmlDocCopyNode` into `dst`: the copy's root declares every namespace
/// the subtree uses but does not declare itself. Not `deep` at an
/// `END_ELEMENT`: the streaming reader has freed the children it passed.
fn copy_into(dst: &mut DocData, src: &DocData, id: NodeId, deep: bool) -> NodeId {
    let root = dst.import(src, id, deep);
    if dst.node(root).kind == NodeKind::Element {
        let mut stack = vec![root];
        let mut order = Vec::new();
        while let Some(n) = stack.pop() {
            order.push(n);
            let mut kids = dst.children(n);
            kids.reverse();
            stack.extend(kids.into_iter().filter(|&k| dst.node(k).kind == NodeKind::Element));
        }
        for el in order {
            let mut uses: Vec<(Option<Vec<u8>>, Vec<u8>)> = Vec::new();
            let en = dst.node(el);
            if let Some(u) = &en.ns {
                uses.push((en.prefix.clone(), u.clone()));
            }
            for &a in &en.attrs {
                let an = dst.node(a);
                if let (Some(p), Some(u)) = (&an.prefix, &an.ns) {
                    uses.push((Some(p.clone()), u.clone()));
                }
            }
            for (prefix, uri) in uses {
                if prefix.as_deref() == Some(b"xml".as_slice()) || uri.as_slice() == XML_NS {
                    continue;
                }
                if declared(dst, el, prefix.as_deref()) {
                    continue;
                }
                dst.node_mut(root).ns_decls.push(NsDecl { prefix, uri });
            }
        }
    }
    root
}

fn declared(d: &DocData, mut id: NodeId, prefix: Option<&[u8]>) -> bool {
    loop {
        let n = d.node(id);
        if n.ns_decls.iter().any(|x| x.prefix.as_deref() == prefix) {
            return true;
        }
        match n.parent {
            Some(p) => id = p,
            None => return false,
        }
    }
}

fn dump(src: &DocData, id: NodeId, deep: bool) -> Vec<u8> {
    let tmp = DocData::new();
    let mut t = tmp.borrow_mut();
    let c = copy_into(&mut t, src, id, deep);
    let mut out = Vec::new();
    rphp_ext_dom::serialize_node(&t, c, false, 0, &mut out);
    out
}

/// `xmlTextReaderCollectSiblings`.
fn collect(d: &DocData, first: Option<NodeId>, out: &mut Vec<u8>) {
    let mut c = first;
    while let Some(id) = c {
        let n = d.node(id);
        match n.kind {
            NodeKind::Text | NodeKind::CData => out.extend_from_slice(&n.value),
            NodeKind::Element => collect(d, n.first_child, out),
            _ => {}
        }
        c = n.next;
    }
}

// ---- diagnostics ------------------------------------------------------------

/// libxml's `xmlParserPrintFileContextInternal`: the line around `cur`
/// (at most 80 bytes back) and a caret under it.
fn context(src: &[u8], cur: usize) -> (Vec<u8>, Vec<u8>) {
    let at = |i: usize| src.get(i).copied().unwrap_or(0);
    let cur = cur.min(src.len());
    let mut c = cur;
    while c > 0 && matches!(at(c), b'\n' | b'\r') {
        c -= 1;
    }
    let mut n = 0;
    while n < 80 && c > 0 && !matches!(at(c), b'\n' | b'\r') {
        n += 1;
        c -= 1;
    }
    if matches!(at(c), b'\n' | b'\r') {
        c += 1;
    }
    let col = cur.saturating_sub(c);
    let mut content = Vec::new();
    let mut i = c;
    while at(i) != 0 && !matches!(at(i), b'\n' | b'\r') && content.len() < 80 {
        content.push(at(i));
        i += 1;
    }
    let mut caret = Vec::new();
    for &b in content.iter().take(col.min(79)) {
        caret.push(if b == b'\t' { b'\t' } else { b' ' });
    }
    caret.push(b'^');
    (content, caret)
}

fn report(ctx: &mut Ctx, who: &str, mut errors: Vec<rphp_ext_dom::XmlError>, src: &[u8], base: &[u8]) -> Result<(), Unwind> {
    // The reader's push parser stops at the first fatal error; the pull
    // parser behind the tree words a missing root differently.
    if let Some(i) = errors.iter().position(|e| e.level == 3) {
        errors.truncate(i + 1);
        if errors[i].message.starts_with("Start tag expected") {
            errors[i].message = "Document is empty\n".to_string();
        }
    }
    if rphp_ext_dom::libxml_state(ctx).internal_errors {
        rphp_ext_dom::libxml_state(ctx).errors.extend(errors);
        return Ok(());
    }
    for e in errors {
        let msg = e.message.lines().next().unwrap_or("").to_string();
        let domain = if (200..300).contains(&e.code) { "namespace" } else { "parser" };
        let kind = if e.level == 1 { "warning" } else { "error" };
        ctx.warn(&format!(
            "{who}: {}:{}: {domain} {kind} : {msg}",
            String::from_utf8_lossy(base),
            e.line
        ))?;
        // The byte offset of the reported line and column.
        let mut off = 0usize;
        let mut line = 1;
        while line < e.line && off < src.len() {
            if src[off] == b'\n' {
                line += 1;
            }
            off += 1;
        }
        let cur = off + (e.column.max(1) as usize - 1);
        let (content, caret) = context(src, cur);
        ctx.warn(&format!("{who}: {}", String::from_utf8_lossy(&content)))?;
        ctx.warn(&format!("{who}: {}", String::from_utf8_lossy(&caret)))?;
    }
    Ok(())
}

// ---- loading -----------------------------------------------------------------

fn load(o: &Object, src: Vec<u8>, base: Vec<u8>, flags: i64) {
    let s = shared_of(o);
    s.borrow_mut().data = Some(Loaded {
        src,
        base,
        flags,
        props: [false; 4],
        doc: None,
        failed: false,
        cur: Cur::Start,
    });
}

fn cwd_base(ctx: &Ctx) -> Vec<u8> {
    let mut b = ctx.cwd.to_string_lossy().into_owned().into_bytes();
    if !b.ends_with(b"/") {
        b.push(b'/');
    }
    b
}

/// A document's bytes and its base URI.
type Source = (Vec<u8>, Vec<u8>);

/// Read a URI's bytes: a local file, or a php stream for a wrapper URL.
fn read_uri(ctx: &mut Ctx, uri: &[u8]) -> Result<Option<Source>, Unwind> {
    let s = String::from_utf8_lossy(uri).into_owned();
    if s.contains("://") && !s.starts_with("file://") {
        let v = ctx.call_function(b"file_get_contents", &[Value::string(uri)])?;
        return Ok(match v {
            Value::Str(b) => Some((b.as_bytes().to_vec(), uri.to_vec())),
            _ => None,
        });
    }
    let path = s.strip_prefix("file://").unwrap_or(&s);
    let full = if std::path::Path::new(path).is_absolute() {
        std::path::PathBuf::from(path)
    } else {
        ctx.cwd.join(path)
    };
    Ok(std::fs::read(&full)
        .ok()
        .map(|b| (b, full.to_string_lossy().into_owned().into_bytes())))
}

fn flags_arg(args: &[Value]) -> i64 {
    args.get(2).map_or(0, |v| v.deref().to_int())
}

fn m_xml(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let src = s_arg(args, 0);
    if src.is_empty() {
        return Err(Unwind::value_error("XMLReader::XML(): Argument #1 ($source) must not be empty"));
    }
    let base = cwd_base(ctx);
    load(o, src, base, flags_arg(args));
    Ok(Value::Bool(true))
}

fn m_open(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let uri = s_arg(args, 0);
    if uri.is_empty() {
        return Err(Unwind::value_error("XMLReader::open(): Argument #1 ($uri) must not be empty"));
    }
    match read_uri(ctx, &uri)? {
        Some((src, base)) => {
            load(o, src, base, flags_arg(args));
            Ok(Value::Bool(true))
        }
        None => {
            ctx.warn("XMLReader::open(): Unable to open source data")?;
            Ok(Value::Bool(false))
        }
    }
}

fn new_reader(ctx: &mut Ctx) -> Result<Object, Unwind> {
    let cid = ctx.lookup_class_or_error(b"XMLReader")?;
    Ok(ctx.instantiate(cid))
}

fn m_from_string(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let src = s_arg(args, 0);
    if src.is_empty() {
        return Err(Unwind::value_error("XMLReader::fromString(): Argument #1 ($source) must not be empty"));
    }
    let o = new_reader(ctx)?;
    let base = cwd_base(ctx);
    load(&o, src, base, flags_arg(args));
    Ok(Value::Object(o))
}

fn m_from_uri(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let uri = s_arg(args, 0);
    if uri.is_empty() {
        return Err(Unwind::value_error("XMLReader::fromUri(): Argument #1 ($uri) must not be empty"));
    }
    match read_uri(ctx, &uri)? {
        Some((src, base)) => {
            let o = new_reader(ctx)?;
            load(&o, src, base, flags_arg(args));
            Ok(Value::Object(o))
        }
        None => Err(Unwind::error("Unable to open source data")),
    }
}

fn m_from_stream(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let v = args.first().map(|v| v.deref().into_owned()).unwrap_or(Value::Null);
    if !matches!(v, Value::Resource(_)) {
        return Err(Unwind::type_error(format!(
            "XMLReader::fromStream(): Argument #1 ($stream) must be of type resource, {} given",
            rphp_runtime::value_name(&v)
        )));
    }
    let data = ctx.call_function(b"stream_get_contents", &[v])?;
    let src = match data {
        Value::Str(s) => s.as_bytes().to_vec(),
        _ => Vec::new(),
    };
    let base = match args.get(3).map(|v| v.deref().into_owned()) {
        None | Some(Value::Null) => cwd_base(ctx),
        Some(u) => u.to_php_bytes(),
    };
    let o = new_reader(ctx)?;
    load(&o, src, base, flags_arg(args));
    Ok(Value::Object(o))
}

/// Parse at the first read; `false` once the document failed.
fn ensure_parsed(ctx: &mut Ctx, s: &Shared, who: &str) -> Result<bool, Unwind> {
    let (src, base, opts) = {
        let b = s.borrow();
        let Some(l) = &b.data else {
            return Err(Unwind::error("Data must be loaded before reading"));
        };
        if l.doc.is_some() {
            return Ok(!l.failed);
        }
        let opts = rphp_ext_dom::ParseOptions {
            substitute_entities: l.props[3] || l.flags & LIBXML_NOENT != 0,
            recover: false,
            preserve_white_space: true,
            no_blanks: l.flags & LIBXML_NOBLANKS != 0,
            no_cdata: l.flags & LIBXML_NOCDATA != 0,
        };
        (l.src.clone(), l.base.clone(), opts)
    };
    let doc = DocData::new();
    let outcome = rphp_ext_dom::parse(&mut doc.borrow_mut(), &src, &opts);
    {
        let mut b = s.borrow_mut();
        let l = b.data.as_mut().expect("loaded");
        l.doc = Some(doc);
        l.failed = !outcome.ok;
        if l.failed {
            l.cur = Cur::Done;
        }
    }
    report(ctx, who, outcome.errors, &src, &base)?;
    Ok(outcome.ok)
}

fn with_doc<R>(s: &Shared, f: impl FnOnce(&DocData, &mut Loaded) -> R) -> Option<R> {
    let mut b = s.borrow_mut();
    let l = b.data.as_mut()?;
    let doc = l.doc.clone()?;
    let d = doc.borrow();
    Some(f(&d, l))
}

fn m_read(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let s = shared_of(this(o)?);
    if !ensure_parsed(ctx, &s, "XMLReader::read()")? {
        return Ok(Value::Bool(false));
    }
    let r = with_doc(&s, |d, l| {
        l.cur = advance(d, l.cur);
        l.cur != Cur::Done
    });
    Ok(Value::Bool(r.unwrap_or(false)))
}

fn m_next(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let s = shared_of(this(o)?);
    if !ensure_parsed(ctx, &s, "XMLReader::next()")? {
        return Ok(Value::Bool(false));
    }
    let name = match args.first().map(|v| v.deref().into_owned()) {
        None | Some(Value::Null) => None,
        Some(v) => Some(v.to_php_bytes()),
    };
    let r = with_doc(&s, |d, l| {
        // `xmlTextReaderNext`: from an element's start, past its subtree;
        // from anywhere else, one read.
        let step = |d: &DocData, cur: Cur| -> Cur {
            match cur {
                Cur::Node(id, false) | Cur::Attr(id, _) if d.node(id).kind == NodeKind::Element => after(d, id),
                _ => advance(d, cur),
            }
        };
        l.cur = step(d, l.cur);
        while let Some(n) = &name {
            if l.cur == Cur::Done {
                break;
            }
            let local = info(d, l.cur).map(|i| i.local).unwrap_or_default();
            if &local == n {
                return true;
            }
            l.cur = step(d, l.cur);
        }
        l.cur != Cur::Done
    });
    Ok(Value::Bool(r.unwrap_or(false)))
}

fn m_close(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let s = shared_of(this(o)?);
    s.borrow_mut().data = None;
    Ok(Value::Bool(true))
}

fn current(s: &Shared) -> Option<(Doc, Cur)> {
    let b = s.borrow();
    let l = b.data.as_ref()?;
    Some((l.doc.clone()?, l.cur))
}

/// The element the cursor is on (or whose attribute it is on).
fn element_of(d: &DocData, cur: Cur) -> Option<NodeId> {
    match cur {
        Cur::Node(id, _) | Cur::Attr(id, _) if d.node(id).kind == NodeKind::Element => Some(id),
        _ => None,
    }
}

/// Find an attribute by qualified name, libxml's way: `xmlns` / `xmlns:p`
/// name the declarations, a prefix is resolved in scope.
fn find_attr(d: &DocData, el: NodeId, name: &[u8]) -> Option<usize> {
    let list = attr_list(d, el);
    let (prefix, local) = match name.iter().position(|&b| b == b':') {
        Some(i) => (Some(&name[..i]), &name[i + 1..]),
        None => (None, name),
    };
    list.iter().position(|a| match a {
        AttrRef::Ns(decl) => match prefix {
            None => name == b"xmlns" && decl.prefix.is_none(),
            Some(p) => p == b"xmlns" && decl.prefix.as_deref() == Some(local),
        },
        AttrRef::Attr(id) => {
            let an = d.node(*id);
            match prefix {
                None => an.ns.is_none() && an.name.as_slice() == name,
                Some(b"xmlns") => false,
                Some(p) => {
                    let uri = d.lookup_ns(el, Some(p));
                    uri.is_some() && an.ns == uri && an.name.as_slice() == local
                }
            }
        }
    })
}

fn find_attr_ns(d: &DocData, el: NodeId, local: &[u8], uri: &[u8]) -> Option<usize> {
    attr_list(d, el).iter().position(|a| match a {
        AttrRef::Ns(decl) => {
            uri == XMLNS_NS
                && match &decl.prefix {
                    Some(p) => p.as_slice() == local,
                    None => local == b"xmlns",
                }
        }
        AttrRef::Attr(id) => {
            let an = d.node(*id);
            an.name.as_slice() == local && an.ns.as_deref() == Some(uri)
        }
    })
}

fn attr_value(d: &DocData, el: NodeId, i: usize) -> Option<Vec<u8>> {
    match attr_list(d, el).into_iter().nth(i)? {
        AttrRef::Ns(decl) => Some(decl.uri),
        AttrRef::Attr(a) => Some(d.attr_value(a)),
    }
}

fn not_empty(v: &[u8], who: &str, pname: &str) -> Result<(), Unwind> {
    if v.is_empty() {
        return Err(Unwind::value_error(format!("{who}(): Argument #1 (${pname}) must not be empty")));
    }
    Ok(())
}

fn m_get_attribute(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let name = s_arg(args, 0);
    not_empty(&name, "XMLReader::getAttribute", "name")?;
    let Some((doc, cur)) = current(&shared_of(this(o)?)) else { return Ok(Value::Null) };
    let d = doc.borrow();
    let Some(el) = element_of(&d, cur) else { return Ok(Value::Null) };
    Ok(opt_s(find_attr(&d, el, &name).and_then(|i| attr_value(&d, el, i))))
}

fn m_get_attribute_no(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let i = args[0].deref().to_int();
    let Some((doc, cur)) = current(&shared_of(this(o)?)) else { return Ok(Value::Null) };
    let d = doc.borrow();
    let Some(el) = element_of(&d, cur) else { return Ok(Value::Null) };
    // libxml's loop over the list never runs for a negative index.
    Ok(opt_s(attr_value(&d, el, i.max(0) as usize)))
}

fn m_get_attribute_ns(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let (name, uri) = (s_arg(args, 0), s_arg(args, 1));
    if name.is_empty() {
        return Err(Unwind::value_error("XMLReader::getAttributeNs(): Argument #1 ($name) must not be empty"));
    }
    if uri.is_empty() {
        return Err(Unwind::value_error("XMLReader::getAttributeNs(): Argument #2 ($namespace) must not be empty"));
    }
    let Some((doc, cur)) = current(&shared_of(this(o)?)) else { return Ok(Value::Null) };
    let d = doc.borrow();
    let Some(el) = element_of(&d, cur) else { return Ok(Value::Null) };
    Ok(opt_s(find_attr_ns(&d, el, &name, &uri).and_then(|i| attr_value(&d, el, i))))
}

fn m_lookup_namespace(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let prefix = s_arg(args, 0);
    not_empty(&prefix, "XMLReader::lookupNamespace", "prefix")?;
    let Some((doc, cur)) = current(&shared_of(this(o)?)) else { return Ok(Value::Null) };
    let d = doc.borrow();
    let id = match cur {
        Cur::Node(id, _) | Cur::Attr(id, _) => id,
        _ => return Ok(Value::Null),
    };
    if prefix.as_slice() == b"xml" {
        return Ok(Value::string(XML_NS));
    }
    Ok(opt_s(d.lookup_ns(id, Some(&prefix))))
}

/// Move the cursor to attribute `i` of the current element.
fn move_to(s: &Shared, f: impl FnOnce(&DocData, NodeId, Cur) -> Option<usize>) -> bool {
    let Some((doc, cur)) = current(s) else { return false };
    let d = doc.borrow();
    let Some(el) = element_of(&d, cur) else { return false };
    if matches!(cur, Cur::Node(_, true)) {
        return false;
    }
    match f(&d, el, cur) {
        Some(i) if i < attr_list(&d, el).len() => {
            if let Some(l) = s.borrow_mut().data.as_mut() {
                l.cur = Cur::Attr(el, i);
            }
            true
        }
        _ => false,
    }
}

fn m_move_to_attribute(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let name = s_arg(args, 0);
    not_empty(&name, "XMLReader::moveToAttribute", "name")?;
    Ok(Value::Bool(move_to(&shared_of(this(o)?), |d, el, _| find_attr(d, el, &name))))
}

fn m_move_to_attribute_no(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let i = args[0].deref().to_int();
    Ok(Value::Bool(move_to(&shared_of(this(o)?), |_, _, _| Some(i.max(0) as usize))))
}

fn m_move_to_attribute_ns(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let (name, uri) = (s_arg(args, 0), s_arg(args, 1));
    if name.is_empty() {
        return Err(Unwind::value_error("XMLReader::moveToAttributeNs(): Argument #1 ($name) must not be empty"));
    }
    if uri.is_empty() {
        return Err(Unwind::value_error("XMLReader::moveToAttributeNs(): Argument #2 ($namespace) must not be empty"));
    }
    Ok(Value::Bool(move_to(&shared_of(this(o)?), |d, el, _| find_attr_ns(d, el, &name, &uri))))
}

fn m_move_to_first_attribute(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Ok(Value::Bool(move_to(&shared_of(this(o)?), |_, _, _| Some(0))))
}

fn m_move_to_next_attribute(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Ok(Value::Bool(move_to(&shared_of(this(o)?), |_, _, cur| match cur {
        Cur::Attr(_, i) => Some(i + 1),
        _ => Some(0),
    })))
}

fn m_move_to_element(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let s = shared_of(this(o)?);
    let mut b = s.borrow_mut();
    let Some(l) = b.data.as_mut() else { return Ok(Value::Bool(false)) };
    match l.cur {
        Cur::Attr(el, _) => {
            l.cur = Cur::Node(el, false);
            Ok(Value::Bool(true))
        }
        _ => Ok(Value::Bool(false)),
    }
}

fn m_read_string(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let Some((doc, cur)) = current(&shared_of(this(o)?)) else { return Ok(Value::string(b"")) };
    let d = doc.borrow();
    let mut out = Vec::new();
    if let Cur::Node(id, false) = cur {
        let n = d.node(id);
        match n.kind {
            NodeKind::Text => out.extend_from_slice(&n.value),
            NodeKind::Element => collect(&d, n.first_child, &mut out),
            _ => {}
        }
    }
    Ok(Value::string(&out))
}

fn m_read_inner_xml(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let Some((doc, cur)) = current(&shared_of(this(o)?)) else { return Ok(Value::string(b"")) };
    let d = doc.borrow();
    let mut out = Vec::new();
    if let Cur::Node(id, false) | Cur::Attr(id, _) = cur {
        for c in d.children(id) {
            out.extend_from_slice(&dump(&d, c, true));
        }
    }
    Ok(Value::string(&out))
}

fn m_read_outer_xml(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let Some((doc, cur)) = current(&shared_of(this(o)?)) else { return Ok(Value::string(b"")) };
    let d = doc.borrow();
    let out = match cur {
        Cur::Node(id, end) => dump(&d, id, !end),
        Cur::Attr(id, _) => dump(&d, id, true),
        _ => Vec::new(),
    };
    Ok(Value::string(&out))
}

fn m_expand(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let s = shared_of(this(o)?);
    if s.borrow().data.is_none() {
        return Err(Unwind::error("Data must be loaded before expanding"));
    }
    let node = match current(&s) {
        Some((doc, Cur::Node(id, end))) => Some((doc, id, !end)),
        Some((doc, Cur::Attr(id, _))) => Some((doc, id, true)),
        _ => None,
    };
    let Some((doc, id, deep)) = node else {
        ctx.warn("XMLReader::expand(): An Error Occurred while expanding")?;
        return Ok(Value::Bool(false));
    };
    let holder = rphp_ext_dom::orphan_doc();
    let copy = {
        let d = doc.borrow();
        let mut h = holder.borrow_mut();
        copy_into(&mut h, &d, id, deep)
    };
    let obj = rphp_ext_dom::wrap(ctx, &holder, copy)?;
    Ok(Value::Object(obj))
}

fn m_get_parser_property(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let p = args[0].deref().to_int();
    let s = shared_of(this(o)?);
    loaded_for_props(&s)?;
    if !(PROP_LOADDTD..=PROP_SUBST_ENTITIES).contains(&p) {
        return Err(Unwind::value_error(
            "XMLReader::getParserProperty(): Argument #1 ($property) must be a valid parser property",
        ));
    }
    let b = s.borrow();
    Ok(Value::Bool(b.data.as_ref().is_some_and(|l| l.props[p as usize - 1])))
}

fn m_set_parser_property(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let p = args[0].deref().to_int();
    let v = args[1].deref().to_bool();
    let s = shared_of(this(o)?);
    loaded_for_props(&s)?;
    if !(PROP_LOADDTD..=PROP_SUBST_ENTITIES).contains(&p) {
        return Err(Unwind::value_error(
            "XMLReader::setParserProperty(): Argument #1 ($property) must be a valid parser property",
        ));
    }
    let mut b = s.borrow_mut();
    match b.data.as_mut() {
        Some(l) => {
            l.props[p as usize - 1] = v;
            Ok(Value::Bool(true))
        }
        None => Ok(Value::Bool(false)),
    }
}

fn loaded_for_props(s: &Shared) -> Result<(), Unwind> {
    if s.borrow().data.is_none() {
        return Err(Unwind::error("Cannot access parser properties before loading data"));
    }
    Ok(())
}

fn m_is_valid(_: &mut Ctx, _: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Ok(Value::Bool(false))
}

// ---- properties --------------------------------------------------------------

const PROPS: &[&str] = &[
    "attributeCount",
    "baseURI",
    "depth",
    "hasAttributes",
    "hasValue",
    "isDefault",
    "isEmptyElement",
    "localName",
    "name",
    "namespaceURI",
    "nodeType",
    "prefix",
    "value",
    "xmlLang",
];

fn get_prop(_: &mut Interp, o: &Object, name: &[u8]) -> Option<Result<Value, Unwind>> {
    if !PROPS.iter().any(|p| p.as_bytes() == name) {
        return None;
    }
    let s = shared_of(o);
    // `xmlTextReaderIsEmptyElement()` fails without a current node.
    if name == b"isEmptyElement" {
        let b = s.borrow();
        if let Some(l) = &b.data {
            if matches!(l.cur, Cur::Start | Cur::Done) {
                return Some(Err(Unwind::error("Failed to read property because no XML data has been read yet")));
            }
        }
    }
    let (i, base) = match current(&s) {
        Some((doc, cur)) => {
            let d = doc.borrow();
            let base = s.borrow().data.as_ref().map(|l| l.base.clone()).unwrap_or_default();
            (info(&d, cur), base)
        }
        None => (None, Vec::new()),
    };
    let str_v = |f: fn(&Info) -> &Vec<u8>| Value::string(i.as_ref().map(f).map_or(&b""[..], |v| v.as_slice()));
    Some(Ok(match name {
        b"attributeCount" => Value::Int(i.as_ref().map_or(0, |i| i.attr_count)),
        b"baseURI" => Value::string(if i.is_some() { &base } else { b"" }),
        b"depth" => Value::Int(i.as_ref().map_or(0, |i| i.depth)),
        b"hasAttributes" => Value::Bool(i.as_ref().is_some_and(|i| i.has_attrs)),
        b"hasValue" => Value::Bool(i.as_ref().is_some_and(|i| i.has_value)),
        b"isDefault" => Value::Bool(false),
        b"isEmptyElement" => Value::Bool(i.as_ref().is_some_and(|i| i.empty)),
        b"localName" => str_v(|i| &i.local),
        b"name" => str_v(|i| &i.name),
        b"namespaceURI" => str_v(|i| &i.ns),
        b"nodeType" => Value::Int(i.as_ref().map_or(0, |i| i.node_type)),
        b"prefix" => str_v(|i| &i.prefix),
        b"value" => str_v(|i| &i.value),
        _ => str_v(|i| &i.lang),
    }))
}

fn set_prop(_: &mut Interp, _: &Object, name: &[u8], _: Value) -> Option<Result<(), Unwind>> {
    if !PROPS.iter().any(|p| p.as_bytes() == name) {
        return None;
    }
    Some(Err(Unwind::error(format!(
        "Cannot modify readonly property XMLReader::${}",
        String::from_utf8_lossy(name)
    ))))
}

fn m(min: u8, max: u8, f: rphp_runtime::NativeMethodHandler, params: &'static [&'static str]) -> NativeMethod {
    NativeMethod {
        handler: f,
        min_args: min,
        max_args: Some(max),
        params,
        by_ref: 0,
        is_static: false,
        is_final: false,
    }
}

fn sm(min: u8, max: u8, f: rphp_runtime::NativeMethodHandler, params: &'static [&'static str]) -> NativeMethod {
    NativeMethod {
        is_static: true,
        ..m(min, max, f, params)
    }
}

pub fn register(r: &mut Registry) {
    let mut b = r.class("XMLReader");
    for (name, v) in [
        ("NONE", 0),
        ("ELEMENT", 1),
        ("ATTRIBUTE", 2),
        ("TEXT", 3),
        ("CDATA", 4),
        ("ENTITY_REF", 5),
        ("ENTITY", 6),
        ("PI", 7),
        ("COMMENT", 8),
        ("DOC", 9),
        ("DOC_TYPE", 10),
        ("DOC_FRAGMENT", 11),
        ("NOTATION", 12),
        ("WHITESPACE", 13),
        ("SIGNIFICANT_WHITESPACE", 14),
        ("END_ELEMENT", 15),
        ("END_ENTITY", 16),
        ("XML_DECLARATION", 17),
        ("LOADDTD", PROP_LOADDTD),
        ("DEFAULTATTRS", PROP_DEFAULTATTRS),
        ("VALIDATE", PROP_VALIDATE),
        ("SUBST_ENTITIES", PROP_SUBST_ENTITIES),
    ] {
        b = b.class_const(name, Value::Int(v));
    }
    b.method("close", m(0, 0, m_close, &[]))
        .method("getAttribute", m(1, 1, m_get_attribute, &["name"]))
        .method("getAttributeNo", m(1, 1, m_get_attribute_no, &["index"]))
        .method("getAttributeNs", m(2, 2, m_get_attribute_ns, &["name", "namespace"]))
        .method("getParserProperty", m(1, 1, m_get_parser_property, &["property"]))
        .method("isValid", m(0, 0, m_is_valid, &[]))
        .method("lookupNamespace", m(1, 1, m_lookup_namespace, &["prefix"]))
        .method("moveToAttribute", m(1, 1, m_move_to_attribute, &["name"]))
        .method("moveToAttributeNo", m(1, 1, m_move_to_attribute_no, &["index"]))
        .method("moveToAttributeNs", m(2, 2, m_move_to_attribute_ns, &["name", "namespace"]))
        .method("moveToElement", m(0, 0, m_move_to_element, &[]))
        .method("moveToFirstAttribute", m(0, 0, m_move_to_first_attribute, &[]))
        .method("moveToNextAttribute", m(0, 0, m_move_to_next_attribute, &[]))
        .method("open", m(1, 3, m_open, &["uri", "encoding", "flags"]))
        .method("read", m(0, 0, m_read, &[]))
        .method("next", m(0, 1, m_next, &["name"]))
        .method("readInnerXml", m(0, 0, m_read_inner_xml, &[]))
        .method("readOuterXml", m(0, 0, m_read_outer_xml, &[]))
        .method("readString", m(0, 0, m_read_string, &[]))
        .method("setParserProperty", m(2, 2, m_set_parser_property, &["property", "value"]))
        .method("XML", m(1, 3, m_xml, &["source", "encoding", "flags"]))
        .method("expand", m(0, 1, m_expand, &["baseNode"]))
        .method("fromString", sm(1, 3, m_from_string, &["source", "encoding", "flags"]))
        .method("fromUri", sm(1, 3, m_from_uri, &["uri", "encoding", "flags"]))
        .method("fromStream", sm(1, 4, m_from_stream, &["stream", "encoding", "flags", "documentUri"]))
        .native_props(rphp_runtime::NativeProps {
            names: PROPS,
            get: get_prop,
            set: set_prop,
            isset: None,
            unset: None,
            list: None,
            debug: None,
            cast: None,
        })
        .finish();
}
