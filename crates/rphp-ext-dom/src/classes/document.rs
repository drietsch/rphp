//! `DOMDocument`: loading and saving, node factories, the document's
//! own properties.

use rphp_runtime::{nm, Ctx, Interp, NativeResult, Registry, Unwind};
use rphp_value::{Object, Value};

use super::{
    dom_exception, node_arg, node_ref, opt_str_arg, set_node_ref, str_arg, wrap, wrap_value,
};
use crate::libxml;
use crate::parser::{self, Options};
use crate::serialize;
use crate::tree::{
    is_valid_name, split_qname, Doc, DocData, DomError, NodeKind, NodeRef, DOCUMENT,
};

fn s(v: &[u8]) -> Value {
    Value::string(v)
}

/// The `native_init` hook: every `DOMDocument` (subclasses included)
/// starts with an empty document behind it.
fn init(_: &mut Interp, o: &Object) -> Result<(), Unwind> {
    let doc = DocData::new();
    set_node_ref(
        o,
        NodeRef {
            doc: doc.clone(),
            id: DOCUMENT,
        },
    );
    doc.borrow_mut().remember(DOCUMENT, o);
    Ok(())
}

/// A `DOMDocument` object (of `class`, default `DOMDocument`) over `doc`.
pub fn new_document_object(ctx: &mut Ctx, class: Option<u32>, doc: Doc) -> Result<Object, Unwind> {
    let cid = match class {
        Some(c) => c,
        None => ctx.lookup_class_or_error(b"DOMDocument")?,
    };
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

fn construct(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let version = opt_str_arg(args, 0).unwrap_or_else(|| b"1.0".to_vec());
    let encoding = opt_str_arg(args, 1).filter(|e| !e.is_empty());
    let mut d = r.doc.borrow_mut();
    d.version = Some(version);
    d.encoding = encoding;
    Ok(Value::Null)
}

pub fn get_prop(ctx: &mut Ctx, r: &NodeRef, name: &[u8]) -> Option<Result<Value, Unwind>> {
    let d = r.doc.borrow();
    let v: Result<Value, Unwind> = match name {
        b"doctype" => {
            let dt = d.doctype();
            drop(d);
            wrap_value(ctx, &r.doc, dt)
        }
        b"implementation" => {
            drop(d);
            super::implementation::new_implementation(ctx).map(Value::Object)
        }
        b"documentElement" => {
            let e = d.document_element();
            drop(d);
            wrap_value(ctx, &r.doc, e)
        }
        b"actualEncoding" | b"encoding" | b"xmlEncoding" => {
            Ok(d.encoding.as_deref().map_or(Value::Null, s))
        }
        b"standalone" | b"xmlStandalone" => Ok(Value::Bool(d.standalone.unwrap_or(false))),
        b"version" | b"xmlVersion" => Ok(d.version.as_deref().map_or(Value::Null, s)),
        b"strictErrorChecking" => Ok(Value::Bool(d.strict_error_checking)),
        b"documentURI" => Ok(d.document_uri.as_deref().map_or(Value::Null, s)),
        b"config" => Ok(Value::Null),
        b"formatOutput" => Ok(Value::Bool(d.format_output)),
        b"validateOnParse" => Ok(Value::Bool(d.validate_on_parse)),
        b"resolveExternals" => Ok(Value::Bool(d.resolve_externals)),
        b"preserveWhiteSpace" => Ok(Value::Bool(d.preserve_white_space)),
        b"recover" => Ok(Value::Bool(d.recover)),
        b"substituteEntities" => Ok(Value::Bool(d.substitute_entities)),
        _ => return None,
    };
    Some(v)
}

pub fn set_prop(
    it: &mut Interp,
    o: &Object,
    r: &NodeRef,
    name: &[u8],
    v: Value,
) -> Option<Result<(), Unwind>> {
    let mut d = r.doc.borrow_mut();
    let b = v.to_bool();
    match name {
        b"encoding" | b"actualEncoding" | b"xmlEncoding" => {
            let e = v.deref().to_php_bytes();
            d.encoding = if e.is_empty() { None } else { Some(e) };
        }
        b"standalone" | b"xmlStandalone" => d.standalone = Some(b),
        b"version" | b"xmlVersion" => d.version = Some(v.deref().to_php_bytes()),
        b"strictErrorChecking" => d.strict_error_checking = b,
        b"documentURI" => d.document_uri = Some(v.deref().to_php_bytes()),
        b"formatOutput" => d.format_output = b,
        b"validateOnParse" => d.validate_on_parse = b,
        b"resolveExternals" => d.resolve_externals = b,
        b"preserveWhiteSpace" => d.preserve_white_space = b,
        b"recover" => d.recover = b,
        b"substituteEntities" => d.substitute_entities = b,
        b"doctype" | b"implementation" | b"documentElement" | b"config" | b"firstElementChild"
        | b"lastElementChild" | b"childElementCount" => {
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

// ---- loading ---------------------------------------------------------------

/// Parse `src` into the document (replacing its contents), report the
/// diagnostics php's way, answer what `loadXML()` answers.
fn load_source(
    ctx: &mut Ctx,
    o: &Object,
    who: &str,
    src: &[u8],
    options: i64,
    uri: Option<Vec<u8>>,
) -> NativeResult {
    let r = node_ref(o)?;
    let (recover, preserve, substitute) = {
        let d = r.doc.borrow();
        (
            d.recover || options & 1 != 0,
            d.preserve_white_space,
            d.substitute_entities || options & libxml::LIBXML_NOENT != 0,
        )
    };
    let opts = Options {
        substitute_entities: substitute,
        recover,
        preserve_white_space: preserve,
        no_blanks: options & libxml::LIBXML_NOBLANKS != 0,
        no_cdata: options & libxml::LIBXML_NOCDATA != 0,
    };
    // A fresh arena: the old nodes' objects keep their old document.
    let fresh = DocData::new();
    let outcome = {
        let mut d = fresh.borrow_mut();
        parser::parse(&mut d, src, &opts)
    };
    let uri = uri.or_else(|| ctx.cwd_uri());
    libxml::report(ctx, who, outcome.errors, options)?;
    if !outcome.ok {
        return Ok(Value::Bool(false));
    }
    {
        let old = r.doc.borrow();
        let mut d = fresh.borrow_mut();
        d.format_output = old.format_output;
        d.preserve_white_space = old.preserve_white_space;
        d.validate_on_parse = old.validate_on_parse;
        d.resolve_externals = old.resolve_externals;
        d.strict_error_checking = old.strict_error_checking;
        d.recover = old.recover;
        d.substitute_entities = old.substitute_entities;
        d.node_classes = old.node_classes.clone();
        d.document_uri = uri;
    }
    // The document object now fronts the new arena; earlier node objects
    // stay attached to the old one, as php's do after a reload.
    r.doc.borrow_mut().objects.remove(&DOCUMENT);
    set_node_ref(
        o,
        NodeRef {
            doc: fresh.clone(),
            id: DOCUMENT,
        },
    );
    fresh.borrow_mut().remember(DOCUMENT, o);
    Ok(Value::Bool(true))
}

trait CwdUri {
    fn cwd_uri(&self) -> Option<Vec<u8>>;
}

impl CwdUri for Ctx<'_> {
    /// php's `documentURI` after `loadXML()`: the (interpreter's) working
    /// directory with a trailing slash.
    fn cwd_uri(&self) -> Option<Vec<u8>> {
        let mut s = self.cwd.to_string_lossy().into_owned().into_bytes();
        if !s.ends_with(b"/") {
            s.push(b'/');
        }
        Some(s)
    }
}

fn load_xml(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = super::this(o)?;
    let src = str_arg(args, 0);
    if src.is_empty() {
        return Err(Unwind::value_error(
            "DOMDocument::loadXML(): Argument #1 ($source) must not be empty",
        ));
    }
    let options = args.get(1).map_or(0, Value::to_int);
    load_source(ctx, o, "DOMDocument::loadXML()", &src, options, None)
}

fn load(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = super::this(o)?;
    let path = str_arg(args, 0);
    if path.is_empty() {
        return Err(Unwind::value_error(
            "DOMDocument::load(): Argument #1 ($filename) must not be empty",
        ));
    }
    let options = args.get(1).map_or(0, Value::to_int);
    let p = String::from_utf8_lossy(&path).into_owned();
    let full = if std::path::Path::new(&p).is_absolute() {
        std::path::PathBuf::from(&p)
    } else {
        ctx.cwd.join(&p)
    };
    match std::fs::read(&full) {
        Ok(src) => {
            let abs = std::fs::canonicalize(&full)
                .map(|a| a.to_string_lossy().into_owned().into_bytes())
                .unwrap_or(path);
            load_source(ctx, o, "DOMDocument::load()", &src, options, Some(abs))
        }
        Err(_) => {
            ctx.warn(&format!(
                "DOMDocument::load(): I/O warning : failed to load external entity \"{p}\""
            ))?;
            Ok(Value::Bool(false))
        }
    }
}

fn save_xml(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let node = match args.first().map(|v| v.deref().into_owned()) {
        None | Some(Value::Null) => None,
        Some(v) => match node_arg(&v) {
            Some(n) => Some(n),
            None => {
                return Err(Unwind::type_error(
                    "DOMDocument::saveXML(): Argument #1 ($node) must be of type ?DOMNode",
                ))
            }
        },
    };
    let d = r.doc.borrow();
    match node {
        Some(n) => {
            if !std::rc::Rc::ptr_eq(&n.doc, &r.doc) {
                drop(d);
                return Err(Unwind::error("Wrong Document Error"));
            }
            Ok(s(&serialize::fragment(&d, n.id)))
        }
        None => Ok(s(&serialize::document(&d))),
    }
}

fn save(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let path = String::from_utf8_lossy(&str_arg(args, 0)).into_owned();
    let out = serialize::document(&r.doc.borrow());
    match std::fs::write(&path, &out) {
        Ok(()) => Ok(Value::Int(out.len() as i64)),
        Err(_) => {
            ctx.warn(&format!(
                "DOMDocument::save(): Failed to open stream: No such file or directory"
            ))?;
            Ok(Value::Bool(false))
        }
    }
}

// ---- factories -------------------------------------------------------------

fn create_element(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let name = str_arg(args, 0);
    if !is_valid_name(&name) {
        return Err(dom_exception(ctx, DomError::InvalidCharacter));
    }
    let id = {
        let mut d = r.doc.borrow_mut();
        let (prefix, local) = split_qname(&name);
        let id = d.create_element(local, prefix, None);
        if let Some(v) = opt_str_arg(args, 1) {
            let t = d.create_text(NodeKind::Text, &v);
            d.link_last(id, t);
        }
        id
    };
    Ok(Value::Object(wrap(ctx, &r.doc, id)?))
}

/// The namespace declarations a `createElementNS`/`setAttributeNS` adds:
/// libxml declares the prefix on the node itself.
pub fn create_element_ns(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let ns = opt_str_arg(args, 0).filter(|u| !u.is_empty());
    let qname = str_arg(args, 1);
    let (prefix, local) = split_qname(&qname);
    if !is_valid_name(local) || prefix.is_some_and(|p| !is_valid_name(p)) {
        return Err(dom_exception(ctx, DomError::InvalidCharacter));
    }
    if prefix.is_some() && ns.is_none() {
        return Err(dom_exception(ctx, DomError::Namespace));
    }
    let id = {
        let mut d = r.doc.borrow_mut();
        let id = d.create_element(local, prefix, ns.as_deref());
        if let Some(uri) = &ns {
            d.node_mut(id).ns_decls.push(crate::tree::NsDecl {
                prefix: prefix.map(<[u8]>::to_vec),
                uri: uri.clone(),
            });
        }
        if let Some(v) = opt_str_arg(args, 2) {
            let t = d.create_text(NodeKind::Text, &v);
            d.link_last(id, t);
        }
        id
    };
    Ok(Value::Object(wrap(ctx, &r.doc, id)?))
}

pub fn create_text_node(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let id = r
        .doc
        .borrow_mut()
        .create_text(NodeKind::Text, &str_arg(args, 0));
    Ok(Value::Object(wrap(ctx, &r.doc, id)?))
}

pub fn create_comment(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let id = r
        .doc
        .borrow_mut()
        .create_text(NodeKind::Comment, &str_arg(args, 0));
    Ok(Value::Object(wrap(ctx, &r.doc, id)?))
}

fn create_cdata_section(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let id = r
        .doc
        .borrow_mut()
        .create_text(NodeKind::CData, &str_arg(args, 0));
    Ok(Value::Object(wrap(ctx, &r.doc, id)?))
}

pub fn create_processing_instruction(
    ctx: &mut Ctx,
    o: Option<&Object>,
    args: &mut [Value],
) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let target = str_arg(args, 0);
    if !is_valid_name(&target) {
        return Err(dom_exception(ctx, DomError::InvalidCharacter));
    }
    let id = r.doc.borrow_mut().create_pi(&target, &str_arg(args, 1));
    Ok(Value::Object(wrap(ctx, &r.doc, id)?))
}

pub fn create_attribute(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let name = str_arg(args, 0);
    if !is_valid_name(&name) {
        return Err(dom_exception(ctx, DomError::InvalidCharacter));
    }
    let id = {
        let (prefix, local) = split_qname(&name);
        r.doc.borrow_mut().create_attr(local, prefix, None, b"")
    };
    Ok(Value::Object(wrap(ctx, &r.doc, id)?))
}

pub fn create_attribute_ns(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let ns = opt_str_arg(args, 0).filter(|u| !u.is_empty());
    let qname = str_arg(args, 1);
    let (prefix, local) = split_qname(&qname);
    if !is_valid_name(local) {
        return Err(dom_exception(ctx, DomError::InvalidCharacter));
    }
    if prefix.is_some() && ns.is_none() {
        return Err(dom_exception(ctx, DomError::Namespace));
    }
    let id = r
        .doc
        .borrow_mut()
        .create_attr(local, prefix, ns.as_deref(), b"");
    Ok(Value::Object(wrap(ctx, &r.doc, id)?))
}

pub fn create_entity_reference(
    ctx: &mut Ctx,
    o: Option<&Object>,
    args: &mut [Value],
) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let name = str_arg(args, 0);
    if !is_valid_name(&name) {
        return Err(dom_exception(ctx, DomError::InvalidCharacter));
    }
    let id = r.doc.borrow_mut().create_entity_ref(&name);
    Ok(Value::Object(wrap(ctx, &r.doc, id)?))
}

pub fn create_document_fragment(
    ctx: &mut Ctx,
    o: Option<&Object>,
    _: &mut [Value],
) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let id = r.doc.borrow_mut().create_fragment();
    Ok(Value::Object(wrap(ctx, &r.doc, id)?))
}

pub fn get_elements_by_tag_name(
    ctx: &mut Ctx,
    o: Option<&Object>,
    args: &mut [Value],
) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    Ok(Value::Object(super::lists::by_tag_name(
        ctx,
        &r,
        &str_arg(args, 0),
    )?))
}

pub fn get_elements_by_tag_name_ns(
    ctx: &mut Ctx,
    o: Option<&Object>,
    args: &mut [Value],
) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let ns = opt_str_arg(args, 0).unwrap_or_default();
    Ok(Value::Object(super::lists::by_tag_name_ns(
        ctx,
        &r,
        &ns,
        &str_arg(args, 1),
    )?))
}

pub fn get_element_by_id(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let want = str_arg(args, 0);
    let found = {
        let d = r.doc.borrow();
        d.descendants(DOCUMENT).into_iter().find(|&id| {
            let n = d.node(id);
            n.kind == NodeKind::Element
                && n.attrs.iter().any(|&a| {
                    let an = d.node(a);
                    (an.is_id || (an.name == b"id" && an.prefix.as_deref() == Some(b"xml")))
                        && d.attr_value(a) == want
                })
        })
    };
    wrap_value(ctx, &r.doc, found)
}

pub fn import_node(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let Some(src) = node_arg(&args[0]) else {
        return Err(Unwind::type_error(
            "DOMDocument::importNode(): Argument #1 ($node) must be of type DOMNode",
        ));
    };
    let deep = args.get(1).is_some_and(Value::to_bool);
    let kind = src.doc.borrow().node(src.id).kind;
    if matches!(kind, NodeKind::Document | NodeKind::DocumentType) {
        return Err(dom_exception(ctx, DomError::NotSupported));
    }
    if std::rc::Rc::ptr_eq(&src.doc, &r.doc) {
        return Ok(Value::Object(wrap(ctx, &r.doc, src.id)?));
    }
    let id = {
        let s = src.doc.borrow();
        r.doc.borrow_mut().import(&s, src.id, deep)
    };
    Ok(Value::Object(wrap(ctx, &r.doc, id)?))
}

pub fn adopt_node(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let Some(src) = node_arg(&args[0]) else {
        return Err(Unwind::type_error(
            "DOMDocument::adoptNode(): Argument #1 ($node) must be of type DOMNode",
        ));
    };
    let kind = src.doc.borrow().node(src.id).kind;
    if matches!(kind, NodeKind::Document | NodeKind::DocumentType) {
        return Err(dom_exception(ctx, DomError::NotSupported));
    }
    if std::rc::Rc::ptr_eq(&src.doc, &r.doc) {
        r.doc.borrow_mut().unlink(src.id);
        return Ok(Value::Object(wrap(ctx, &r.doc, src.id)?));
    }
    let id = {
        let s = src.doc.borrow();
        r.doc.borrow_mut().import(&s, src.id, true)
    };
    if let Some(obj) = src.doc.borrow().object_of(src.id) {
        set_node_ref(
            &obj,
            NodeRef {
                doc: r.doc.clone(),
                id,
            },
        );
        r.doc.borrow_mut().remember(id, &obj);
    }
    src.doc.borrow_mut().unlink(src.id);
    Ok(Value::Object(wrap(ctx, &r.doc, id)?))
}

pub fn normalize_document(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    r.doc.borrow_mut().normalize(DOCUMENT);
    Ok(Value::Null)
}

pub fn register_node_class(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let base = str_arg(args, 0);
    let Some(base_id) = ctx.lookup_class(&base)? else {
        return Err(Unwind::type_error(format!(
            "DOMDocument::registerNodeClass(): Argument #1 ($baseClass) must be a valid class name, {} given",
            String::from_utf8_lossy(&base)
        )));
    };
    let base_l = base.to_ascii_lowercase();
    let mut d = r.doc.borrow_mut();
    d.node_classes.retain(|(b, _)| *b != base_l);
    if let Some(ext) = opt_str_arg(args, 1) {
        let Some(ext_id) = ctx.lookup_class(&ext)? else {
            return Err(Unwind::type_error(format!(
                "DOMDocument::registerNodeClass(): Argument #2 ($extendedClass) must be a valid class name or null, {} given",
                String::from_utf8_lossy(&ext)
            )));
        };
        if !ctx.is_subclass_or_eq(ext_id, base_id) {
            return Err(Unwind::type_error(format!(
                "DOMDocument::registerNodeClass(): Argument #2 ($extendedClass) must be a class that extends {}, {} given",
                String::from_utf8_lossy(&base),
                String::from_utf8_lossy(&ext)
            )));
        }
        d.node_classes.push((base_l, ext));
    }
    Ok(Value::Bool(true))
}

/// The validators: the schema languages are not implemented; the document
/// is reported valid (cataloged in `COVERAGE.md`).
pub fn validate_true(_: &mut Ctx, _: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Ok(Value::Bool(true))
}

pub fn xinclude(_: &mut Ctx, _: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Ok(Value::Int(0))
}

// ---- HTML ------------------------------------------------------------------

fn load_html(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = super::this(o)?;
    let src = str_arg(args, 0);
    if src.is_empty() {
        return Err(Unwind::value_error(
            "DOMDocument::loadHTML(): Argument #1 ($source) must not be empty",
        ));
    }
    let options = args.get(1).map_or(0, Value::to_int);
    crate::html::load(ctx, o, "DOMDocument::loadHTML()", &src, options)
}

fn load_html_file(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = super::this(o)?;
    let path = String::from_utf8_lossy(&str_arg(args, 0)).into_owned();
    let options = args.get(1).map_or(0, Value::to_int);
    match std::fs::read(&path) {
        Ok(src) => crate::html::load(ctx, o, "DOMDocument::loadHTMLFile()", &src, options),
        Err(_) => {
            ctx.warn(&format!("DOMDocument::loadHTMLFile(): I/O warning : failed to load external entity \"{path}\""))?;
            Ok(Value::Bool(false))
        }
    }
}

fn save_html(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let node = match args.first().map(|v| v.deref().into_owned()) {
        None | Some(Value::Null) => None,
        Some(v) => node_arg(&v),
    };
    let d = r.doc.borrow();
    Ok(s(&crate::html::save(&d, node.map(|n| n.id))))
}

fn save_html_file(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let path = String::from_utf8_lossy(&str_arg(args, 0)).into_owned();
    let out = crate::html::save(&r.doc.borrow(), None);
    match std::fs::write(&path, &out) {
        Ok(()) => Ok(Value::Int(out.len() as i64)),
        Err(_) => {
            ctx.warn(
                "DOMDocument::saveHTMLFile(): Failed to open stream: No such file or directory",
            )?;
            Ok(Value::Bool(false))
        }
    }
}

pub fn register(r: &mut Registry) {
    r.class("DOMDocument")
        .extends("DOMNode")
        .implements(&["DOMParentNode"])
        .native_props(super::node::DOCUMENT_PROPS)
        .native_init(init)
        .method("__construct", nm!(0, Some(2), construct))
        .method("loadXML", nm!(1, Some(2), load_xml))
        .method("load", nm!(1, Some(2), load))
        .method("saveXML", nm!(0, Some(2), save_xml))
        .method("save", nm!(1, Some(2), save))
        .method("loadHTML", nm!(1, Some(2), load_html))
        .method("loadHTMLFile", nm!(1, Some(2), load_html_file))
        .method("saveHTML", nm!(0, Some(1), save_html))
        .method("saveHTMLFile", nm!(1, Some(1), save_html_file))
        .method("createElement", nm!(1, Some(2), create_element))
        .method("createElementNS", nm!(2, Some(3), create_element_ns))
        .method("createTextNode", nm!(1, Some(1), create_text_node))
        .method("createComment", nm!(1, Some(1), create_comment))
        .method("createCDATASection", nm!(1, Some(1), create_cdata_section))
        .method(
            "createProcessingInstruction",
            nm!(1, Some(2), create_processing_instruction),
        )
        .method("createAttribute", nm!(1, Some(1), create_attribute))
        .method("createAttributeNS", nm!(2, Some(2), create_attribute_ns))
        .method(
            "createEntityReference",
            nm!(1, Some(1), create_entity_reference),
        )
        .method(
            "createDocumentFragment",
            nm!(0, Some(0), create_document_fragment),
        )
        .method(
            "getElementsByTagName",
            nm!(1, Some(1), get_elements_by_tag_name),
        )
        .method(
            "getElementsByTagNameNS",
            nm!(2, Some(2), get_elements_by_tag_name_ns),
        )
        .method("getElementById", nm!(1, Some(1), get_element_by_id))
        .method("importNode", nm!(1, Some(2), import_node))
        .method("adoptNode", nm!(1, Some(1), adopt_node))
        .method("normalizeDocument", nm!(0, Some(0), normalize_document))
        .method("registerNodeClass", nm!(2, Some(2), register_node_class))
        .method("validate", nm!(0, Some(0), validate_true))
        .method("schemaValidate", nm!(1, Some(2), validate_true))
        .method("schemaValidateSource", nm!(1, Some(2), validate_true))
        .method("relaxNGValidate", nm!(1, Some(1), validate_true))
        .method("relaxNGValidateSource", nm!(1, Some(1), validate_true))
        .method("xinclude", nm!(0, Some(1), xinclude))
        .method("append", nm!(0, None, super::element::append))
        .method("prepend", nm!(0, None, super::element::prepend))
        .method(
            "replaceChildren",
            nm!(0, None, super::element::replace_children),
        )
        .finish();
}
