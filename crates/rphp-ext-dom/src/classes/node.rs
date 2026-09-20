//! `DOMNode`: the properties every node answers (one getter for the whole
//! hierarchy, dispatched on the node's kind, since php's subclasses only
//! add names) and the tree methods.

use rphp_runtime::{nm, Ctx, Interp, NativeProps, NativeResult, Registry, Unwind};
use rphp_value::{Object, Value};

use super::{adopt_into, dom_exception, node_arg, node_ref, str_arg, wrap, wrap_value};
use crate::serialize;
use crate::tree::{DomError, NodeId, NodeKind, NodeRef};

/// The property names php declares, base class first (dumps list them).
pub const NODE_PROPS: &[&str] = &[
    "nodeName",
    "nodeValue",
    "nodeType",
    "parentNode",
    "parentElement",
    "childNodes",
    "firstChild",
    "lastChild",
    "previousSibling",
    "nextSibling",
    "attributes",
    "isConnected",
    "ownerDocument",
    "namespaceURI",
    "prefix",
    "localName",
    "baseURI",
    "textContent",
];

/// php's `get_debug_info` for a DOM node: every declared property, an
/// object-valued one shown as `(object value omitted)`.
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

pub const PROPS: NativeProps = NativeProps {
    names: NODE_PROPS,
    get: get_prop,
    set: set_prop,
    isset: None,
    unset: None,
    list: None,
    debug: Some(debug_table),
};

/// A subclass's `NativeProps`: its own names first, then `DOMNode`'s, in
/// the order php's dumps list them.
macro_rules! props {
    ($name:ident, [$($own:literal),* $(,)?]) => {
        pub const $name: NativeProps = NativeProps {
            names: &[
                $($own,)*
                "nodeName",
                "nodeValue",
                "nodeType",
                "parentNode",
                "parentElement",
                "childNodes",
                "firstChild",
                "lastChild",
                "previousSibling",
                "nextSibling",
                "attributes",
                "isConnected",
                "ownerDocument",
                "namespaceURI",
                "prefix",
                "localName",
                "baseURI",
                "textContent",
            ],
            get: get_prop,
            set: set_prop,
            isset: None,
            unset: None,
            list: None,
            debug: Some(debug_table),
        };
    };
}

props!(
    ELEMENT_PROPS,
    [
        "tagName",
        "className",
        "id",
        "schemaTypeInfo",
        "firstElementChild",
        "lastElementChild",
        "childElementCount",
        "previousElementSibling",
        "nextElementSibling"
    ]
);
props!(
    ATTR_PROPS,
    [
        "name",
        "specified",
        "value",
        "ownerElement",
        "schemaTypeInfo"
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
props!(PI_PROPS, ["target", "data"]);
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
    ["firstElementChild", "lastElementChild", "childElementCount"]
);
props!(
    ENTITY_PROPS,
    [
        "publicId",
        "systemId",
        "notationName",
        "actualEncoding",
        "encoding",
        "version"
    ]
);
/// `DOMNotation`'s table: no `ownerDocument` (a notation has no document
/// in libxml, and php's table skips it).
pub const NOTATION_PROPS: NativeProps = NativeProps {
    names: &[
        "publicId",
        "systemId",
        "nodeName",
        "nodeValue",
        "nodeType",
        "parentNode",
        "parentElement",
        "childNodes",
        "firstChild",
        "lastChild",
        "previousSibling",
        "nextSibling",
        "attributes",
        "isConnected",
        "namespaceURI",
        "prefix",
        "localName",
        "baseURI",
        "textContent",
    ],
    get: get_prop,
    set: set_prop,
    isset: None,
    unset: None,
    list: None,
    debug: Some(debug_table),
};
props!(
    DOCUMENT_PROPS,
    [
        "doctype",
        "implementation",
        "documentElement",
        "actualEncoding",
        "encoding",
        "xmlEncoding",
        "standalone",
        "xmlStandalone",
        "version",
        "xmlVersion",
        "strictErrorChecking",
        "documentURI",
        "config",
        "formatOutput",
        "validateOnParse",
        "resolveExternals",
        "preserveWhiteSpace",
        "recover",
        "substituteEntities",
        "firstElementChild",
        "lastElementChild",
        "childElementCount"
    ]
);

fn s(v: &[u8]) -> Value {
    Value::string(v)
}

fn opt_s(v: Option<&[u8]>) -> Value {
    v.map_or(Value::Null, Value::string)
}

/// An attribute's neighbour in its element's attribute list (libxml keeps
/// them linked, and php's `nextSibling` follows the link).
fn attr_sibling(r: &NodeRef, forward: bool) -> Option<NodeId> {
    let d = r.doc.borrow();
    let parent = d.node(r.id).parent?;
    let attrs = &d.node(parent).attrs;
    let pos = attrs.iter().position(|&a| a == r.id)?;
    if forward {
        attrs.get(pos + 1).copied()
    } else {
        pos.checked_sub(1).map(|i| attrs[i])
    }
}

/// `nodeName`: php 8.4's HTML documents read an HTML element's name in
/// uppercase (the HTML DOM's `tagName` casing).
pub fn display_name(r: &NodeRef) -> Vec<u8> {
    let d = r.doc.borrow();
    let n = d.node(r.id);
    let name = n.node_name();
    if d.modern && n.kind == NodeKind::Element && n.ns.as_deref() == Some(super::modern::XHTML_NS) {
        return name.to_ascii_uppercase();
    }
    name
}

/// The nearest element sibling in a direction.
fn element_sibling(r: &NodeRef, forward: bool) -> Option<NodeId> {
    let d = r.doc.borrow();
    let mut cur = if forward {
        d.node(r.id).next
    } else {
        d.node(r.id).prev
    };
    while let Some(c) = cur {
        if d.node(c).kind == NodeKind::Element {
            return Some(c);
        }
        cur = if forward {
            d.node(c).next
        } else {
            d.node(c).prev
        };
    }
    None
}

fn element_children(r: &NodeRef) -> Vec<NodeId> {
    let d = r.doc.borrow();
    d.children(r.id)
        .into_iter()
        .filter(|&c| d.node(c).kind == NodeKind::Element)
        .collect()
}

/// The value php's `nodeValue` reads for a kind (null for a document,
/// doctype, fragment and element in php < 8.4; php 8.5 answers an
/// element's text content for `nodeValue` too).
fn node_value(r: &NodeRef) -> Value {
    let d = r.doc.borrow();
    let n = d.node(r.id);
    match n.kind {
        NodeKind::Text | NodeKind::CData | NodeKind::Comment | NodeKind::Pi => s(&n.value),
        NodeKind::Attribute => s(&d.attr_value(r.id)),
        // The modern API answers `null` for an element (the DOM spec);
        // libxml's answers its text content.
        NodeKind::Element if d.modern => Value::Null,
        NodeKind::Entity => Value::Null,
        NodeKind::Element | NodeKind::EntityRef => {
            d.text_content(r.id).map_or(Value::Null, |v| s(&v))
        }
        NodeKind::Document | NodeKind::DocumentType | NodeKind::Fragment | NodeKind::Notation => {
            Value::Null
        }
    }
}

pub fn get_prop(it: &mut Interp, o: &Object, name: &[u8]) -> Option<Result<Value, Unwind>> {
    let r = node_ref(o).ok()?;
    let kind = r.doc.borrow().node(r.id).kind;
    let mut ctx = Ctx(it);
    let ctx = &mut ctx;
    let v: Result<Value, Unwind> = match name {
        b"nodeName" => Ok(s(&display_name(&r))),
        b"nodeValue" => Ok(node_value(&r)),
        b"nodeType" => {
            let d = r.doc.borrow();
            Ok(Value::Int(match kind {
                NodeKind::Document if d.modern && d.is_html => 13,
                // php's entity nodes are the declarations (`XML_ENTITY_DECL_NODE`).
                NodeKind::Entity => 17,
                _ => kind as i64,
            }))
        }
        b"parentNode" => {
            // libxml links an attribute to its element as its parent.
            let p = r.doc.borrow().node(r.id).parent;
            wrap_value(ctx, &r.doc, p)
        }
        b"parentElement" => {
            let d = r.doc.borrow();
            let p = d
                .node(r.id)
                .parent
                .filter(|&p| d.node(p).kind == NodeKind::Element);
            drop(d);
            wrap_value(ctx, &r.doc, p)
        }
        b"childNodes" => super::lists::children_list(ctx, &r).map(Value::Object),
        b"firstChild" => {
            let c = r.doc.borrow().node(r.id).first_child;
            wrap_value(ctx, &r.doc, c)
        }
        b"lastChild" => {
            let c = r.doc.borrow().node(r.id).last_child;
            wrap_value(ctx, &r.doc, c)
        }
        b"previousSibling" => {
            let c = if kind == NodeKind::Attribute {
                attr_sibling(&r, false)
            } else {
                r.doc.borrow().node(r.id).prev
            };
            wrap_value(ctx, &r.doc, c)
        }
        b"nextSibling" => {
            let c = if kind == NodeKind::Attribute {
                attr_sibling(&r, true)
            } else {
                r.doc.borrow().node(r.id).next
            };
            wrap_value(ctx, &r.doc, c)
        }
        b"attributes" => {
            if kind == NodeKind::Element {
                super::lists::attr_map(ctx, &r).map(Value::Object)
            } else {
                Ok(Value::Null)
            }
        }
        b"isConnected" => Ok(Value::Bool(
            !r.doc.borrow().orphan && r.doc.borrow().is_connected(r.id),
        )),
        b"ownerDocument" => {
            if kind == NodeKind::Document || r.doc.borrow().orphan {
                Ok(Value::Null)
            } else {
                super::document_object(ctx, &r.doc).map(Value::Object)
            }
        }
        b"namespaceURI" => Ok(opt_s(r.doc.borrow().node(r.id).ns.as_deref())),
        b"prefix" => {
            let d = r.doc.borrow();
            // The modern API answers `null` for no prefix; libxml's `""`.
            Ok(match d.node(r.id).prefix.as_deref() {
                Some(p) => s(p),
                None if d.modern => Value::Null,
                None => s(b""),
            })
        }
        b"localName" => Ok(match kind {
            NodeKind::Element | NodeKind::Attribute => s(&r.doc.borrow().node(r.id).name),
            _ => Value::Null,
        }),
        b"baseURI"
            if matches!(kind, NodeKind::Entity | NodeKind::Notation) && !r.doc.borrow().modern =>
        {
            Ok(Value::Null)
        }
        b"baseURI" => {
            let d = r.doc.borrow();
            Ok(match (&d.document_uri, d.modern) {
                (Some(uri), _) => s(uri),
                (None, true) => s(b"about:blank"),
                (None, false) => Value::Null,
            })
        }
        b"textContent" if kind == NodeKind::Document && r.doc.borrow().modern => Ok(Value::Null),
        // An entity declaration's text: php's legacy node answers "", the
        // modern one null; a notation "" / null likewise.
        b"textContent" if matches!(kind, NodeKind::Entity | NodeKind::Notation) => {
            Ok(if r.doc.borrow().modern {
                Value::Null
            } else {
                s(b"")
            })
        }
        b"textContent" => Ok(r
            .doc
            .borrow()
            .text_content(r.id)
            .map_or(Value::Null, |v| s(&v))),
        // ---- element ----
        b"tagName" if kind == NodeKind::Element => Ok(s(&display_name(&r))),
        b"id" if kind == NodeKind::Element => {
            let d = r.doc.borrow();
            Ok(s(&d
                .find_attr(r.id, b"id")
                .map(|a| d.attr_value(a))
                .unwrap_or_default()))
        }
        b"className" if kind == NodeKind::Element => {
            let d = r.doc.borrow();
            Ok(s(&d
                .find_attr(r.id, b"class")
                .map(|a| d.attr_value(a))
                .unwrap_or_default()))
        }
        b"schemaTypeInfo" if matches!(kind, NodeKind::Element | NodeKind::Attribute) => {
            Ok(Value::Null)
        }
        b"firstElementChild"
            if matches!(
                kind,
                NodeKind::Element | NodeKind::Document | NodeKind::Fragment
            ) =>
        {
            let c = element_children(&r).first().copied();
            wrap_value(ctx, &r.doc, c)
        }
        b"lastElementChild"
            if matches!(
                kind,
                NodeKind::Element | NodeKind::Document | NodeKind::Fragment
            ) =>
        {
            let c = element_children(&r).last().copied();
            wrap_value(ctx, &r.doc, c)
        }
        b"childElementCount"
            if matches!(
                kind,
                NodeKind::Element | NodeKind::Document | NodeKind::Fragment
            ) =>
        {
            Ok(Value::Int(element_children(&r).len() as i64))
        }
        b"previousElementSibling"
            if matches!(
                kind,
                NodeKind::Element
                    | NodeKind::Text
                    | NodeKind::CData
                    | NodeKind::Comment
                    | NodeKind::Pi
            ) =>
        {
            let c = element_sibling(&r, false);
            wrap_value(ctx, &r.doc, c)
        }
        b"nextElementSibling"
            if matches!(
                kind,
                NodeKind::Element
                    | NodeKind::Text
                    | NodeKind::CData
                    | NodeKind::Comment
                    | NodeKind::Pi
            ) =>
        {
            let c = element_sibling(&r, true);
            wrap_value(ctx, &r.doc, c)
        }
        // ---- attribute ----
        b"name" if kind == NodeKind::Attribute => {
            Ok(s(&r.doc.borrow().node(r.id).qualified_name()))
        }
        b"value" if kind == NodeKind::Attribute => Ok(s(&r.doc.borrow().attr_value(r.id))),
        b"specified" if kind == NodeKind::Attribute => Ok(Value::Bool(true)),
        b"ownerElement" if kind == NodeKind::Attribute => {
            let p = r.doc.borrow().node(r.id).parent;
            wrap_value(ctx, &r.doc, p)
        }
        // ---- character data / pi ----
        b"data"
            if matches!(
                kind,
                NodeKind::Text | NodeKind::CData | NodeKind::Comment | NodeKind::Pi
            ) =>
        {
            Ok(s(&r.doc.borrow().node(r.id).value))
        }
        b"length"
            if matches!(kind, NodeKind::Text | NodeKind::CData | NodeKind::Comment)
                || (kind == NodeKind::Pi && r.doc.borrow().modern) =>
        {
            let v = r.doc.borrow().node(r.id).value.clone();
            Ok(Value::Int(
                String::from_utf8_lossy(&v).chars().count() as i64
            ))
        }
        b"wholeText" if matches!(kind, NodeKind::Text | NodeKind::CData) => Ok(s(&whole_text(&r))),
        b"target" if kind == NodeKind::Pi => Ok(s(&r.doc.borrow().node(r.id).name)),
        // ---- doctype ----
        b"name" if kind == NodeKind::DocumentType => Ok(s(&r.doc.borrow().node(r.id).name)),
        b"publicId" if kind == NodeKind::DocumentType => Ok(s(&r
            .doc
            .borrow()
            .node(r.id)
            .doctype
            .as_ref()
            .map(|i| i.public_id.clone())
            .unwrap_or_default())),
        b"systemId" if kind == NodeKind::DocumentType => Ok(s(&r
            .doc
            .borrow()
            .node(r.id)
            .doctype
            .as_ref()
            .map(|i| i.system_id.clone())
            .unwrap_or_default())),
        b"internalSubset" if kind == NodeKind::DocumentType => {
            let sub = r
                .doc
                .borrow()
                .node(r.id)
                .doctype
                .as_ref()
                .and_then(|i| i.internal_subset.clone());
            Ok(match sub {
                Some(mut v) => {
                    if !v.ends_with(b"\n") {
                        v.push(b'\n');
                    }
                    s(&v)
                }
                None => Value::Null,
            })
        }
        b"entities" | b"notations" if kind == NodeKind::DocumentType => {
            super::lists::dtd_map(ctx, &r, name == b"entities").map(Value::Object)
        }
        // An entity declaration: ids only when external (else null); a
        // notation always has both (empty when absent).
        // php answers an entity's ids only for an unparsed (NDATA) one.
        b"publicId" | b"systemId" if kind == NodeKind::Entity => {
            let d = r.doc.borrow();
            let decl = d
                .node(r.id)
                .entity
                .as_deref()
                .filter(|e| e.notation.is_some());
            let v = if name == b"publicId" {
                decl.and_then(|e| e.public_id.clone())
            } else {
                decl.and_then(|e| e.system_id.clone())
            };
            Ok(v.map_or(Value::Null, |v| s(&v)))
        }
        b"publicId" | b"systemId" if kind == NodeKind::Notation => {
            let d = r.doc.borrow();
            let info = d.node(r.id).doctype.as_deref();
            let v = if name == b"publicId" {
                info.map(|i| i.public_id.clone())
            } else {
                info.map(|i| i.system_id.clone())
            };
            Ok(s(&v.unwrap_or_default()))
        }
        b"notationName" if kind == NodeKind::Entity => Ok(r
            .doc
            .borrow()
            .node(r.id)
            .entity
            .as_deref()
            .and_then(|e| e.notation.clone())
            .map_or(Value::Null, |v| s(&v))),
        b"actualEncoding" | b"encoding" | b"version" if kind == NodeKind::Entity => Ok(Value::Null),
        b"notationName" | b"actualEncoding" | b"encoding" | b"version"
            if kind == NodeKind::Notation =>
        {
            Ok(Value::Null)
        }
        // ---- php 8.4 extras ----
        b"children"
            if matches!(
                kind,
                NodeKind::Element | NodeKind::Document | NodeKind::Fragment
            ) && r.doc.borrow().modern =>
        {
            super::lists::children_collection(ctx, &r).map(Value::Object)
        }
        b"classList" if kind == NodeKind::Element && r.doc.borrow().modern => {
            super::modern::token_list(ctx, &r).map(Value::Object)
        }
        b"innerHTML" if kind == NodeKind::Element && r.doc.borrow().modern => {
            Ok(s(&super::modern::inner_html(&r)))
        }
        b"outerHTML" if kind == NodeKind::Element && r.doc.borrow().modern => {
            Ok(s(&super::modern::outer_html(&r)))
        }
        b"substitutedNodeValue" if kind == NodeKind::Element && r.doc.borrow().modern => Ok(r
            .doc
            .borrow()
            .text_content(r.id)
            .map_or(Value::Null, |v| s(&v))),
        // ---- document ----
        _ if kind == NodeKind::Document && r.doc.borrow().modern => {
            return super::modern::document_prop(ctx, &r, name)
        }
        _ if kind == NodeKind::Document => return super::document::get_prop(ctx, &r, name),
        _ => return None,
    };
    Some(v)
}

/// A text node's `wholeText`: it and its adjacent text siblings.
fn whole_text(r: &NodeRef) -> Vec<u8> {
    let d = r.doc.borrow();
    let mut first = r.id;
    while let Some(p) = d.node(first).prev {
        if matches!(d.node(p).kind, NodeKind::Text | NodeKind::CData) {
            first = p;
        } else {
            break;
        }
    }
    let mut out = Vec::new();
    let mut cur = Some(first);
    while let Some(c) = cur {
        if !matches!(d.node(c).kind, NodeKind::Text | NodeKind::CData) {
            break;
        }
        out.extend_from_slice(&d.node(c).value);
        cur = d.node(c).next;
    }
    out
}

fn readonly(it: &Interp, o: &Object, name: &[u8]) -> Unwind {
    Unwind::error(format!(
        "Cannot modify readonly property {}::${}",
        it.class_name_of(o),
        String::from_utf8_lossy(name)
    ))
}

/// Replace a node's children with one text node (`nodeValue` /
/// `textContent` writes on an element or attribute).
fn set_text_children(r: &NodeRef, v: &[u8]) {
    let mut d = r.doc.borrow_mut();
    for c in d.children(r.id) {
        d.unlink(c);
    }
    if !v.is_empty() {
        let t = d.create_text(NodeKind::Text, v);
        d.link_last(r.id, t);
    }
}

pub fn set_prop(it: &mut Interp, o: &Object, name: &[u8], v: Value) -> Option<Result<(), Unwind>> {
    let r = node_ref(o).ok()?;
    let kind = r.doc.borrow().node(r.id).kind;
    let text = || v.deref().to_php_bytes();
    let res: Result<(), Unwind> = match name {
        b"nodeValue" | b"textContent" => {
            match kind {
                NodeKind::Text | NodeKind::CData | NodeKind::Comment | NodeKind::Pi => {
                    r.doc.borrow_mut().node_mut(r.id).value = text();
                }
                NodeKind::Element
                | NodeKind::Attribute
                | NodeKind::Fragment
                | NodeKind::EntityRef => set_text_children(&r, &text()),
                _ => {}
            }
            Ok(())
        }
        b"prefix" if kind == NodeKind::Element => {
            let p = text();
            r.doc.borrow_mut().node_mut(r.id).prefix = if p.is_empty() { None } else { Some(p) };
            Ok(())
        }
        b"value" if kind == NodeKind::Attribute => {
            r.doc.borrow_mut().set_attr_value(r.id, &text());
            Ok(())
        }
        b"data"
            if matches!(
                kind,
                NodeKind::Text | NodeKind::CData | NodeKind::Comment | NodeKind::Pi
            ) =>
        {
            r.doc.borrow_mut().node_mut(r.id).value = text();
            Ok(())
        }
        b"id" if kind == NodeKind::Element => {
            r.doc.borrow_mut().set_attr(r.id, b"id", &text());
            Ok(())
        }
        b"className" if kind == NodeKind::Element => {
            r.doc.borrow_mut().set_attr(r.id, b"class", &text());
            Ok(())
        }
        b"innerHTML" if kind == NodeKind::Element && r.doc.borrow().modern => {
            super::modern::set_inner_html(it, &r, &text())
        }
        _ if kind == NodeKind::Document && r.doc.borrow().modern => {
            return super::modern::document_set_prop(it, o, &r, name, v)
        }
        _ if kind == NodeKind::Document => return super::document::set_prop(it, o, &r, name, v),
        _ if NODE_PROPS.contains(&std::str::from_utf8(name).unwrap_or("")) => {
            Err(readonly(it, o, name))
        }
        b"tagName"
        | b"schemaTypeInfo"
        | b"firstElementChild"
        | b"lastElementChild"
        | b"childElementCount"
        | b"previousElementSibling"
        | b"nextElementSibling"
            if kind == NodeKind::Element =>
        {
            Err(readonly(it, o, name))
        }
        b"name" | b"specified" | b"ownerElement" | b"schemaTypeInfo"
            if kind == NodeKind::Attribute =>
        {
            Err(readonly(it, o, name))
        }
        b"length" | b"wholeText" | b"previousElementSibling" | b"nextElementSibling"
            if matches!(
                kind,
                NodeKind::Text | NodeKind::CData | NodeKind::Comment | NodeKind::Pi
            ) =>
        {
            Err(readonly(it, o, name))
        }
        b"target" if kind == NodeKind::Pi => Err(readonly(it, o, name)),
        b"name" | b"entities" | b"notations" | b"publicId" | b"systemId" | b"internalSubset"
            if kind == NodeKind::DocumentType =>
        {
            Err(readonly(it, o, name))
        }
        _ => return None,
    };
    Some(res)
}

// ---- methods ---------------------------------------------------------------

pub fn append_child(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let Some(child) = node_arg(&args[0]) else {
        return Err(Unwind::type_error(
            "DOMNode::appendChild(): Argument #1 ($node) must be of type DOMNode",
        ));
    };
    if child.same(&r) {
        return Ok(Value::Object(wrap(ctx, &r.doc, r.id)?));
    }
    let cid = adopt_into(ctx, &r, &child)?;
    let res = r.doc.borrow_mut().append_child(r.id, cid);
    match res {
        Ok(id) => Ok(Value::Object(wrap(ctx, &r.doc, id)?)),
        Err(e) => Err(dom_exception(ctx, e)),
    }
}

pub fn insert_before(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let Some(child) = node_arg(&args[0]) else {
        return Err(Unwind::type_error(
            "DOMNode::insertBefore(): Argument #1 ($node) must be of type DOMNode",
        ));
    };
    let before = match args.get(1).map(|v| v.deref().into_owned()) {
        None | Some(Value::Null) => None,
        Some(v) => match node_arg(&v) {
            Some(b) => Some(b),
            None => {
                return Err(Unwind::type_error(
                    "DOMNode::insertBefore(): Argument #2 ($child) must be of type ?DOMNode",
                ))
            }
        },
    };
    if let Some(b) = &before {
        if !std::rc::Rc::ptr_eq(&b.doc, &r.doc) || r.doc.borrow().node(b.id).parent != Some(r.id) {
            return Err(dom_exception(ctx, DomError::NotFound));
        }
    }
    let cid = adopt_into(ctx, &r, &child)?;
    let res = r
        .doc
        .borrow_mut()
        .insert_before(r.id, cid, before.map(|b| b.id));
    match res {
        Ok(id) => Ok(Value::Object(wrap(ctx, &r.doc, id)?)),
        Err(e) => Err(dom_exception(ctx, e)),
    }
}

pub fn remove_child(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let Some(child) = node_arg(&args[0]) else {
        return Err(Unwind::type_error(
            "DOMNode::removeChild(): Argument #1 ($child) must be of type DOMNode",
        ));
    };
    if !std::rc::Rc::ptr_eq(&child.doc, &r.doc) {
        return Err(dom_exception(ctx, DomError::NotFound));
    }
    let res = r.doc.borrow_mut().remove_child(r.id, child.id);
    match res {
        Ok(id) => Ok(Value::Object(wrap(ctx, &r.doc, id)?)),
        Err(e) => Err(dom_exception(ctx, e)),
    }
}

pub fn replace_child(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let (Some(new), Some(old)) = (node_arg(&args[0]), node_arg(&args[1])) else {
        return Err(Unwind::type_error(
            "DOMNode::replaceChild(): Argument #1 ($node) must be of type DOMNode",
        ));
    };
    if !std::rc::Rc::ptr_eq(&old.doc, &r.doc) {
        return Err(dom_exception(ctx, DomError::NotFound));
    }
    let nid = adopt_into(ctx, &r, &new)?;
    let res = r.doc.borrow_mut().replace_child(r.id, nid, old.id);
    match res {
        Ok(id) => Ok(Value::Object(wrap(ctx, &r.doc, id)?)),
        Err(e) => Err(dom_exception(ctx, e)),
    }
}

pub fn has_child_nodes(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let has = r.doc.borrow().node(r.id).first_child.is_some();
    Ok(Value::Bool(has))
}

pub fn has_attributes(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let d = r.doc.borrow();
    Ok(Value::Bool(
        d.node(r.id).kind == NodeKind::Element && !d.node(r.id).attrs.is_empty(),
    ))
}

pub fn clone_node(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let deep = args.first().is_some_and(Value::to_bool);
    let kind = r.doc.borrow().node(r.id).kind;
    if kind == NodeKind::Document {
        // A document clone is a new document.
        let new = crate::tree::DocData::new();
        {
            let src = r.doc.borrow();
            let mut dst = new.borrow_mut();
            dst.version = src.version.clone();
            dst.encoding = src.encoding.clone();
            dst.standalone = src.standalone;
            dst.document_uri = src.document_uri.clone();
            if deep {
                for c in src.children(crate::tree::DOCUMENT) {
                    let copy = dst.import(&src, c, true);
                    dst.link_last(crate::tree::DOCUMENT, copy);
                }
            }
        }
        return Ok(Value::Object(super::document::new_document_object(
            ctx,
            o.map(|o| ctx.class_of(o).id),
            new,
        )?));
    }
    let id = r.doc.borrow_mut().clone_node(r.id, deep);
    Ok(Value::Object(wrap(ctx, &r.doc, id)?))
}

pub fn normalize(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    r.doc.borrow_mut().normalize(r.id);
    Ok(Value::Null)
}

pub fn is_same_node(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    Ok(Value::Bool(
        node_arg(&args[0]).is_some_and(|other| other.same(&r)),
    ))
}

pub fn is_equal_node(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let Some(other) = node_arg(&args[0]) else {
        return Ok(Value::Bool(false));
    };
    let a = serialize::fragment(&r.doc.borrow(), r.id);
    let b = serialize::fragment(&other.doc.borrow(), other.id);
    Ok(Value::Bool(a == b))
}

pub fn lookup_namespace_uri(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let prefix = super::opt_str_arg(args, 0);
    let from = start_element(&r);
    let uri = from.and_then(|e| r.doc.borrow().lookup_ns(e, prefix.as_deref()));
    Ok(opt_s(uri.as_deref()))
}

pub fn lookup_prefix(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let uri = str_arg(args, 0);
    let from = start_element(&r);
    let p = from
        .and_then(|e| r.doc.borrow().lookup_prefix(e, &uri))
        .flatten();
    Ok(opt_s(p.as_deref()))
}

pub fn is_default_namespace(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let uri = str_arg(args, 0);
    let from = start_element(&r);
    Ok(Value::Bool(
        from.and_then(|e| r.doc.borrow().lookup_ns(e, None))
            .is_some_and(|u| u == uri),
    ))
}

/// The element a namespace lookup starts from: the node itself, an
/// attribute's owner, a document's root.
fn start_element(r: &NodeRef) -> Option<NodeId> {
    let d = r.doc.borrow();
    match d.node(r.id).kind {
        NodeKind::Element => Some(r.id),
        NodeKind::Document => d.document_element(),
        _ => {
            let mut cur = d.node(r.id).parent;
            while let Some(p) = cur {
                if d.node(p).kind == NodeKind::Element {
                    return Some(p);
                }
                cur = d.node(p).parent;
            }
            None
        }
    }
}

pub fn get_line_no(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let line = r.doc.borrow().node(r.id).line;
    Ok(Value::Int(i64::from(line)))
}

/// `getNodePath()`: libxml's `xmlGetNodePath`.
pub fn get_node_path(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let d = r.doc.borrow();
    let mut parts: Vec<String> = Vec::new();
    let mut cur = Some(r.id);
    while let Some(id) = cur {
        let n = d.node(id);
        let step = match n.kind {
            NodeKind::Document => {
                cur = None;
                continue;
            }
            NodeKind::Element => {
                let name = String::from_utf8_lossy(&n.qualified_name()).into_owned();
                let siblings: Vec<NodeId> = n
                    .parent
                    .map(|p| d.children(p))
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|&c| {
                        d.node(c).kind == NodeKind::Element
                            && d.node(c).qualified_name() == n.qualified_name()
                    })
                    .collect();
                if siblings.len() > 1 {
                    let pos = siblings.iter().position(|&c| c == id).unwrap_or(0) + 1;
                    format!("{name}[{pos}]")
                } else {
                    name
                }
            }
            NodeKind::Attribute => format!("@{}", String::from_utf8_lossy(&n.qualified_name())),
            NodeKind::Text | NodeKind::CData => {
                let siblings: Vec<NodeId> = n
                    .parent
                    .map(|p| d.children(p))
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|&c| matches!(d.node(c).kind, NodeKind::Text | NodeKind::CData))
                    .collect();
                if siblings.len() > 1 {
                    format!(
                        "text()[{}]",
                        siblings.iter().position(|&c| c == id).unwrap_or(0) + 1
                    )
                } else {
                    "text()".to_string()
                }
            }
            NodeKind::Comment => "comment()".to_string(),
            NodeKind::Pi => format!(
                "processing-instruction('{}')",
                String::from_utf8_lossy(&n.name)
            ),
            _ => String::new(),
        };
        parts.push(step);
        cur = n.parent;
    }
    if parts.is_empty() {
        return Ok(s(b"/"));
    }
    parts.reverse();
    Ok(s(format!("/{}", parts.join("/")).as_bytes()))
}

pub fn c14n(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let out = serialize::fragment(&r.doc.borrow(), r.id);
    Ok(s(&out))
}

pub fn contains(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let Some(other) = node_arg(&args[0]) else {
        return Ok(Value::Bool(false));
    };
    if !std::rc::Rc::ptr_eq(&other.doc, &r.doc) {
        return Ok(Value::Bool(false));
    }
    Ok(Value::Bool(
        other.id == r.id || r.doc.borrow().is_ancestor(r.id, other.id),
    ))
}

pub fn get_root_node(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let mut cur = r.id;
    loop {
        let p = r.doc.borrow().node(cur).parent;
        match p {
            Some(p) => cur = p,
            None => break,
        }
    }
    Ok(Value::Object(wrap(ctx, &r.doc, cur)?))
}

pub fn compare_document_position(
    _: &mut Ctx,
    o: Option<&Object>,
    args: &mut [Value],
) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let Some(other) = node_arg(&args[0]) else {
        return Ok(Value::Int(0));
    };
    if !std::rc::Rc::ptr_eq(&other.doc, &r.doc) {
        return Ok(Value::Int(1 | 32 | 4));
    }
    if other.id == r.id {
        return Ok(Value::Int(0));
    }
    let d = r.doc.borrow();
    if d.is_ancestor(other.id, r.id) {
        return Ok(Value::Int(8 | 2)); // contains | preceding
    }
    if d.is_ancestor(r.id, other.id) {
        return Ok(Value::Int(16 | 4)); // contained by | following
    }
    let order = d.descendants(crate::tree::DOCUMENT);
    let a = order.iter().position(|&n| n == r.id);
    let b = order.iter().position(|&n| n == other.id);
    Ok(Value::Int(match (a, b) {
        (Some(a), Some(b)) if b < a => 2,
        _ => 4,
    }))
}

pub fn register(r: &mut Registry) {
    r.class("DOMNode")
        .native_props(PROPS)
        .class_const("DOCUMENT_POSITION_DISCONNECTED", Value::Int(1))
        .class_const("DOCUMENT_POSITION_PRECEDING", Value::Int(2))
        .class_const("DOCUMENT_POSITION_FOLLOWING", Value::Int(4))
        .class_const("DOCUMENT_POSITION_CONTAINS", Value::Int(8))
        .class_const("DOCUMENT_POSITION_CONTAINED_BY", Value::Int(16))
        .class_const("DOCUMENT_POSITION_IMPLEMENTATION_SPECIFIC", Value::Int(32))
        .method("appendChild", nm!(1, Some(1), append_child))
        .method("insertBefore", nm!(1, Some(2), insert_before))
        .method("removeChild", nm!(1, Some(1), remove_child))
        .method("replaceChild", nm!(2, Some(2), replace_child))
        .method("hasChildNodes", nm!(0, Some(0), has_child_nodes))
        .method("hasAttributes", nm!(0, Some(0), has_attributes))
        .method("cloneNode", nm!(0, Some(1), clone_node))
        .method("normalize", nm!(0, Some(0), normalize))
        .method("isSameNode", nm!(1, Some(1), is_same_node))
        .method("isEqualNode", nm!(1, Some(1), is_equal_node))
        .method("lookupNamespaceURI", nm!(1, Some(1), lookup_namespace_uri))
        .method("lookupPrefix", nm!(1, Some(1), lookup_prefix))
        .method("isDefaultNamespace", nm!(1, Some(1), is_default_namespace))
        .method("getLineNo", nm!(0, Some(0), get_line_no))
        .method("getNodePath", nm!(0, Some(0), get_node_path))
        .method("C14N", nm!(0, Some(4), c14n))
        .method("contains", nm!(1, Some(1), contains))
        .method("getRootNode", nm!(0, Some(1), get_root_node))
        .method(
            "compareDocumentPosition",
            nm!(1, Some(1), compare_document_position),
        )
        .method("isSupported", nm!(2, Some(2), is_supported))
        .finish();
}

fn is_supported(_: &mut Ctx, _: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Ok(Value::Bool(false))
}
