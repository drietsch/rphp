//! The `DOM*` class surface over the tree: one php object per node
//! (cached in the document, so identity holds), the computed properties
//! through the runtime's native-property hook, php's `DOMException`s.

pub mod chardata;
pub mod document;
pub mod element;
pub mod implementation;
pub mod lists;
pub mod modern;
pub mod node;
pub mod selector;
pub mod xpath_class;

use std::rc::Rc;

use rphp_runtime::{Ctx, NativeMethod, Unwind, Visibility};
use rphp_value::{Object, Payload, Value};

use crate::tree::{Doc, DocData, DomError, NodeId, NodeKind, NodeRef, DOCUMENT};

/// The receiver of an instance method.
pub fn this(o: Option<&Object>) -> Result<&Object, Unwind> {
    o.ok_or_else(|| Unwind::error("Non-static method called statically"))
}

/// The node an object wraps.
pub fn node_ref(o: &Object) -> Result<NodeRef, Unwind> {
    o.with_payload::<NodeRef, _>(|r| r.clone())
        .ok_or_else(|| Unwind::error("Couldn't fetch DOMNode"))
}

pub fn set_node_ref(o: &Object, r: NodeRef) {
    o.set_payload(Payload::Native(Box::new(r)));
}

/// The node a method argument names, if it is a DOM node.
pub fn node_arg(v: &Value) -> Option<NodeRef> {
    match &*v.deref() {
        Value::Object(o) => o.with_payload::<NodeRef, _>(|r| r.clone()),
        _ => None,
    }
}

pub fn static_method(
    min: u8,
    max: Option<u8>,
    f: rphp_runtime::NativeMethodHandler,
) -> NativeMethod {
    NativeMethod {
        handler: f,
        min_args: min,
        max_args: max,
        params: &[],
        by_ref: 0,
        is_static: true,
        is_final: false,
    }
}

/// php's class for a node kind: the legacy `DOM*` family, or php 8.4's
/// `Dom\*` for a modern document (an HTML element is a `Dom\HTMLElement`).
pub fn class_for(kind: NodeKind, modern: bool, html: bool) -> &'static str {
    if modern {
        return match kind {
            NodeKind::Element if html => "Dom\\HTMLElement",
            NodeKind::Element => "Dom\\Element",
            NodeKind::Attribute => "Dom\\Attr",
            NodeKind::Text => "Dom\\Text",
            NodeKind::CData => "Dom\\CDATASection",
            NodeKind::EntityRef => "Dom\\EntityReference",
            NodeKind::Entity => "Dom\\Entity",
            NodeKind::Pi => "Dom\\ProcessingInstruction",
            NodeKind::Comment => "Dom\\Comment",
            NodeKind::Document if html => "Dom\\HTMLDocument",
            NodeKind::Document => "Dom\\XMLDocument",
            NodeKind::DocumentType => "Dom\\DocumentType",
            NodeKind::Fragment => "Dom\\DocumentFragment",
            NodeKind::Notation => "Dom\\Notation",
        };
    }
    match kind {
        NodeKind::Element => "DOMElement",
        NodeKind::Attribute => "DOMAttr",
        NodeKind::Text => "DOMText",
        NodeKind::CData => "DOMCdataSection",
        NodeKind::EntityRef => "DOMEntityReference",
        NodeKind::Entity => "DOMEntity",
        NodeKind::Pi => "DOMProcessingInstruction",
        NodeKind::Comment => "DOMComment",
        NodeKind::Document => "DOMDocument",
        NodeKind::DocumentType => "DOMDocumentType",
        NodeKind::Fragment => "DOMDocumentFragment",
        NodeKind::Notation => "DOMNotation",
    }
}

/// The object over node `id` of `doc`: the one made earlier while it
/// lives, else a fresh instance of the node's class (or the class
/// `registerNodeClass()` mapped it to), remembered in the document.
pub fn wrap(ctx: &mut Ctx, doc: &Doc, id: NodeId) -> Result<Object, Unwind> {
    if let Some(o) = doc.borrow().object_of(id) {
        return Ok(o);
    }
    let (kind, modern, html) = {
        let d = doc.borrow();
        let n = d.node(id);
        // An HTML document's `svg`/`math` content is a plain `Dom\Element`.
        let html_ns =
            n.kind != NodeKind::Element || n.ns.as_deref().is_none_or(|ns| ns == modern::XHTML_NS);
        (n.kind, d.modern, d.is_html && html_ns)
    };
    let base = class_for(kind, modern, html);
    let mapped = doc
        .borrow()
        .node_classes
        .iter()
        .find(|(b, _)| b.as_slice() == base.to_ascii_lowercase().as_bytes())
        .map(|(_, c)| c.clone());
    let name = mapped.unwrap_or_else(|| base.as_bytes().to_vec());
    let cid = ctx.lookup_class_or_error(&name)?;
    let o = ctx.instantiate(cid);
    set_node_ref(
        &o,
        NodeRef {
            doc: doc.clone(),
            id,
        },
    );
    doc.borrow_mut().remember(id, &o);
    Ok(o)
}

pub fn wrap_value(ctx: &mut Ctx, doc: &Doc, id: Option<NodeId>) -> Result<Value, Unwind> {
    match id {
        Some(id) => Ok(Value::Object(wrap(ctx, doc, id)?)),
        None => Ok(Value::Null),
    }
}

/// `throw new DOMException(message, code)`.
pub fn dom_exception(ctx: &mut Ctx, err: DomError) -> Unwind {
    let Some(cid) = ctx.class_by_name(b"DOMException") else {
        return Unwind::error(err.message());
    };
    let o = ctx.create_throwable(cid, err.message(), err as i64, None, None);
    Unwind::Throw(o)
}

/// A `DOMException` with a message of php's own wording.
pub fn dom_exception_with(ctx: &mut Ctx, err: DomError, message: &str) -> Unwind {
    let Some(cid) = ctx.class_by_name(b"DOMException") else {
        return Unwind::error(message);
    };
    let o = ctx.create_throwable(cid, message, err as i64, None, None);
    Unwind::Throw(o)
}

/// Bring `child` into `parent`'s document: same document → as is; a node
/// from a holding document (`new DOMElement`) → moved in (its object
/// re-pointed); any other document → `WRONG_DOCUMENT_ERR`.
pub fn adopt_into(ctx: &mut Ctx, parent: &NodeRef, child: &NodeRef) -> Result<NodeId, Unwind> {
    if Rc::ptr_eq(&parent.doc, &child.doc) {
        return Ok(child.id);
    }
    if !child.doc.borrow().orphan {
        return Err(dom_exception(ctx, DomError::WrongDocument));
    }
    let new_id = {
        let src = child.doc.borrow();
        parent.doc.borrow_mut().import(&src, child.id, true)
    };
    // The php object keeps pointing at the node it names.
    if let Some(o) = child.doc.borrow().object_of(child.id) {
        set_node_ref(
            &o,
            NodeRef {
                doc: parent.doc.clone(),
                id: new_id,
            },
        );
        parent.doc.borrow_mut().remember(new_id, &o);
    }
    child.doc.borrow_mut().unlink(child.id);
    Ok(new_id)
}

/// A fresh holding document for a node made with `new DOMElement(...)`.
pub fn orphan_doc() -> Doc {
    let d = DocData::new();
    d.borrow_mut().orphan = true;
    d
}

/// The document object of `doc` (made on demand when the original was
/// dropped).
pub fn document_object(ctx: &mut Ctx, doc: &Doc) -> Result<Object, Unwind> {
    wrap(ctx, doc, DOCUMENT)
}

/// Register every DOM class, in dependency order.
pub fn register(r: &mut rphp_runtime::Registry) {
    if r.interp().class_by_name(b"DOMNode").is_some() {
        return;
    }
    r.class("DOMException")
        .extends("Exception")
        .prop("code", Visibility::Public, Value::Int(0))
        .finish();
    r.interface("DOMParentNode").finish();
    r.interface("DOMChildNode").finish();
    node::register(r);
    document::register(r);
    element::register(r);
    chardata::register(r);
    lists::register(r);
    implementation::register(r);
    xpath_class::register(r);
    modern::register(r);
    for (name, v) in [
        ("XML_ELEMENT_NODE", 1),
        ("XML_ATTRIBUTE_NODE", 2),
        ("XML_TEXT_NODE", 3),
        ("XML_CDATA_SECTION_NODE", 4),
        ("XML_ENTITY_REF_NODE", 5),
        ("XML_ENTITY_NODE", 6),
        ("XML_PI_NODE", 7),
        ("XML_COMMENT_NODE", 8),
        ("XML_DOCUMENT_NODE", 9),
        ("XML_DOCUMENT_TYPE_NODE", 10),
        ("XML_DOCUMENT_FRAG_NODE", 11),
        ("XML_NOTATION_NODE", 12),
        ("XML_HTML_DOCUMENT_NODE", 13),
        ("XML_DTD_NODE", 14),
        ("XML_ELEMENT_DECL_NODE", 15),
        ("XML_ATTRIBUTE_DECL_NODE", 16),
        ("XML_ENTITY_DECL_NODE", 17),
        ("XML_NAMESPACE_DECL_NODE", 18),
        ("XML_LOCAL_NAMESPACE", 18),
        ("XML_ATTRIBUTE_CDATA", 1),
        ("XML_ATTRIBUTE_ID", 2),
        ("XML_ATTRIBUTE_IDREF", 3),
        ("XML_ATTRIBUTE_IDREFS", 4),
        ("XML_ATTRIBUTE_ENTITY", 6),
        ("XML_ATTRIBUTE_NMTOKEN", 7),
        ("XML_ATTRIBUTE_NMTOKENS", 8),
        ("XML_ATTRIBUTE_ENUMERATION", 9),
        ("XML_ATTRIBUTE_NOTATION", 10),
        ("DOM_PHP_ERR", 0),
        ("DOM_INDEX_SIZE_ERR", 1),
        ("DOMSTRING_SIZE_ERR", 2),
        ("DOM_HIERARCHY_REQUEST_ERR", 3),
        ("DOM_WRONG_DOCUMENT_ERR", 4),
        ("DOM_INVALID_CHARACTER_ERR", 5),
        ("DOM_NO_DATA_ALLOWED_ERR", 6),
        ("DOM_NO_MODIFICATION_ALLOWED_ERR", 7),
        ("DOM_NOT_FOUND_ERR", 8),
        ("DOM_NOT_SUPPORTED_ERR", 9),
        ("DOM_INUSE_ATTRIBUTE_ERR", 10),
        ("DOM_INVALID_STATE_ERR", 11),
        ("DOM_SYNTAX_ERR", 12),
        ("DOM_INVALID_MODIFICATION_ERR", 13),
        ("DOM_NAMESPACE_ERR", 14),
        ("DOM_INVALID_ACCESS_ERR", 15),
        ("DOM_VALIDATION_ERR", 16),
    ] {
        r.constant(name, Value::Int(v));
    }
}

/// A string argument (php coerces scalars at a `string` parameter).
pub fn str_arg(args: &[Value], i: usize) -> Vec<u8> {
    args.get(i)
        .map(|v| v.deref().to_php_bytes())
        .unwrap_or_default()
}

/// An optional string argument: `None` when absent or null.
pub fn opt_str_arg(args: &[Value], i: usize) -> Option<Vec<u8>> {
    match args.get(i).map(|v| v.deref().into_owned()) {
        None | Some(Value::Null) => None,
        Some(v) => Some(v.to_php_bytes()),
    }
}
