//! `DOMElement`, and the `DOMParentNode` / `DOMChildNode` methods
//! (`append()`, `before()`, `remove()`, …) shared with the other classes.

use rphp_runtime::{nm, Ctx, NativeResult, Registry, Unwind};
use rphp_value::{Object, Value};

use super::{
    adopt_into, dom_exception, node_arg, node_ref, opt_str_arg, orphan_doc, set_node_ref, str_arg,
    wrap, wrap_value,
};
use crate::tree::{is_valid_name, split_qname, DomError, NodeId, NodeKind, NodeRef, NsDecl};

fn s(v: &[u8]) -> Value {
    Value::string(v)
}

const XMLNS_URI: &[u8] = b"http://www.w3.org/2000/xmlns/";

/// `new DOMElement(string $qualifiedName, ?string $value = null, string $namespace = "")`
fn construct(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = super::this(o)?;
    let qname = str_arg(args, 0);
    let (prefix, local) = split_qname(&qname);
    if !is_valid_name(local) || qname.is_empty() {
        return Err(dom_exception(ctx, DomError::InvalidCharacter));
    }
    let ns = opt_str_arg(args, 2).filter(|u| !u.is_empty());
    if prefix.is_some() && ns.is_none() {
        return Err(dom_exception(ctx, DomError::Namespace));
    }
    let doc = orphan_doc();
    let id = {
        let mut d = doc.borrow_mut();
        let id = d.create_element(local, prefix, ns.as_deref());
        if let Some(uri) = &ns {
            d.node_mut(id).ns_decls.push(NsDecl {
                prefix: prefix.map(<[u8]>::to_vec),
                uri: uri.clone(),
            });
        }
        if let Some(v) = opt_str_arg(args, 1) {
            let t = d.create_text(NodeKind::Text, &v);
            d.link_last(id, t);
        }
        id
    };
    set_node_ref(
        o,
        NodeRef {
            doc: doc.clone(),
            id,
        },
    );
    doc.borrow_mut().remember(id, o);
    Ok(Value::Null)
}

pub fn get_attribute(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let name = str_arg(args, 0);
    let d = r.doc.borrow();
    Ok(s(&d
        .find_attr(r.id, &name)
        .map(|a| d.attr_value(a))
        .unwrap_or_default()))
}

pub fn get_attribute_ns(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let ns = opt_str_arg(args, 0);
    let local = str_arg(args, 1);
    let d = r.doc.borrow();
    Ok(s(&d
        .find_attr_ns(r.id, ns.as_deref(), &local)
        .map(|a| d.attr_value(a))
        .unwrap_or_default()))
}

pub fn has_attribute(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let has = r.doc.borrow().find_attr(r.id, &str_arg(args, 0)).is_some();
    Ok(Value::Bool(has))
}

pub fn has_attribute_ns(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let ns = opt_str_arg(args, 0);
    let has = r
        .doc
        .borrow()
        .find_attr_ns(r.id, ns.as_deref(), &str_arg(args, 1))
        .is_some();
    Ok(Value::Bool(has))
}

pub fn set_attribute(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let name = str_arg(args, 0);
    if !is_valid_name(&name) {
        return Err(dom_exception(ctx, DomError::InvalidCharacter));
    }
    let value = str_arg(args, 1);
    let a = r.doc.borrow_mut().set_attr(r.id, &name, &value);
    Ok(Value::Object(wrap(ctx, &r.doc, a)?))
}

/// `setAttributeNS`: an `xmlns` declaration goes to the element's
/// namespace declarations; any other prefix gets declared on the element
/// when nothing in scope binds it.
pub fn set_attribute_ns(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let ns = opt_str_arg(args, 0).filter(|u| !u.is_empty());
    let qname = str_arg(args, 1);
    let value = str_arg(args, 2);
    let (prefix, local) = split_qname(&qname);
    if !is_valid_name(local) {
        return Err(dom_exception(ctx, DomError::InvalidCharacter));
    }
    if ns.as_deref() == Some(XMLNS_URI) || qname == b"xmlns" || prefix == Some(b"xmlns") {
        let decl_prefix = if qname == b"xmlns" {
            None
        } else {
            Some(local.to_vec())
        };
        let mut d = r.doc.borrow_mut();
        let decls = &mut d.node_mut(r.id).ns_decls;
        match decls.iter_mut().find(|dcl| dcl.prefix == decl_prefix) {
            Some(dcl) => dcl.uri = value,
            None => decls.push(NsDecl {
                prefix: decl_prefix,
                uri: value,
            }),
        }
        return Ok(Value::Null);
    }
    if prefix.is_some() && ns.is_none() {
        return Err(dom_exception(ctx, DomError::Namespace));
    }
    let mut d = r.doc.borrow_mut();
    if let (Some(p), Some(uri)) = (prefix, &ns) {
        if d.lookup_ns(r.id, Some(p)).as_deref() != Some(uri.as_slice()) {
            d.node_mut(r.id).ns_decls.push(NsDecl {
                prefix: Some(p.to_vec()),
                uri: uri.clone(),
            });
        }
    }
    if let Some(a) = d.find_attr_ns(r.id, ns.as_deref(), local) {
        d.set_attr_value(a, &value);
        d.node_mut(a).prefix = prefix.map(<[u8]>::to_vec);
    } else {
        let a = d.create_attr(local, prefix, ns.as_deref(), &value);
        d.node_mut(a).parent = Some(r.id);
        d.node_mut(r.id).attrs.push(a);
    }
    Ok(Value::Null)
}

pub fn remove_attribute(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let removed = r.doc.borrow_mut().remove_attr(r.id, &str_arg(args, 0));
    Ok(Value::Bool(removed))
}

pub fn remove_attribute_ns(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let ns = opt_str_arg(args, 0);
    let mut d = r.doc.borrow_mut();
    if let Some(a) = d.find_attr_ns(r.id, ns.as_deref(), &str_arg(args, 1)) {
        d.unlink(a);
    }
    Ok(Value::Null)
}

pub fn get_attribute_node(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let a = r.doc.borrow().find_attr(r.id, &str_arg(args, 0));
    match a {
        Some(a) => Ok(Value::Object(wrap(ctx, &r.doc, a)?)),
        None => Ok(Value::Bool(false)),
    }
}

pub fn get_attribute_node_ns(
    ctx: &mut Ctx,
    o: Option<&Object>,
    args: &mut [Value],
) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let ns = opt_str_arg(args, 0);
    let a = r
        .doc
        .borrow()
        .find_attr_ns(r.id, ns.as_deref(), &str_arg(args, 1));
    wrap_value(ctx, &r.doc, a)
}

pub fn set_attribute_node(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let Some(attr) = node_arg(&args[0]) else {
        return Err(Unwind::type_error(
            "DOMElement::setAttributeNode(): Argument #1 ($attr) must be of type DOMAttr",
        ));
    };
    let aid = adopt_into(ctx, &r, &attr)?;
    let res = r.doc.borrow_mut().set_attr_node(r.id, aid);
    match res {
        Ok(Some(old)) => Ok(Value::Object(wrap(ctx, &r.doc, old)?)),
        Ok(None) => Ok(Value::Null),
        Err(e) => Err(dom_exception(ctx, e)),
    }
}

pub fn remove_attribute_node(
    ctx: &mut Ctx,
    o: Option<&Object>,
    args: &mut [Value],
) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let Some(attr) = node_arg(&args[0]) else {
        return Err(Unwind::type_error(
            "DOMElement::removeAttributeNode(): Argument #1 ($attr) must be of type DOMAttr",
        ));
    };
    if !std::rc::Rc::ptr_eq(&attr.doc, &r.doc) || r.doc.borrow().node(attr.id).parent != Some(r.id)
    {
        return Err(dom_exception(ctx, DomError::NotFound));
    }
    r.doc.borrow_mut().unlink(attr.id);
    Ok(Value::Object(wrap(ctx, &r.doc, attr.id)?))
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

pub fn set_id_attribute(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let name = str_arg(args, 0);
    let is_id = args.get(1).is_some_and(Value::to_bool);
    let a = r.doc.borrow().find_attr(r.id, &name);
    match a {
        Some(a) => {
            r.doc.borrow_mut().node_mut(a).is_id = is_id;
            Ok(Value::Null)
        }
        None => Err(dom_exception(ctx, DomError::NotFound)),
    }
}

pub fn set_id_attribute_ns(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let ns = opt_str_arg(args, 0);
    let is_id = args.get(2).is_some_and(Value::to_bool);
    let a = r
        .doc
        .borrow()
        .find_attr_ns(r.id, ns.as_deref(), &str_arg(args, 1));
    match a {
        Some(a) => {
            r.doc.borrow_mut().node_mut(a).is_id = is_id;
            Ok(Value::Null)
        }
        None => Err(dom_exception(ctx, DomError::NotFound)),
    }
}

pub fn set_id_attribute_node(
    ctx: &mut Ctx,
    o: Option<&Object>,
    args: &mut [Value],
) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let Some(attr) = node_arg(&args[0]) else {
        return Err(Unwind::type_error(
            "DOMElement::setIdAttributeNode(): Argument #1 ($attr) must be of type DOMAttr",
        ));
    };
    if r.doc.borrow().node(attr.id).parent != Some(r.id) {
        return Err(dom_exception(ctx, DomError::NotFound));
    }
    r.doc.borrow_mut().node_mut(attr.id).is_id = args.get(1).is_some_and(Value::to_bool);
    Ok(Value::Null)
}

pub fn get_attribute_names(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let d = r.doc.borrow();
    let mut out = rphp_value::Array::new();
    for dcl in &d.node(r.id).ns_decls {
        let name = match &dcl.prefix {
            Some(p) => [b"xmlns:".as_slice(), p].concat(),
            None => b"xmlns".to_vec(),
        };
        out.push(s(&name));
    }
    for &a in &d.node(r.id).attrs {
        out.push(s(&d.node(a).qualified_name()));
    }
    Ok(Value::Array(out))
}

pub fn toggle_attribute(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let name = str_arg(args, 0);
    if !is_valid_name(&name) {
        return Err(dom_exception(ctx, DomError::InvalidCharacter));
    }
    let force = args
        .get(1)
        .map(|v| v.deref().into_owned())
        .filter(|v| !matches!(v, Value::Null))
        .map(|v| v.to_bool());
    let mut d = r.doc.borrow_mut();
    let has = d.find_attr(r.id, &name).is_some();
    match (has, force) {
        (true, Some(true)) => Ok(Value::Bool(true)),
        (true, _) => {
            d.remove_attr(r.id, &name);
            Ok(Value::Bool(false))
        }
        (false, Some(false)) => Ok(Value::Bool(false)),
        (false, _) => {
            d.set_attr(r.id, &name, b"");
            Ok(Value::Bool(true))
        }
    }
}

// ---- ParentNode / ChildNode --------------------------------------------------

/// The nodes `append()` & co. take: a `DOMNode` as is, a string as a text
/// node of `r`'s document.
fn nodes_of(ctx: &mut Ctx, r: &NodeRef, args: &[Value]) -> Result<Vec<NodeId>, Unwind> {
    let mut out = Vec::new();
    for v in args {
        match &*v.deref() {
            Value::Object(_) => {
                match node_arg(v) {
                    Some(n) => out.push(adopt_into(ctx, r, &n)?),
                    None => return Err(Unwind::type_error(
                        "DOMNode::append(): Argument #1 ($nodes) must be of type DOMNode|string",
                    )),
                }
            }
            other => {
                let t = r
                    .doc
                    .borrow_mut()
                    .create_text(NodeKind::Text, &other.to_php_bytes());
                out.push(t);
            }
        }
    }
    Ok(out)
}

pub fn append(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    for id in nodes_of(ctx, &r, args)? {
        if let Err(e) = r.doc.borrow_mut().append_child(r.id, id) {
            return Err(dom_exception(ctx, e));
        }
    }
    Ok(Value::Null)
}

pub fn prepend(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let first = r.doc.borrow().node(r.id).first_child;
    for id in nodes_of(ctx, &r, args)? {
        if let Err(e) = r.doc.borrow_mut().insert_before(r.id, id, first) {
            return Err(dom_exception(ctx, e));
        }
    }
    Ok(Value::Null)
}

pub fn replace_children(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let new = nodes_of(ctx, &r, args)?;
    {
        let mut d = r.doc.borrow_mut();
        for c in d.children(r.id) {
            d.unlink(c);
        }
    }
    for id in new {
        if let Err(e) = r.doc.borrow_mut().append_child(r.id, id) {
            return Err(dom_exception(ctx, e));
        }
    }
    Ok(Value::Null)
}

pub fn before(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let Some(parent) = r.doc.borrow().node(r.id).parent else {
        return Ok(Value::Null);
    };
    let pr = NodeRef {
        doc: r.doc.clone(),
        id: parent,
    };
    for id in nodes_of(ctx, &pr, args)? {
        if let Err(e) = r.doc.borrow_mut().insert_before(parent, id, Some(r.id)) {
            return Err(dom_exception(ctx, e));
        }
    }
    Ok(Value::Null)
}

pub fn after(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let Some(parent) = r.doc.borrow().node(r.id).parent else {
        return Ok(Value::Null);
    };
    let next = r.doc.borrow().node(r.id).next;
    let pr = NodeRef {
        doc: r.doc.clone(),
        id: parent,
    };
    for id in nodes_of(ctx, &pr, args)? {
        if let Err(e) = r.doc.borrow_mut().insert_before(parent, id, next) {
            return Err(dom_exception(ctx, e));
        }
    }
    Ok(Value::Null)
}

pub fn remove(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    r.doc.borrow_mut().unlink(r.id);
    Ok(Value::Null)
}

pub fn replace_with(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let Some(parent) = r.doc.borrow().node(r.id).parent else {
        return Ok(Value::Null);
    };
    let next = r.doc.borrow().node(r.id).next;
    let pr = NodeRef {
        doc: r.doc.clone(),
        id: parent,
    };
    let new = nodes_of(ctx, &pr, args)?;
    r.doc.borrow_mut().unlink(r.id);
    for id in new {
        if let Err(e) = r.doc.borrow_mut().insert_before(parent, id, next) {
            return Err(dom_exception(ctx, e));
        }
    }
    Ok(Value::Null)
}

pub fn insert_adjacent_element(
    ctx: &mut Ctx,
    o: Option<&Object>,
    args: &mut [Value],
) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let pos = str_arg(args, 0).to_ascii_lowercase();
    let Some(el) = node_arg(&args[1]) else {
        return Err(Unwind::type_error("DOMElement::insertAdjacentElement(): Argument #2 ($element) must be of type DOMElement"));
    };
    let id = adopt_into(ctx, &r, &el)?;
    let res = {
        let mut d = r.doc.borrow_mut();
        match pos.as_slice() {
            b"beforebegin" => match d.node(r.id).parent {
                Some(p) => d.insert_before(p, id, Some(r.id)),
                None => return Ok(Value::Null),
            },
            b"afterbegin" => {
                let first = d.node(r.id).first_child;
                d.insert_before(r.id, id, first)
            }
            b"beforeend" => d.append_child(r.id, id),
            b"afterend" => match d.node(r.id).parent {
                Some(p) => {
                    let next = d.node(r.id).next;
                    d.insert_before(p, id, next)
                }
                None => return Ok(Value::Null),
            },
            _ => return Err(dom_exception(ctx, DomError::Namespace)),
        }
    };
    match res {
        Ok(id) => Ok(Value::Object(wrap(ctx, &r.doc, id)?)),
        Err(e) => Err(dom_exception(ctx, e)),
    }
}

pub fn insert_adjacent_text(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let t = r
        .doc
        .borrow_mut()
        .create_text(NodeKind::Text, &str_arg(args, 1));
    let node = wrap(ctx, &r.doc, t)?;
    let mut a = [args[0].clone(), Value::Object(node)];
    insert_adjacent_element(ctx, o, &mut a)?;
    Ok(Value::Null)
}

pub fn register(r: &mut Registry) {
    r.class("DOMElement")
        .extends("DOMNode")
        .implements(&["DOMParentNode", "DOMChildNode"])
        .native_props(super::node::ELEMENT_PROPS)
        .method("__construct", nm!(1, Some(3), construct))
        .method("getAttribute", nm!(1, Some(1), get_attribute))
        .method("getAttributeNS", nm!(2, Some(2), get_attribute_ns))
        .method("hasAttribute", nm!(1, Some(1), has_attribute))
        .method("hasAttributeNS", nm!(2, Some(2), has_attribute_ns))
        .method("setAttribute", nm!(2, Some(2), set_attribute))
        .method("setAttributeNS", nm!(3, Some(3), set_attribute_ns))
        .method("removeAttribute", nm!(1, Some(1), remove_attribute))
        .method("removeAttributeNS", nm!(2, Some(2), remove_attribute_ns))
        .method("getAttributeNode", nm!(1, Some(1), get_attribute_node))
        .method("getAttributeNodeNS", nm!(2, Some(2), get_attribute_node_ns))
        .method("setAttributeNode", nm!(1, Some(1), set_attribute_node))
        .method("setAttributeNodeNS", nm!(1, Some(1), set_attribute_node))
        .method(
            "removeAttributeNode",
            nm!(1, Some(1), remove_attribute_node),
        )
        .method(
            "getElementsByTagName",
            nm!(1, Some(1), get_elements_by_tag_name),
        )
        .method(
            "getElementsByTagNameNS",
            nm!(2, Some(2), get_elements_by_tag_name_ns),
        )
        .method("setIdAttribute", nm!(2, Some(2), set_id_attribute))
        .method("setIdAttributeNS", nm!(3, Some(3), set_id_attribute_ns))
        .method("setIdAttributeNode", nm!(2, Some(2), set_id_attribute_node))
        .method("getAttributeNames", nm!(0, Some(0), get_attribute_names))
        .method("toggleAttribute", nm!(1, Some(2), toggle_attribute))
        .method(
            "insertAdjacentElement",
            nm!(2, Some(2), insert_adjacent_element),
        )
        .method("insertAdjacentText", nm!(2, Some(2), insert_adjacent_text))
        .method("append", nm!(0, None, append))
        .method("prepend", nm!(0, None, prepend))
        .method("replaceChildren", nm!(0, None, replace_children))
        .method("before", nm!(0, None, before))
        .method("after", nm!(0, None, after))
        .method("remove", nm!(0, Some(0), remove))
        .method("replaceWith", nm!(0, None, replace_with))
        .finish();
}
