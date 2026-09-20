//! php 8.4's `Dom\*` API over the same tree: `Dom\HTMLDocument` /
//! `Dom\XMLDocument` with their static constructors, the `Dom\Node` family
//! (the legacy handlers serve them where the semantics agree), the HTML
//! serializer's rules for a modern document, `Dom\TokenList` (`classList`),
//! `innerHTML`/`outerHTML`, and `Dom\Implementation`.

use rphp_runtime::{nm, Ctx, Interp, NativeProps, NativeResult, Registry, Unwind, Visibility};
use rphp_value::{Array, Object, Payload, Value};

use super::{
    dom_exception, node_arg, node_ref, opt_str_arg, set_node_ref, static_method, str_arg, this,
    wrap, wrap_value,
};
use crate::html;
use crate::libxml;
use crate::parser::{self, Options};
use crate::serialize;
use crate::tree::{split_qname, DocData, DomError, NodeId, NodeKind, NodeRef, NsDecl, DOCUMENT};

pub const XHTML_NS: &[u8] = b"http://www.w3.org/1999/xhtml";

fn s(v: &[u8]) -> Value {
    Value::string(v)
}

// ---- documents -------------------------------------------------------------

/// A `Dom\HTMLDocument`/`Dom\XMLDocument` object (of `class` or the static
/// receiver's class) over a fresh modern document.
fn new_modern_document(ctx: &mut Ctx, class: &[u8], doc: DocData) -> Result<Object, Unwind> {
    let doc = std::rc::Rc::new(std::cell::RefCell::new(doc));
    doc.borrow_mut().modern = true;
    let cid = ctx.lookup_class_or_error(class)?;
    let o = ctx.instantiate(cid);
    set_node_ref(
        &o,
        NodeRef {
            doc: doc.clone(),
            id: DOCUMENT,
        },
    );
    doc.borrow_mut().remember(DOCUMENT, &o);
    Ok(o)
}

/// Give every HTML element the XHTML namespace, as php 8.4's parser does.
fn xhtml_namespace(doc: &mut DocData) {
    for n in doc.nodes.iter_mut() {
        if n.kind == NodeKind::Element && n.ns.is_none() && n.prefix.is_none() {
            n.ns = Some(XHTML_NS.to_vec());
        }
    }
}

fn html_create_from_string(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let src = str_arg(args, 0);
    let options = args.get(1).map_or(0, Value::to_int);
    let mut doc = html::parse_document(
        ctx,
        "Dom\\HTMLDocument::createFromString()",
        &src,
        options,
        true,
    )?;
    xhtml_namespace(&mut doc);
    doc.encoding = Some(opt_str_arg(args, 2).unwrap_or_else(|| b"UTF-8".to_vec()));
    Ok(Value::Object(new_modern_document(
        ctx,
        b"Dom\\HTMLDocument",
        doc,
    )?))
}

fn html_create_from_file(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let path = String::from_utf8_lossy(&str_arg(args, 0)).into_owned();
    let options = args.get(1).map_or(0, Value::to_int);
    let full = if std::path::Path::new(&path).is_absolute() {
        std::path::PathBuf::from(&path)
    } else {
        ctx.cwd.join(&path)
    };
    let Ok(src) = std::fs::read(&full) else {
        return Err(Unwind::exception(
            "Exception",
            format!("Cannot open file '{path}'"),
        ));
    };
    let mut doc = html::parse_document(
        ctx,
        "Dom\\HTMLDocument::createFromFile()",
        &src,
        options,
        true,
    )?;
    xhtml_namespace(&mut doc);
    doc.encoding = Some(opt_str_arg(args, 2).unwrap_or_else(|| b"UTF-8".to_vec()));
    if let Ok(abs) = std::fs::canonicalize(&full) {
        doc.document_uri = Some(format!("file://{}", abs.to_string_lossy()).into_bytes());
    }
    Ok(Value::Object(new_modern_document(
        ctx,
        b"Dom\\HTMLDocument",
        doc,
    )?))
}

fn html_create_empty(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let doc = DocData::new();
    let mut doc = std::rc::Rc::try_unwrap(doc)
        .ok()
        .expect("unshared")
        .into_inner();
    doc.is_html = true;
    doc.version = None;
    doc.standalone = Some(true);
    doc.encoding = Some(opt_str_arg(args, 0).unwrap_or_else(|| b"UTF-8".to_vec()));
    Ok(Value::Object(new_modern_document(
        ctx,
        b"Dom\\HTMLDocument",
        doc,
    )?))
}

fn xml_create_from_string(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let src = str_arg(args, 0);
    let options = args.get(1).map_or(0, Value::to_int);
    let doc = DocData::new();
    let mut doc = std::rc::Rc::try_unwrap(doc)
        .ok()
        .expect("unshared")
        .into_inner();
    let outcome = parser::parse(
        &mut doc,
        &src,
        &Options {
            substitute_entities: options & libxml::LIBXML_NOENT != 0,
            recover: options & 1 != 0,
            preserve_white_space: true,
            no_blanks: options & libxml::LIBXML_NOBLANKS != 0,
            no_cdata: options & libxml::LIBXML_NOCDATA != 0,
        },
    );
    libxml::report(
        ctx,
        "Dom\\XMLDocument::createFromString()",
        outcome.errors,
        options,
    )?;
    if !outcome.ok {
        return Err(dom_exception_msg(
            ctx,
            DomError::HierarchyRequest,
            "XML fragment is not well-formed",
        ));
    }
    if doc.encoding.is_none() {
        doc.encoding = Some(b"UTF-8".to_vec());
    }
    if let Some(enc) = opt_str_arg(args, 2) {
        doc.encoding = Some(enc);
    }
    // libxml names a string-parsed document after the working directory.
    let mut cwd = ctx.cwd.to_string_lossy().into_owned();
    if !cwd.ends_with('/') {
        cwd.push('/');
    }
    doc.document_uri = Some(cwd.into_bytes());
    Ok(Value::Object(new_modern_document(
        ctx,
        b"Dom\\XMLDocument",
        doc,
    )?))
}

fn xml_create_from_file(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let path = String::from_utf8_lossy(&str_arg(args, 0)).into_owned();
    let full = if std::path::Path::new(&path).is_absolute() {
        std::path::PathBuf::from(&path)
    } else {
        ctx.cwd.join(&path)
    };
    let Ok(src) = std::fs::read(&full) else {
        return Err(Unwind::exception(
            "Exception",
            format!("Cannot open file '{path}'"),
        ));
    };
    let mut a = vec![Value::string(&src)];
    a.extend(args.iter().skip(1).cloned());
    let r = xml_create_from_string(ctx, o, &mut a)?;
    if let (Value::Object(obj), Ok(abs)) = (&r, std::fs::canonicalize(&full)) {
        if let Ok(nr) = node_ref(obj) {
            nr.doc.borrow_mut().document_uri =
                Some(format!("file://{}", abs.to_string_lossy()).into_bytes());
        }
    }
    Ok(r)
}

fn xml_create_empty(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let doc = DocData::new();
    let mut doc = std::rc::Rc::try_unwrap(doc)
        .ok()
        .expect("unshared")
        .into_inner();
    doc.version = Some(opt_str_arg(args, 0).unwrap_or_else(|| b"1.0".to_vec()));
    doc.encoding = Some(opt_str_arg(args, 1).unwrap_or_else(|| b"UTF-8".to_vec()));
    Ok(Value::Object(new_modern_document(
        ctx,
        b"Dom\\XMLDocument",
        doc,
    )?))
}

use super::dom_exception_with as dom_exception_msg;

pub fn document_prop(ctx: &mut Ctx, r: &NodeRef, name: &[u8]) -> Option<Result<Value, Unwind>> {
    let d = r.doc.borrow();
    let html = d.is_html;
    let v: Result<Value, Unwind> = match name {
        b"URL" | b"documentURI" => Ok(s(d.document_uri.as_deref().unwrap_or(b"about:blank"))),
        b"characterSet" | b"charset" | b"inputEncoding" => {
            Ok(s(d.encoding.as_deref().unwrap_or(b"UTF-8")))
        }
        b"doctype" => {
            let dt = d.doctype();
            drop(d);
            wrap_value(ctx, &r.doc, dt)
        }
        b"documentElement" => {
            let e = d.document_element();
            drop(d);
            wrap_value(ctx, &r.doc, e)
        }
        b"implementation" => {
            drop(d);
            new_implementation(ctx).map(Value::Object)
        }
        b"body" => {
            let body = d.document_element().and_then(|h| {
                d.children(h).into_iter().find(|&c| {
                    d.node(c).kind == NodeKind::Element
                        && matches!(d.node(c).name.as_slice(), b"body" | b"frameset")
                })
            });
            drop(d);
            wrap_value(ctx, &r.doc, body)
        }
        b"head" => {
            let head = d.document_element().and_then(|h| {
                d.children(h)
                    .into_iter()
                    .find(|&c| d.node(c).kind == NodeKind::Element && d.node(c).name == b"head")
            });
            drop(d);
            wrap_value(ctx, &r.doc, head)
        }
        b"title" => {
            let title = d
                .elements_by_name(DOCUMENT, b"title")
                .first()
                .map(|&t| d.text_content(t).unwrap_or_default())
                .unwrap_or_default();
            let t = String::from_utf8_lossy(&title);
            let words: Vec<&str> = t.split_whitespace().collect();
            Ok(s(words.join(" ").as_bytes()))
        }
        b"xmlEncoding" if !html => Ok(d.encoding.as_deref().map_or(Value::Null, s)),
        b"xmlStandalone" if !html => Ok(Value::Bool(d.standalone.unwrap_or(false))),
        b"xmlVersion" if !html => Ok(s(d.version.as_deref().unwrap_or(b"1.0"))),
        b"formatOutput" if !html => Ok(Value::Bool(d.format_output)),
        _ => return None,
    };
    Some(v)
}

pub fn document_set_prop(
    it: &mut Interp,
    o: &Object,
    r: &NodeRef,
    name: &[u8],
    v: Value,
) -> Option<Result<(), Unwind>> {
    let mut d = r.doc.borrow_mut();
    match name {
        b"documentURI" => d.document_uri = Some(v.deref().to_php_bytes()),
        b"xmlStandalone" => d.standalone = Some(v.to_bool()),
        b"xmlVersion" => d.version = Some(v.deref().to_php_bytes()),
        b"formatOutput" => d.format_output = v.to_bool(),
        b"title" if d.is_html => {
            let text = v.deref().to_php_bytes();
            let existing = d.elements_by_name(DOCUMENT, b"title").first().copied();
            match existing {
                Some(t) => {
                    for c in d.children(t) {
                        d.unlink(c);
                    }
                    let tn = d.create_text(NodeKind::Text, &text);
                    d.link_last(t, tn);
                }
                None => {
                    let head = d.document_element().and_then(|h| {
                        d.children(h)
                            .into_iter()
                            .find(|&c| d.node(c).name == b"head")
                    });
                    if let Some(head) = head {
                        let t = d.create_element(b"title", None, Some(XHTML_NS));
                        let tn = d.create_text(NodeKind::Text, &text);
                        d.link_last(t, tn);
                        d.link_last(head, t);
                    }
                }
            }
        }
        b"URL" | b"characterSet" | b"charset" | b"inputEncoding" | b"doctype"
        | b"documentElement" | b"implementation" | b"body" | b"head" | b"children"
        | b"firstElementChild" | b"lastElementChild" | b"childElementCount" | b"xmlEncoding" => {
            return Some(Err(Unwind::error(format!(
                "Cannot modify readonly property {}::${}",
                it.class_name_of(o),
                String::from_utf8_lossy(name)
            ))))
        }
        _ => return None,
    }
    Some(Ok(()))
}

fn save_html(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(this(o)?)?;
    let node = match args.first().map(|v| v.deref().into_owned()) {
        None | Some(Value::Null) => None,
        Some(v) => node_arg(&v).map(|n| n.id),
    };
    let d = r.doc.borrow();
    Ok(s(&html::save_modern(&d, node)))
}

fn save_html_file(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(this(o)?)?;
    let path = String::from_utf8_lossy(&str_arg(args, 0)).into_owned();
    let out = html::save_modern(&r.doc.borrow(), None);
    match std::fs::write(&path, &out) {
        Ok(()) => Ok(Value::Int(out.len() as i64)),
        Err(_) => {
            ctx.warn("Dom\\HTMLDocument::saveHtmlFile(): Failed to open stream")?;
            Ok(Value::Bool(false))
        }
    }
}

fn save_xml(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(this(o)?)?;
    let node = match args.first().map(|v| v.deref().into_owned()) {
        None | Some(Value::Null) => None,
        Some(v) => node_arg(&v).map(|n| n.id),
    };
    let d = r.doc.borrow();
    Ok(s(&match node {
        Some(n) => serialize::fragment(&d, n),
        None => serialize::document(&d),
    }))
}

fn save_xml_file(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(this(o)?)?;
    let path = String::from_utf8_lossy(&str_arg(args, 0)).into_owned();
    let out = serialize::document(&r.doc.borrow());
    match std::fs::write(&path, &out) {
        Ok(()) => Ok(Value::Int(out.len() as i64)),
        Err(_) => {
            ctx.warn("Dom\\XMLDocument::saveXmlFile(): Failed to open stream")?;
            Ok(Value::Bool(false))
        }
    }
}

/// `Dom\Document::createElement()`: an HTML document lowercases the name
/// and puts the element in the XHTML namespace.
fn create_element(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(this(o)?)?;
    let name = str_arg(args, 0);
    if !crate::tree::is_valid_name(&name) {
        return Err(dom_exception(ctx, DomError::InvalidCharacter));
    }
    let id = {
        let mut d = r.doc.borrow_mut();
        let (prefix, local) = split_qname(&name);
        if d.is_html {
            d.create_element(&local.to_ascii_lowercase(), prefix, Some(XHTML_NS))
        } else {
            d.create_element(local, prefix, None)
        }
    };
    Ok(Value::Object(wrap(ctx, &r.doc, id)?))
}

fn create_cdata_section(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(this(o)?)?;
    if r.doc.borrow().is_html {
        return Err(dom_exception(ctx, DomError::NotSupported));
    }
    let id = r
        .doc
        .borrow_mut()
        .create_text(NodeKind::CData, &str_arg(args, 0));
    Ok(Value::Object(wrap(ctx, &r.doc, id)?))
}

// ---- innerHTML / outerHTML -------------------------------------------------

pub fn inner_html(r: &NodeRef) -> Vec<u8> {
    let d = r.doc.borrow();
    let mut out = Vec::new();
    for c in d.children(r.id) {
        if d.is_html {
            out.extend_from_slice(&html::save_modern(&d, Some(c)));
        } else {
            out.extend_from_slice(&serialize::fragment(&d, c));
        }
    }
    out
}

pub fn outer_html(r: &NodeRef) -> Vec<u8> {
    let d = r.doc.borrow();
    if d.is_html {
        html::save_modern(&d, Some(r.id))
    } else {
        serialize::fragment(&d, r.id)
    }
}

/// `$el->innerHTML = '…'`: the markup parsed as the element's content.
pub fn set_inner_html(it: &mut Interp, r: &NodeRef, markup: &[u8]) -> Result<(), Unwind> {
    let mut ctx = Ctx(it);
    let is_html = r.doc.borrow().is_html;
    let fragment = if is_html {
        let mut src = b"<!DOCTYPE html><body>".to_vec();
        src.extend_from_slice(markup);
        src.extend_from_slice(b"</body>");
        let mut doc = html::parse_document(
            &mut ctx,
            "Dom\\Element::innerHTML",
            &src,
            libxml::LIBXML_NOERROR,
            true,
        )?;
        xhtml_namespace(&mut doc);
        let body = doc.document_element().and_then(|h| {
            doc.children(h)
                .into_iter()
                .find(|&c| doc.node(c).name == b"body")
        });
        (doc, body)
    } else {
        let mut src = b"<r>".to_vec();
        src.extend_from_slice(markup);
        src.extend_from_slice(b"</r>");
        let doc = DocData::new();
        let mut doc = std::rc::Rc::try_unwrap(doc)
            .ok()
            .expect("unshared")
            .into_inner();
        let outcome = parser::parse(
            &mut doc,
            &src,
            &Options {
                substitute_entities: false,
                recover: true,
                preserve_white_space: true,
                no_blanks: false,
                no_cdata: false,
            },
        );
        if outcome.errors.iter().any(|e| e.level == 3) {
            return Err(dom_exception_msg(
                &mut ctx,
                DomError::HierarchyRequest,
                "XML fragment is not well-formed",
            ));
        }
        let root = doc.document_element();
        (doc, root)
    };
    let (src_doc, container) = fragment;
    let mut d = r.doc.borrow_mut();
    for c in d.children(r.id) {
        d.unlink(c);
    }
    if let Some(container) = container {
        for c in src_doc.children(container) {
            let copy = d.import(&src_doc, c, true);
            d.link_last(r.id, copy);
        }
    }
    Ok(())
}

fn insert_adjacent_html(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(this(o)?)?;
    let pos = match args[0].deref().into_owned() {
        Value::Object(e) => e
            .get_deref(b"value")
            .map(|v| v.to_php_bytes())
            .unwrap_or_default(),
        other => other.to_php_bytes(),
    };
    let markup = str_arg(args, 1);
    // Parse into a temporary element of this document, then move.
    let tmp = r
        .doc
        .borrow_mut()
        .create_element(b"div", None, Some(XHTML_NS));
    set_inner_html(
        ctx,
        &NodeRef {
            doc: r.doc.clone(),
            id: tmp,
        },
        &markup,
    )?;
    let mut d = r.doc.borrow_mut();
    let nodes = d.children(tmp);
    match pos.to_ascii_lowercase().as_slice() {
        b"beforebegin" => {
            if let Some(p) = d.node(r.id).parent {
                for n in nodes {
                    d.link_before(p, n, r.id);
                }
            }
        }
        b"afterbegin" => {
            let first = d.node(r.id).first_child;
            for n in nodes {
                match first {
                    Some(f) => d.link_before(r.id, n, f),
                    None => d.link_last(r.id, n),
                }
            }
        }
        b"beforeend" => {
            for n in nodes {
                d.link_last(r.id, n);
            }
        }
        b"afterend" => {
            if let Some(p) = d.node(r.id).parent {
                match d.node(r.id).next {
                    Some(next) => {
                        for n in nodes {
                            d.link_before(p, n, next);
                        }
                    }
                    None => {
                        for n in nodes {
                            d.link_last(p, n);
                        }
                    }
                }
            }
        }
        _ => {
            drop(d);
            return Err(dom_exception(ctx, DomError::Namespace));
        }
    }
    Ok(Value::Null)
}

// ---- TokenList (classList) -------------------------------------------------

#[derive(Clone)]
struct TokenListState {
    element: NodeRef,
}

fn tokens(r: &NodeRef) -> Vec<Vec<u8>> {
    let d = r.doc.borrow();
    let value = d
        .find_attr(r.id, b"class")
        .map(|a| d.attr_value(a))
        .unwrap_or_default();
    let mut out: Vec<Vec<u8>> = Vec::new();
    for t in value.split(|b| b.is_ascii_whitespace()) {
        if !t.is_empty() && !out.iter().any(|x| x == t) {
            out.push(t.to_vec());
        }
    }
    out
}

fn set_tokens(r: &NodeRef, toks: &[Vec<u8>]) {
    let joined = toks.join(&b' ');
    r.doc.borrow_mut().set_attr(r.id, b"class", &joined);
}

pub fn token_list(ctx: &mut Ctx, r: &NodeRef) -> Result<Object, Unwind> {
    let cid = ctx.lookup_class_or_error(b"Dom\\TokenList")?;
    let o = ctx.instantiate(cid);
    o.set_payload(Payload::Native(Box::new(TokenListState {
        element: r.clone(),
    })));
    Ok(o)
}

fn token_state(o: &Object) -> Result<NodeRef, Unwind> {
    o.with_payload::<TokenListState, _>(|s| s.element.clone())
        .ok_or_else(|| Unwind::error("Invalid TokenList"))
}

fn token_get(_: &mut Interp, o: &Object, name: &[u8]) -> Option<Result<Value, Unwind>> {
    let r = token_state(o).ok()?;
    match name {
        b"length" => Some(Ok(Value::Int(tokens(&r).len() as i64))),
        b"value" => {
            let d = r.doc.borrow();
            Some(Ok(s(&d
                .find_attr(r.id, b"class")
                .map(|a| d.attr_value(a))
                .unwrap_or_default())))
        }
        _ => None,
    }
}

fn token_set(it: &mut Interp, o: &Object, name: &[u8], v: Value) -> Option<Result<(), Unwind>> {
    let r = token_state(o).ok()?;
    match name {
        b"value" => {
            r.doc
                .borrow_mut()
                .set_attr(r.id, b"class", &v.deref().to_php_bytes());
            Some(Ok(()))
        }
        b"length" => Some(Err(Unwind::error(format!(
            "Cannot modify readonly property {}::$length",
            it.class_name_of(o)
        )))),
        _ => None,
    }
}

fn token_item(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = token_state(this(o)?)?;
    let i = args[0].to_int();
    Ok(usize::try_from(i)
        .ok()
        .and_then(|i| tokens(&r).get(i).cloned())
        .map_or(Value::Null, |t| s(&t)))
}

fn token_contains(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = token_state(this(o)?)?;
    let want = str_arg(args, 0);
    Ok(Value::Bool(tokens(&r).iter().any(|t| *t == want)))
}

fn check_token(ctx: &mut Ctx, t: &[u8]) -> Result<(), Unwind> {
    if t.is_empty() {
        return Err(dom_exception_msg(
            ctx,
            DomError::Syntax,
            "The empty string is not a valid token",
        ));
    }
    if t.iter().any(u8::is_ascii_whitespace) {
        return Err(dom_exception_msg(
            ctx,
            DomError::InvalidCharacter,
            "The token must not contain any ASCII whitespace",
        ));
    }
    Ok(())
}

fn token_add(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = token_state(this(o)?)?;
    let mut toks = tokens(&r);
    for a in args.iter() {
        let t = a.deref().to_php_bytes();
        check_token(ctx, &t)?;
        if !toks.contains(&t) {
            toks.push(t);
        }
    }
    set_tokens(&r, &toks);
    Ok(Value::Null)
}

fn token_remove(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = token_state(this(o)?)?;
    let mut toks = tokens(&r);
    for a in args.iter() {
        let t = a.deref().to_php_bytes();
        check_token(ctx, &t)?;
        toks.retain(|x| *x != t);
    }
    set_tokens(&r, &toks);
    Ok(Value::Null)
}

fn token_toggle(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = token_state(this(o)?)?;
    let t = str_arg(args, 0);
    check_token(ctx, &t)?;
    let force = args
        .get(1)
        .map(|v| v.deref().into_owned())
        .filter(|v| !matches!(v, Value::Null))
        .map(|v| v.to_bool());
    let mut toks = tokens(&r);
    let has = toks.contains(&t);
    let result = match (has, force) {
        (true, Some(true)) => true,
        (true, _) => {
            toks.retain(|x| *x != t);
            false
        }
        (false, Some(false)) => false,
        (false, _) => {
            toks.push(t);
            true
        }
    };
    set_tokens(&r, &toks);
    Ok(Value::Bool(result))
}

fn token_replace(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = token_state(this(o)?)?;
    let (from, to) = (str_arg(args, 0), str_arg(args, 1));
    check_token(ctx, &from)?;
    check_token(ctx, &to)?;
    let mut toks = tokens(&r);
    let Some(pos) = toks.iter().position(|x| *x == from) else {
        return Ok(Value::Bool(false));
    };
    if toks.contains(&to) {
        toks.remove(pos);
    } else {
        toks[pos] = to;
    }
    set_tokens(&r, &toks);
    Ok(Value::Bool(true))
}

fn token_supports(_: &mut Ctx, _: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Ok(Value::Bool(false))
}

fn token_count(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let r = token_state(this(o)?)?;
    Ok(Value::Int(tokens(&r).len() as i64))
}

fn token_get_iterator(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let r = token_state(this(o)?)?;
    let items: Vec<(Value, Value)> = tokens(&r)
        .into_iter()
        .enumerate()
        .map(|(i, t)| (Value::Int(i as i64), s(&t)))
        .collect();
    Ok(Value::Object(rphp_stdlib::new_internal_iterator(
        ctx, items,
    )))
}

// ---- implementation --------------------------------------------------------

pub fn new_implementation(ctx: &mut Ctx) -> Result<Object, Unwind> {
    let cid = ctx.lookup_class_or_error(b"Dom\\Implementation")?;
    Ok(ctx.instantiate(cid))
}

fn impl_create_document(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let ns = opt_str_arg(args, 0).filter(|u| !u.is_empty());
    let qname = opt_str_arg(args, 1).unwrap_or_default();
    let doc = DocData::new();
    let mut doc = std::rc::Rc::try_unwrap(doc)
        .ok()
        .expect("unshared")
        .into_inner();
    doc.encoding = Some(b"UTF-8".to_vec());
    if let Some(v) = args.get(2).map(|v| v.deref().into_owned()) {
        if let Some(dt) = node_arg(&v) {
            let src = dt.doc.borrow();
            let copy = doc.import(&src, dt.id, true);
            doc.link_last(DOCUMENT, copy);
        }
    }
    if !qname.is_empty() {
        let (prefix, local) = split_qname(&qname);
        let e = doc.create_element(local, prefix, ns.as_deref());
        if let Some(uri) = &ns {
            doc.node_mut(e).ns_decls.push(NsDecl {
                prefix: prefix.map(<[u8]>::to_vec),
                uri: uri.clone(),
            });
        }
        doc.link_last(DOCUMENT, e);
    }
    Ok(Value::Object(new_modern_document(
        ctx,
        b"Dom\\XMLDocument",
        doc,
    )?))
}

fn impl_create_html_document(
    ctx: &mut Ctx,
    _: Option<&Object>,
    args: &mut [Value],
) -> NativeResult {
    let title = opt_str_arg(args, 0);
    let doc = DocData::new();
    let mut doc = std::rc::Rc::try_unwrap(doc)
        .ok()
        .expect("unshared")
        .into_inner();
    doc.is_html = true;
    doc.version = None;
    doc.standalone = Some(true);
    doc.encoding = Some(b"UTF-8".to_vec());
    let dt = doc.create_doctype(b"html", crate::tree::DocTypeInfo::default());
    doc.link_last(DOCUMENT, dt);
    let html = doc.create_element(b"html", None, Some(XHTML_NS));
    doc.link_last(DOCUMENT, html);
    let head = doc.create_element(b"head", None, Some(XHTML_NS));
    doc.link_last(html, head);
    if let Some(t) = title {
        let te = doc.create_element(b"title", None, Some(XHTML_NS));
        let tn = doc.create_text(NodeKind::Text, &t);
        doc.link_last(te, tn);
        doc.link_last(head, te);
    }
    let body = doc.create_element(b"body", None, Some(XHTML_NS));
    doc.link_last(html, body);
    Ok(Value::Object(new_modern_document(
        ctx,
        b"Dom\\HTMLDocument",
        doc,
    )?))
}

fn impl_create_document_type(
    ctx: &mut Ctx,
    _: Option<&Object>,
    args: &mut [Value],
) -> NativeResult {
    super::implementation::create_document_type_in(ctx, args, true)
}

/// `getElementsByClassName()`: the descendants carrying every listed class.
fn get_elements_by_class_name(
    ctx: &mut Ctx,
    o: Option<&Object>,
    args: &mut [Value],
) -> NativeResult {
    let r = node_ref(this(o)?)?;
    let wanted: Vec<Vec<u8>> = str_arg(args, 0)
        .split(|b| b.is_ascii_whitespace())
        .filter(|t| !t.is_empty())
        .map(<[u8]>::to_vec)
        .collect();
    let found: Vec<NodeId> = {
        let d = r.doc.borrow();
        d.descendants(r.id)
            .into_iter()
            .filter(|&id| id != r.id && d.node(id).kind == NodeKind::Element)
            .filter(|&id| {
                let classes = d
                    .find_attr(id, b"class")
                    .map(|a| d.attr_value(a))
                    .unwrap_or_default();
                let have: Vec<&[u8]> = classes.split(|b| b.is_ascii_whitespace()).collect();
                !wanted.is_empty() && wanted.iter().all(|w| have.contains(&w.as_slice()))
            })
            .collect()
    };
    Ok(Value::Object(super::lists::fixed_collection(
        ctx, &r.doc, found,
    )?))
}

// ---- namespaces (8.4 extras) -----------------------------------------------

fn get_in_scope_namespaces(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let r = node_ref(this(o)?)?;
    let mut out = Array::new();
    let mut seen: Vec<Option<Vec<u8>>> = Vec::new();
    let mut cur = Some(r.id);
    let decls: Vec<(Option<Vec<u8>>, Vec<u8>, NodeId)> = {
        let d = r.doc.borrow();
        let mut v = Vec::new();
        while let Some(n) = cur {
            if d.node(n).kind == NodeKind::Element {
                for dcl in &d.node(n).ns_decls {
                    if !seen.contains(&dcl.prefix) {
                        seen.push(dcl.prefix.clone());
                        v.push((dcl.prefix.clone(), dcl.uri.clone(), n));
                    }
                }
            }
            cur = d.node(n).parent;
        }
        v
    };
    for (prefix, uri, _declared_on) in decls {
        // php reports the element asked, not the declaring one.
        out.push(Value::Object(namespace_info(ctx, &r, prefix, uri, r.id)?));
    }
    Ok(Value::Array(out))
}

fn namespace_info(
    ctx: &mut Ctx,
    r: &NodeRef,
    prefix: Option<Vec<u8>>,
    uri: Vec<u8>,
    element: NodeId,
) -> Result<Object, Unwind> {
    let cid = ctx.lookup_class_or_error(b"Dom\\NamespaceInfo")?;
    let o = ctx.instantiate(cid);
    o.set(b"prefix", prefix.map_or(Value::Null, |p| s(&p)));
    o.set(b"namespaceURI", s(&uri));
    o.set(b"element", Value::Object(wrap(ctx, &r.doc, element)?));
    Ok(o)
}

fn rename(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(this(o)?)?;
    let ns = opt_str_arg(args, 0).filter(|u| !u.is_empty());
    let qname = str_arg(args, 1);
    let (prefix, local) = split_qname(&qname);
    if !crate::tree::is_valid_name(local) {
        return Err(dom_exception(ctx, DomError::InvalidCharacter));
    }
    let mut d = r.doc.borrow_mut();
    let n = d.node_mut(r.id);
    n.name = local.to_vec();
    n.prefix = prefix.map(<[u8]>::to_vec);
    n.ns = ns;
    Ok(Value::Null)
}

// ---- registration ----------------------------------------------------------

/// The property-name lists php's dumps show, per class.
macro_rules! props {
    ($name:ident, [$($own:literal),* $(,)?]) => {
        const $name: NativeProps = NativeProps {
            names: &[
                $($own,)*
                "nodeType",
                "nodeName",
                "baseURI",
                "isConnected",
                "ownerDocument",
                "parentNode",
                "parentElement",
                "childNodes",
                "firstChild",
                "lastChild",
                "previousSibling",
                "nextSibling",
                "nodeValue",
                "textContent",
            ],
            get: super::node::get_prop,
            set: super::node::set_prop,
            isset: None,
            unset: None,
            list: None,
            debug: Some(debug_table),
        };
    };
}

fn debug_table(it: &mut Interp, o: &Object) -> Vec<(rphp_value::ArrayKey, Value)> {
    let Some(np) = it.class_of(o).native_props else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for name in np.names {
        let v = match (np.get)(it, o, name.as_bytes()) {
            Some(Ok(Value::Object(_))) => Value::string(b"(object value omitted)"),
            Some(Ok(v)) => v,
            _ => continue,
        };
        out.push((rphp_value::ArrayKey::str(name.as_bytes()), v));
    }
    out
}

props!(NODE_PROPS, []);
props!(
    ELEMENT_PROPS,
    [
        "namespaceURI",
        "prefix",
        "localName",
        "tagName",
        "id",
        "className",
        "classList",
        "attributes",
        "children",
        "firstElementChild",
        "lastElementChild",
        "childElementCount",
        "previousElementSibling",
        "nextElementSibling",
        "innerHTML",
        "outerHTML",
        "substitutedNodeValue"
    ]
);
props!(
    HTML_ELEMENT_PROPS,
    [
        "classList",
        "namespaceURI",
        "prefix",
        "localName",
        "tagName",
        "id",
        "className",
        "attributes",
        "children",
        "firstElementChild",
        "lastElementChild",
        "childElementCount",
        "previousElementSibling",
        "nextElementSibling",
        "innerHTML",
        "outerHTML",
        "substitutedNodeValue"
    ]
);
props!(
    ATTR_PROPS,
    [
        "namespaceURI",
        "prefix",
        "localName",
        "name",
        "value",
        "ownerElement",
        "specified"
    ]
);
props!(
    CHARDATA_PROPS,
    [
        "data",
        "length",
        "previousElementSibling",
        "nextElementSibling"
    ]
);
props!(
    TEXT_PROPS,
    [
        "wholeText",
        "data",
        "length",
        "previousElementSibling",
        "nextElementSibling"
    ]
);
props!(
    PI_PROPS,
    [
        "target",
        "data",
        "length",
        "previousElementSibling",
        "nextElementSibling"
    ]
);
props!(
    DOCTYPE_PROPS,
    [
        "name",
        "entities",
        "notations",
        "publicId",
        "systemId",
        "internalSubset"
    ]
);
props!(
    FRAGMENT_PROPS,
    [
        "children",
        "firstElementChild",
        "lastElementChild",
        "childElementCount"
    ]
);
props!(
    HTML_DOCUMENT_PROPS,
    [
        "children",
        "implementation",
        "URL",
        "documentURI",
        "characterSet",
        "charset",
        "inputEncoding",
        "doctype",
        "documentElement",
        "firstElementChild",
        "lastElementChild",
        "childElementCount",
        "body",
        "head",
        "title"
    ]
);
props!(
    XML_DOCUMENT_PROPS,
    [
        "implementation",
        "xmlEncoding",
        "xmlStandalone",
        "xmlVersion",
        "formatOutput",
        "URL",
        "documentURI",
        "characterSet",
        "charset",
        "inputEncoding",
        "doctype",
        "documentElement",
        "children",
        "firstElementChild",
        "lastElementChild",
        "childElementCount",
        "body",
        "head",
        "title"
    ]
);
props!(ENTITY_PROPS, ["publicId", "systemId", "notationName"]);
props!(NOTATION_PROPS, ["publicId", "systemId"]);

pub fn register(r: &mut Registry) {
    use super::chardata as cd;
    use super::document as doc;
    use super::element as el;
    use super::node as nd;

    r.interface("Dom\\ParentNode").finish();
    r.interface("Dom\\ChildNode").finish();
    r.class("Dom\\NamespaceInfo")
        .prop("prefix", Visibility::Public, Value::Null)
        .prop("namespaceURI", Visibility::Public, Value::Null)
        .prop("element", Visibility::Public, Value::Null)
        .finish();

    r.class("Dom\\Node")
        .native_props(NODE_PROPS)
        .class_const("DOCUMENT_POSITION_DISCONNECTED", Value::Int(1))
        .class_const("DOCUMENT_POSITION_PRECEDING", Value::Int(2))
        .class_const("DOCUMENT_POSITION_FOLLOWING", Value::Int(4))
        .class_const("DOCUMENT_POSITION_CONTAINS", Value::Int(8))
        .class_const("DOCUMENT_POSITION_CONTAINED_BY", Value::Int(16))
        .class_const("DOCUMENT_POSITION_IMPLEMENTATION_SPECIFIC", Value::Int(32))
        .method("appendChild", nm!(1, Some(1), nd::append_child))
        .method("insertBefore", nm!(1, Some(2), nd::insert_before))
        .method("removeChild", nm!(1, Some(1), nd::remove_child))
        .method("replaceChild", nm!(2, Some(2), nd::replace_child))
        .method("hasChildNodes", nm!(0, Some(0), nd::has_child_nodes))
        .method("cloneNode", nm!(0, Some(1), nd::clone_node))
        .method("normalize", nm!(0, Some(0), nd::normalize))
        .method("isSameNode", nm!(1, Some(1), nd::is_same_node))
        .method("isEqualNode", nm!(1, Some(1), nd::is_equal_node))
        .method(
            "lookupNamespaceURI",
            nm!(1, Some(1), nd::lookup_namespace_uri),
        )
        .method("lookupPrefix", nm!(1, Some(1), nd::lookup_prefix))
        .method(
            "isDefaultNamespace",
            nm!(1, Some(1), nd::is_default_namespace),
        )
        .method("getLineNo", nm!(0, Some(0), nd::get_line_no))
        .method("getNodePath", nm!(0, Some(0), nd::get_node_path))
        .method("C14N", nm!(0, Some(4), nd::c14n))
        .method("contains", nm!(1, Some(1), nd::contains))
        .method("getRootNode", nm!(0, Some(1), nd::get_root_node))
        .method(
            "compareDocumentPosition",
            nm!(1, Some(1), nd::compare_document_position),
        )
        .finish();

    fn document_methods(b: rphp_runtime::ClassBuilder<'_>) -> rphp_runtime::ClassBuilder<'_> {
        b.method("createElement", nm!(1, Some(1), create_element))
            .method("createElementNS", nm!(2, Some(2), doc::create_element_ns))
            .method("createTextNode", nm!(1, Some(1), doc::create_text_node))
            .method("createComment", nm!(1, Some(1), doc::create_comment))
            .method("createCDATASection", nm!(1, Some(1), create_cdata_section))
            .method(
                "createProcessingInstruction",
                nm!(2, Some(2), doc::create_processing_instruction),
            )
            .method("createAttribute", nm!(1, Some(1), doc::create_attribute))
            .method(
                "createAttributeNS",
                nm!(2, Some(2), doc::create_attribute_ns),
            )
            .method(
                "createDocumentFragment",
                nm!(0, Some(0), doc::create_document_fragment),
            )
            .method(
                "getElementsByTagName",
                nm!(1, Some(1), doc::get_elements_by_tag_name),
            )
            .method(
                "getElementsByTagNameNS",
                nm!(2, Some(2), doc::get_elements_by_tag_name_ns),
            )
            .method(
                "getElementsByClassName",
                nm!(1, Some(1), get_elements_by_class_name),
            )
            .method("getElementById", nm!(1, Some(1), doc::get_element_by_id))
            .method("importNode", nm!(1, Some(2), doc::import_node))
            .method("importLegacyNode", nm!(1, Some(2), doc::import_node))
            .method("adoptNode", nm!(1, Some(1), doc::adopt_node))
            .method(
                "normalizeDocument",
                nm!(0, Some(0), doc::normalize_document),
            )
            .method(
                "registerNodeClass",
                nm!(2, Some(2), doc::register_node_class),
            )
            .method("append", nm!(0, None, el::append))
            .method("prepend", nm!(0, None, el::prepend))
            .method("replaceChildren", nm!(0, None, el::replace_children))
            .method(
                "querySelector",
                nm!(1, Some(1), super::selector::query_selector),
            )
            .method(
                "querySelectorAll",
                nm!(1, Some(1), super::selector::query_selector_all),
            )
            .method("schemaValidate", nm!(1, Some(2), doc::validate_true))
            .method("schemaValidateSource", nm!(1, Some(2), doc::validate_true))
            .method("relaxNgValidate", nm!(1, Some(1), doc::validate_true))
            .method("relaxNgValidateSource", nm!(1, Some(1), doc::validate_true))
            .method("saveXml", nm!(0, Some(2), save_xml))
            .method("saveXmlFile", nm!(1, Some(2), save_xml_file))
    }
    document_methods(
        r.class("Dom\\Document")
            .extends("Dom\\Node")
            .implements(&["Dom\\ParentNode"])
            .flags(rphp_runtime::ClassFlags::ABSTRACT),
    )
    .finish();
    r.class("Dom\\HTMLDocument")
        .extends("Dom\\Document")
        .flags(rphp_runtime::ClassFlags::FINAL)
        .native_props(HTML_DOCUMENT_PROPS)
        .method(
            "createFromString",
            static_method(1, Some(3), html_create_from_string),
        )
        .method(
            "createFromFile",
            static_method(1, Some(3), html_create_from_file),
        )
        .method("createEmpty", static_method(0, Some(1), html_create_empty))
        .method("saveHtml", nm!(0, Some(1), save_html))
        .method("saveHtmlFile", nm!(1, Some(1), save_html_file))
        .finish();
    r.class("Dom\\XMLDocument")
        .extends("Dom\\Document")
        .flags(rphp_runtime::ClassFlags::FINAL)
        .native_props(XML_DOCUMENT_PROPS)
        .method(
            "createFromString",
            static_method(1, Some(3), xml_create_from_string),
        )
        .method(
            "createFromFile",
            static_method(1, Some(3), xml_create_from_file),
        )
        .method("createEmpty", static_method(0, Some(2), xml_create_empty))
        .method(
            "createEntityReference",
            nm!(1, Some(1), doc::create_entity_reference),
        )
        .method("validate", nm!(0, Some(0), doc::validate_true))
        .method("xinclude", nm!(0, Some(1), doc::xinclude))
        .finish();

    r.class("Dom\\Element")
        .extends("Dom\\Node")
        .implements(&["Dom\\ParentNode", "Dom\\ChildNode"])
        .native_props(ELEMENT_PROPS)
        .method("getAttribute", nm!(1, Some(1), el::get_attribute))
        .method("getAttributeNS", nm!(2, Some(2), el::get_attribute_ns))
        .method("hasAttribute", nm!(1, Some(1), el::has_attribute))
        .method("hasAttributeNS", nm!(2, Some(2), el::has_attribute_ns))
        .method("hasAttributes", nm!(0, Some(0), nd::has_attributes))
        .method("setAttribute", nm!(2, Some(2), el::set_attribute))
        .method("setAttributeNS", nm!(3, Some(3), el::set_attribute_ns))
        .method("removeAttribute", nm!(1, Some(1), el::remove_attribute))
        .method(
            "removeAttributeNS",
            nm!(2, Some(2), el::remove_attribute_ns),
        )
        .method("getAttributeNode", nm!(1, Some(1), el::get_attribute_node))
        .method(
            "getAttributeNodeNS",
            nm!(2, Some(2), el::get_attribute_node_ns),
        )
        .method("setAttributeNode", nm!(1, Some(1), el::set_attribute_node))
        .method(
            "setAttributeNodeNS",
            nm!(1, Some(1), el::set_attribute_node),
        )
        .method(
            "removeAttributeNode",
            nm!(1, Some(1), el::remove_attribute_node),
        )
        .method(
            "getElementsByTagName",
            nm!(1, Some(1), el::get_elements_by_tag_name),
        )
        .method(
            "getElementsByTagNameNS",
            nm!(2, Some(2), el::get_elements_by_tag_name_ns),
        )
        .method(
            "getElementsByClassName",
            nm!(1, Some(1), get_elements_by_class_name),
        )
        .method("setIdAttribute", nm!(2, Some(2), el::set_id_attribute))
        .method("setIdAttributeNS", nm!(3, Some(3), el::set_id_attribute_ns))
        .method(
            "setIdAttributeNode",
            nm!(2, Some(2), el::set_id_attribute_node),
        )
        .method(
            "getAttributeNames",
            nm!(0, Some(0), el::get_attribute_names),
        )
        .method("toggleAttribute", nm!(1, Some(2), el::toggle_attribute))
        .method(
            "insertAdjacentElement",
            nm!(2, Some(2), el::insert_adjacent_element),
        )
        .method(
            "insertAdjacentText",
            nm!(2, Some(2), el::insert_adjacent_text),
        )
        .method("insertAdjacentHTML", nm!(2, Some(2), insert_adjacent_html))
        .method("append", nm!(0, None, el::append))
        .method("prepend", nm!(0, None, el::prepend))
        .method("replaceChildren", nm!(0, None, el::replace_children))
        .method("before", nm!(0, None, el::before))
        .method("after", nm!(0, None, el::after))
        .method("remove", nm!(0, Some(0), el::remove))
        .method("replaceWith", nm!(0, None, el::replace_with))
        .method(
            "querySelector",
            nm!(1, Some(1), super::selector::query_selector),
        )
        .method(
            "querySelectorAll",
            nm!(1, Some(1), super::selector::query_selector_all),
        )
        .method("closest", nm!(1, Some(1), super::selector::closest))
        .method("matches", nm!(1, Some(1), super::selector::matches))
        .method(
            "getInScopeNamespaces",
            nm!(0, Some(0), get_in_scope_namespaces),
        )
        .method(
            "getDescendantNamespaces",
            nm!(0, Some(0), get_in_scope_namespaces),
        )
        .method("rename", nm!(2, Some(2), rename))
        .finish();
    r.class("Dom\\HTMLElement")
        .extends("Dom\\Element")
        .native_props(HTML_ELEMENT_PROPS)
        .finish();
    r.class("Dom\\Attr")
        .extends("Dom\\Node")
        .native_props(ATTR_PROPS)
        .method("isId", nm!(0, Some(0), cd::attr_is_id))
        .method("rename", nm!(2, Some(2), rename))
        .finish();
    r.class("Dom\\CharacterData")
        .extends("Dom\\Node")
        .implements(&["Dom\\ChildNode"])
        .native_props(CHARDATA_PROPS)
        .method("substringData", nm!(2, Some(2), cd::substring_data))
        .method("appendData", nm!(1, Some(1), cd::append_data))
        .method("insertData", nm!(2, Some(2), cd::insert_data))
        .method("deleteData", nm!(2, Some(2), cd::delete_data))
        .method("replaceData", nm!(3, Some(3), cd::replace_data))
        .method("before", nm!(0, None, el::before))
        .method("after", nm!(0, None, el::after))
        .method("remove", nm!(0, Some(0), el::remove))
        .method("replaceWith", nm!(0, None, el::replace_with))
        .finish();
    r.class("Dom\\Text")
        .extends("Dom\\CharacterData")
        .native_props(TEXT_PROPS)
        .method("splitText", nm!(1, Some(1), cd::split_text))
        .finish();
    r.class("Dom\\CDATASection")
        .extends("Dom\\Text")
        .native_props(TEXT_PROPS)
        .finish();
    r.class("Dom\\Comment")
        .extends("Dom\\CharacterData")
        .native_props(CHARDATA_PROPS)
        .finish();
    r.class("Dom\\ProcessingInstruction")
        .extends("Dom\\CharacterData")
        .native_props(PI_PROPS)
        .finish();
    r.class("Dom\\DocumentType")
        .extends("Dom\\Node")
        .implements(&["Dom\\ChildNode"])
        .native_props(DOCTYPE_PROPS)
        .method("before", nm!(0, None, el::before))
        .method("after", nm!(0, None, el::after))
        .method("remove", nm!(0, Some(0), el::remove))
        .method("replaceWith", nm!(0, None, el::replace_with))
        .finish();
    r.class("Dom\\DocumentFragment")
        .extends("Dom\\Node")
        .implements(&["Dom\\ParentNode"])
        .native_props(FRAGMENT_PROPS)
        .method("appendXml", nm!(1, Some(1), cd::append_xml))
        .method("append", nm!(0, None, el::append))
        .method("prepend", nm!(0, None, el::prepend))
        .method("replaceChildren", nm!(0, None, el::replace_children))
        .method(
            "querySelector",
            nm!(1, Some(1), super::selector::query_selector),
        )
        .method(
            "querySelectorAll",
            nm!(1, Some(1), super::selector::query_selector_all),
        )
        .finish();
    r.class("Dom\\Entity")
        .extends("Dom\\Node")
        .native_props(ENTITY_PROPS)
        .finish();
    r.class("Dom\\EntityReference")
        .extends("Dom\\Node")
        .native_props(NODE_PROPS)
        .finish();
    r.class("Dom\\Notation")
        .extends("Dom\\Node")
        .native_props(NOTATION_PROPS)
        .finish();

    r.class("Dom\\TokenList")
        .implements(&["IteratorAggregate", "Countable"])
        .native_props(NativeProps {
            names: &["length", "value"],
            get: token_get,
            set: token_set,
            isset: None,
            unset: None,
            list: None,
            debug: None,
        })
        .method("item", nm!(1, Some(1), token_item))
        .method("contains", nm!(1, Some(1), token_contains))
        .method("add", nm!(0, None, token_add))
        .method("remove", nm!(0, None, token_remove))
        .method("toggle", nm!(1, Some(2), token_toggle))
        .method("replace", nm!(2, Some(2), token_replace))
        .method("supports", nm!(1, Some(1), token_supports))
        .method("count", nm!(0, Some(0), token_count))
        .method("getIterator", nm!(0, Some(0), token_get_iterator))
        .finish();
    r.class("Dom\\Implementation")
        .method(
            "createDocumentType",
            nm!(3, Some(3), impl_create_document_type),
        )
        .method("createDocument", nm!(2, Some(3), impl_create_document))
        .method(
            "createHTMLDocument",
            nm!(0, Some(1), impl_create_html_document),
        )
        .finish();
    super::lists::register_modern(r);
    super::xpath_class::register_modern(r);
}
