//! `DOMNodeList` and `DOMNamedNodeMap`: live views over the tree (a
//! `childNodes` list or a `getElementsByTagName()` result recomputes on
//! every read, as libxml's do), or a fixed set (an XPath result).

use rphp_runtime::{nm, Ctx, Interp, NativeProps, NativeResult, Registry, Unwind};
use rphp_value::{Object, Payload, Value};

use super::{this, wrap};
use crate::tree::{Doc, NodeId, NodeRef};

#[derive(Clone)]
pub enum ListKind {
    Children(NodeId),
    /// The element children only (`children`).
    ElementChildren(NodeId),
    ByTagName(NodeId, Vec<u8>),
    ByTagNameNs(NodeId, Vec<u8>, Vec<u8>),
    Fixed(Vec<NodeId>),
}

#[derive(Clone)]
pub struct ListState {
    pub doc: Doc,
    pub kind: ListKind,
}

impl ListState {
    pub fn items(&self) -> Vec<NodeId> {
        let d = self.doc.borrow();
        match &self.kind {
            ListKind::Children(p) => d.children(*p),
            ListKind::ElementChildren(p) => d
                .children(*p)
                .into_iter()
                .filter(|&c| d.node(c).kind == crate::tree::NodeKind::Element)
                .collect(),
            ListKind::ByTagName(root, name) => d.elements_by_name(*root, name),
            ListKind::ByTagNameNs(root, ns, local) => d.elements_by_ns(*root, ns, local),
            ListKind::Fixed(v) => v.clone(),
        }
    }
}

fn list_state(o: &Object) -> Result<ListState, Unwind> {
    o.with_payload::<ListState, _>(|s| s.clone())
        .ok_or_else(|| Unwind::error("Couldn't fetch DOMNodeList"))
}

fn new_list(ctx: &mut Ctx, doc: &Doc, kind: ListKind) -> Result<Object, Unwind> {
    let modern = doc.borrow().modern;
    let name: &[u8] = if modern {
        b"Dom\\NodeList"
    } else {
        b"DOMNodeList"
    };
    let cid = ctx.lookup_class_or_error(name)?;
    let o = ctx.instantiate(cid);
    o.set_payload(Payload::Native(Box::new(ListState {
        doc: doc.clone(),
        kind,
    })));
    Ok(o)
}

/// A php 8.4 `Dom\HTMLCollection` (elements only, `namedItem()`).
fn new_collection(ctx: &mut Ctx, doc: &Doc, kind: ListKind) -> Result<Object, Unwind> {
    let cid = ctx.lookup_class_or_error(b"Dom\\HTMLCollection")?;
    let o = ctx.instantiate(cid);
    o.set_payload(Payload::Native(Box::new(ListState {
        doc: doc.clone(),
        kind,
    })));
    Ok(o)
}

/// `$node->children` (modern): the element children.
pub fn children_collection(ctx: &mut Ctx, r: &NodeRef) -> Result<Object, Unwind> {
    new_collection(ctx, &r.doc, ListKind::ElementChildren(r.id))
}

pub fn children_list(ctx: &mut Ctx, r: &NodeRef) -> Result<Object, Unwind> {
    new_list(ctx, &r.doc, ListKind::Children(r.id))
}

pub fn by_tag_name(ctx: &mut Ctx, r: &NodeRef, name: &[u8]) -> Result<Object, Unwind> {
    let modern = r.doc.borrow().modern;
    let name = name.to_vec();
    if modern {
        return new_collection(ctx, &r.doc, ListKind::ByTagName(r.id, name));
    }
    new_list(ctx, &r.doc, ListKind::ByTagName(r.id, name))
}

pub fn by_tag_name_ns(
    ctx: &mut Ctx,
    r: &NodeRef,
    ns: &[u8],
    local: &[u8],
) -> Result<Object, Unwind> {
    if r.doc.borrow().modern {
        return new_collection(
            ctx,
            &r.doc,
            ListKind::ByTagNameNs(r.id, ns.to_vec(), local.to_vec()),
        );
    }
    new_list(
        ctx,
        &r.doc,
        ListKind::ByTagNameNs(r.id, ns.to_vec(), local.to_vec()),
    )
}

/// A fixed `Dom\\HTMLCollection` (`getElementsByClassName()`).
pub fn fixed_collection(ctx: &mut Ctx, doc: &Doc, items: Vec<NodeId>) -> Result<Object, Unwind> {
    new_collection(ctx, doc, ListKind::Fixed(items))
}

pub fn fixed_list(ctx: &mut Ctx, doc: &Doc, items: Vec<NodeId>) -> Result<Object, Unwind> {
    new_list(ctx, doc, ListKind::Fixed(items))
}

fn list_get(it: &mut Interp, o: &Object, name: &[u8]) -> Option<Result<Value, Unwind>> {
    if name != b"length" {
        return None;
    }
    let st = list_state(o).ok()?;
    let _ = it;
    Some(Ok(Value::Int(st.items().len() as i64)))
}

fn list_set(it: &mut Interp, o: &Object, name: &[u8], _: Value) -> Option<Result<(), Unwind>> {
    if name != b"length" {
        return None;
    }
    Some(Err(Unwind::error(format!(
        "Cannot modify readonly property {}::$length",
        it.class_name_of(o)
    ))))
}

fn list_item(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let st = list_state(this(o)?)?;
    let i = args[0].to_int();
    let items = st.items();
    if i < 0 || i as usize >= items.len() {
        return Ok(Value::Null);
    }
    Ok(Value::Object(wrap(ctx, &st.doc, items[i as usize])?))
}

fn list_count(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let st = list_state(this(o)?)?;
    Ok(Value::Int(st.items().len() as i64))
}

fn list_get_iterator(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let st = list_state(this(o)?)?;
    let mut items = Vec::new();
    for (i, id) in st.items().into_iter().enumerate() {
        items.push((Value::Int(i as i64), Value::Object(wrap(ctx, &st.doc, id)?)));
    }
    Ok(Value::Object(rphp_stdlib::new_internal_iterator(
        ctx, items,
    )))
}

// `$list[$i]` through ArrayAccess-like offsets php's DOMNodeList supports.
fn list_offset_get(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    list_item(ctx, o, args)
}

fn list_offset_exists(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let st = list_state(this(o)?)?;
    let i = args[0].to_int();
    Ok(Value::Bool(i >= 0 && (i as usize) < st.items().len()))
}

fn list_offset_set(_: &mut Ctx, _: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Err(Unwind::error("Cannot write to DOMNodeList"))
}

fn list_offset_unset(_: &mut Ctx, _: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Err(Unwind::error("Cannot unset from DOMNodeList"))
}

// ---- named node map --------------------------------------------------------

#[derive(Clone)]
pub enum MapKind {
    Attributes(NodeId),
    /// A doctype's entity declarations.
    Entities(NodeId),
    /// A doctype's notation declarations.
    Notations(NodeId),
}

#[derive(Clone)]
pub struct MapState {
    pub doc: Doc,
    pub kind: MapKind,
}

impl MapState {
    pub fn items(&self) -> Vec<NodeId> {
        let d = self.doc.borrow();
        match self.kind {
            MapKind::Attributes(e) => d.node(e).attrs.clone(),
            MapKind::Entities(dt) => d.decls_of(dt, crate::tree::NodeKind::Entity),
            MapKind::Notations(dt) => d.decls_of(dt, crate::tree::NodeKind::Notation),
        }
    }
}

fn map_state(o: &Object) -> Result<MapState, Unwind> {
    o.with_payload::<MapState, _>(|s| s.clone())
        .ok_or_else(|| Unwind::error("Couldn't fetch DOMNamedNodeMap"))
}

fn new_map(ctx: &mut Ctx, doc: &Doc, kind: MapKind) -> Result<Object, Unwind> {
    let modern = doc.borrow().modern;
    let name: &[u8] = if modern {
        b"Dom\\NamedNodeMap"
    } else {
        b"DOMNamedNodeMap"
    };
    let cid = ctx.lookup_class_or_error(name)?;
    let o = ctx.instantiate(cid);
    o.set_payload(Payload::Native(Box::new(MapState {
        doc: doc.clone(),
        kind,
    })));
    Ok(o)
}

pub fn attr_map(ctx: &mut Ctx, r: &NodeRef) -> Result<Object, Unwind> {
    new_map(ctx, &r.doc, MapKind::Attributes(r.id))
}

/// `$doctype->entities` / `->notations` (a `Dom\DtdNamedNodeMap` in the
/// modern API).
pub fn dtd_map(ctx: &mut Ctx, r: &NodeRef, entities: bool) -> Result<Object, Unwind> {
    let kind = if entities {
        MapKind::Entities(r.id)
    } else {
        MapKind::Notations(r.id)
    };
    if r.doc.borrow().modern {
        let cid = ctx.lookup_class_or_error(b"Dom\\DtdNamedNodeMap")?;
        let o = ctx.instantiate(cid);
        o.set_payload(Payload::Native(Box::new(MapState {
            doc: r.doc.clone(),
            kind,
        })));
        return Ok(o);
    }
    new_map(ctx, &r.doc, kind)
}

fn map_get(_: &mut Interp, o: &Object, name: &[u8]) -> Option<Result<Value, Unwind>> {
    if name != b"length" {
        return None;
    }
    let st = map_state(o).ok()?;
    Some(Ok(Value::Int(st.items().len() as i64)))
}

fn map_set(it: &mut Interp, o: &Object, name: &[u8], _: Value) -> Option<Result<(), Unwind>> {
    if name != b"length" {
        return None;
    }
    Some(Err(Unwind::error(format!(
        "Cannot modify readonly property {}::$length",
        it.class_name_of(o)
    ))))
}

fn map_get_named_item(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let st = map_state(this(o)?)?;
    let name = super::str_arg(args, 0);
    let found = {
        let d = st.doc.borrow();
        st.items()
            .into_iter()
            .find(|&a| d.node(a).qualified_name() == name)
    };
    match found {
        Some(a) => Ok(Value::Object(wrap(ctx, &st.doc, a)?)),
        None => Ok(Value::Null),
    }
}

fn map_get_named_item_ns(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let st = map_state(this(o)?)?;
    let ns = super::opt_str_arg(args, 0).unwrap_or_default();
    let local = super::str_arg(args, 1);
    let found = {
        let d = st.doc.borrow();
        st.items().into_iter().find(|&a| {
            let n = d.node(a);
            n.name == local && n.ns.as_deref().unwrap_or(b"") == ns.as_slice()
        })
    };
    match found {
        Some(a) => Ok(Value::Object(wrap(ctx, &st.doc, a)?)),
        None => Ok(Value::Null),
    }
}

fn map_item(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let st = map_state(this(o)?)?;
    let i = args[0].to_int();
    let items = st.items();
    if i < 0 || i as usize >= items.len() {
        return Ok(Value::Null);
    }
    Ok(Value::Object(wrap(ctx, &st.doc, items[i as usize])?))
}

fn map_count(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let st = map_state(this(o)?)?;
    Ok(Value::Int(st.items().len() as i64))
}

fn map_get_iterator(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let st = map_state(this(o)?)?;
    let mut items = Vec::new();
    for id in st.items() {
        let key = st.doc.borrow().node(id).qualified_name();
        items.push((Value::string(&key), Value::Object(wrap(ctx, &st.doc, id)?)));
    }
    Ok(Value::Object(rphp_stdlib::new_internal_iterator(
        ctx, items,
    )))
}

fn map_offset_get(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    match &*args[0].deref() {
        Value::Int(_) => map_item(ctx, o, args),
        _ => map_get_named_item(ctx, o, args),
    }
}

fn map_offset_exists(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    Ok(Value::Bool(!matches!(
        map_offset_get(ctx, o, args)?,
        Value::Null
    )))
}

fn collection_named_item(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let st = list_state(this(o)?)?;
    let name = super::str_arg(args, 0);
    let found = {
        let d = st.doc.borrow();
        st.items().into_iter().find(|&e| {
            d.node(e).attrs.iter().any(|&a| {
                let an = d.node(a);
                (an.name == b"id" || an.name == b"name") && d.attr_value(a) == name
            })
        })
    };
    match found {
        Some(e) => Ok(Value::Object(wrap(ctx, &st.doc, e)?)),
        None => Ok(Value::Null),
    }
}

/// The php 8.4 list classes over the same states.
pub fn register_modern(r: &mut Registry) {
    r.class("Dom\\NodeList")
        .implements(&["IteratorAggregate", "Countable", "ArrayAccess"])
        .native_props(NativeProps {
            names: &["length"],
            get: list_get,
            set: list_set,
            isset: None,
            unset: None,
            list: None,
            debug: None,
        })
        .method("item", nm!(1, Some(1), list_item))
        .method("count", nm!(0, Some(0), list_count))
        .method("getIterator", nm!(0, Some(0), list_get_iterator))
        .method("offsetGet", nm!(1, Some(1), list_offset_get))
        .method("offsetExists", nm!(1, Some(1), list_offset_exists))
        .method("offsetSet", nm!(2, Some(2), list_offset_set))
        .method("offsetUnset", nm!(1, Some(1), list_offset_unset))
        .finish();
    r.class("Dom\\HTMLCollection")
        .implements(&["IteratorAggregate", "Countable", "ArrayAccess"])
        .native_props(NativeProps {
            names: &["length"],
            get: list_get,
            set: list_set,
            isset: None,
            unset: None,
            list: None,
            debug: None,
        })
        .method("item", nm!(1, Some(1), list_item))
        .method("namedItem", nm!(1, Some(1), collection_named_item))
        .method("count", nm!(0, Some(0), list_count))
        .method("getIterator", nm!(0, Some(0), list_get_iterator))
        .method("offsetGet", nm!(1, Some(1), list_offset_get))
        .method("offsetExists", nm!(1, Some(1), list_offset_exists))
        .method("offsetSet", nm!(2, Some(2), list_offset_set))
        .method("offsetUnset", nm!(1, Some(1), list_offset_unset))
        .finish();
    for name in ["Dom\\NamedNodeMap", "Dom\\DtdNamedNodeMap"] {
        r.class(name)
            .implements(&["IteratorAggregate", "Countable", "ArrayAccess"])
            .native_props(NativeProps {
                names: &["length"],
                get: map_get,
                set: map_set,
                isset: None,
                unset: None,
                list: None,
                debug: None,
            })
            .method("getNamedItem", nm!(1, Some(1), map_get_named_item))
            .method("getNamedItemNS", nm!(2, Some(2), map_get_named_item_ns))
            .method("item", nm!(1, Some(1), map_item))
            .method("count", nm!(0, Some(0), map_count))
            .method("getIterator", nm!(0, Some(0), map_get_iterator))
            .method("offsetGet", nm!(1, Some(1), map_offset_get))
            .method("offsetExists", nm!(1, Some(1), map_offset_exists))
            .method("offsetSet", nm!(2, Some(2), list_offset_set))
            .method("offsetUnset", nm!(1, Some(1), list_offset_unset))
            .finish();
    }
}

pub fn register(r: &mut Registry) {
    r.class("DOMNodeList")
        .implements(&["IteratorAggregate", "Countable", "ArrayAccess"])
        .native_props(NativeProps {
            names: &["length"],
            get: list_get,
            set: list_set,
            isset: None,
            unset: None,
            list: None,
            debug: None,
        })
        .method("item", nm!(1, Some(1), list_item))
        .method("count", nm!(0, Some(0), list_count))
        .method("getIterator", nm!(0, Some(0), list_get_iterator))
        .method("offsetGet", nm!(1, Some(1), list_offset_get))
        .method("offsetExists", nm!(1, Some(1), list_offset_exists))
        .method("offsetSet", nm!(2, Some(2), list_offset_set))
        .method("offsetUnset", nm!(1, Some(1), list_offset_unset))
        .finish();
    r.class("DOMNamedNodeMap")
        .implements(&["IteratorAggregate", "Countable", "ArrayAccess"])
        .native_props(NativeProps {
            names: &["length"],
            get: map_get,
            set: map_set,
            isset: None,
            unset: None,
            list: None,
            debug: None,
        })
        .method("getNamedItem", nm!(1, Some(1), map_get_named_item))
        .method("getNamedItemNS", nm!(2, Some(2), map_get_named_item_ns))
        .method("item", nm!(1, Some(1), map_item))
        .method("count", nm!(0, Some(0), map_count))
        .method("getIterator", nm!(0, Some(0), map_get_iterator))
        .method("offsetGet", nm!(1, Some(1), map_offset_get))
        .method("offsetExists", nm!(1, Some(1), map_offset_exists))
        .method("offsetSet", nm!(2, Some(2), list_offset_set))
        .method("offsetUnset", nm!(1, Some(1), list_offset_unset))
        .finish();
}
