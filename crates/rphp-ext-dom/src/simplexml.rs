//! `ext/simplexml`: `SimpleXMLElement` over the DOM tree. An instance
//! names a node plus php's *iterator kind* — the element itself, the
//! children of one name (`$x->item`), its attribute list
//! (`$x->attributes()`), or an attribute — and resolves to a node on
//! demand; that is how `$x->item` is at once the first `item`, a list to
//! `foreach` over, and something `count()` measures. The property table
//! php computes (`@attributes`, children grouped by name, text-only
//! children as strings, comments under `comment`) feeds `var_dump()`,
//! `(array)`, `json_encode()` and `get_object_vars()` through the runtime's
//! native-property hooks; the casts go through the object's cast handler.

use rphp_runtime::{nf, nm, Ctx, Interp, NativeFn, NativeProps, NativeResult, Registry, Unwind};
use rphp_value::{Array, ArrayKey, CastTarget, Object, Payload, Value};

use crate::classes::wrap;
use crate::libxml;
use crate::parser::{self, Options};
use crate::serialize;
use crate::tree::{split_qname, Doc, DocData, NodeId, NodeKind, NsDecl, DOCUMENT};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Iter {
    /// The node itself (`foreach` walks its element children).
    None,
    /// `children()`: the element children.
    Element,
    /// `$x->name`: the children named so.
    Child,
    /// `attributes()`.
    AttrList,
}

#[derive(Clone)]
pub struct SxState {
    pub doc: Doc,
    pub node: NodeId,
    pub iter: Iter,
    pub name: Vec<u8>,
    /// The namespace filter: a URI, or a prefix when `is_prefix`.
    pub ns: Option<Vec<u8>>,
    pub is_prefix: bool,
    pub xpath_ns: Vec<(Vec<u8>, Vec<u8>)>,
    pub cursor: Option<NodeId>,
    pub started: bool,
    /// The class new instances take (a subclass stays a subclass).
    pub class: u32,
}

fn state(o: &Object) -> Result<SxState, Unwind> {
    o.with_payload::<SxState, _>(|s| s.clone())
        .ok_or_else(|| Unwind::error("SimpleXMLElement is not properly initialized"))
}

fn update(o: &Object, f: impl FnOnce(&mut SxState)) {
    o.with_payload::<SxState, _>(f);
}

fn this(o: Option<&Object>) -> Result<&Object, Unwind> {
    o.ok_or_else(|| Unwind::error("Non-static method called statically"))
}

impl SxState {
    fn matches_ns(&self, d: &DocData, id: NodeId) -> bool {
        let n = d.node(id);
        match &self.ns {
            None => n.prefix.is_none(),
            Some(f) if self.is_prefix => n.prefix.as_deref() == Some(f.as_slice()),
            Some(f) => n.ns.as_deref() == Some(f.as_slice()),
        }
    }

    /// The element children this state's filter admits.
    fn element_children(&self, d: &DocData, parent: NodeId) -> Vec<NodeId> {
        d.children(parent)
            .into_iter()
            .filter(|&c| d.node(c).kind == NodeKind::Element && self.matches_ns(d, c))
            .collect()
    }

    fn named_children(&self, d: &DocData, parent: NodeId) -> Vec<NodeId> {
        self.element_children(d, parent)
            .into_iter()
            .filter(|&c| d.node(c).name == self.name)
            .collect()
    }

    fn attributes(&self, d: &DocData, element: NodeId) -> Vec<NodeId> {
        if d.node(element).kind != NodeKind::Element {
            return Vec::new();
        }
        d.node(element)
            .attrs
            .iter()
            .copied()
            .filter(|&a| self.matches_ns(d, a))
            .collect()
    }

    /// The node this state stands for (the first named child for a
    /// `Child` iterator), if any.
    fn target(&self, d: &DocData) -> Option<NodeId> {
        match self.iter {
            Iter::Child => self.named_children(d, self.node).first().copied(),
            _ => Some(self.node),
        }
    }

    /// What `foreach` / `count()` walk.
    fn items(&self, d: &DocData) -> Vec<NodeId> {
        match self.iter {
            Iter::Child => self.named_children(d, self.node),
            Iter::AttrList => self.attributes(d, self.node),
            Iter::None | Iter::Element => {
                if d.node(self.node).kind == NodeKind::Element {
                    self.element_children(d, self.node)
                } else {
                    Vec::new()
                }
            }
        }
    }
}

/// The text php casts a node to: an element's direct text children, an
/// attribute's value.
fn node_text(d: &DocData, id: NodeId) -> Vec<u8> {
    let n = d.node(id);
    match n.kind {
        NodeKind::Attribute => d.attr_value(id),
        NodeKind::Element => {
            let mut out = Vec::new();
            for c in d.children(id) {
                match d.node(c).kind {
                    NodeKind::Text | NodeKind::CData => out.extend_from_slice(&d.node(c).value),
                    NodeKind::EntityRef => out.extend_from_slice(&d.entity_value(&d.node(c).name)),
                    _ => {}
                }
            }
            out
        }
        _ => n.value.clone(),
    }
}

/// A new instance of `class` over `st`.
fn make(ctx: &mut Ctx, st: SxState) -> Result<Object, Unwind> {
    let o = ctx.instantiate(st.class);
    o.set_payload(Payload::Native(Box::new(st)));
    o.set_cast_handler(cast_handler);
    Ok(o)
}

fn child_state(st: &SxState, node: NodeId, iter: Iter, name: &[u8]) -> SxState {
    SxState {
        doc: st.doc.clone(),
        node,
        iter,
        name: name.to_vec(),
        ns: st.ns.clone(),
        is_prefix: st.is_prefix,
        xpath_ns: Vec::new(),
        cursor: None,
        started: false,
        class: st.class,
    }
}

fn cast_handler(o: &Object, target: CastTarget) -> Option<Value> {
    let st = o.with_payload::<SxState, _>(|s| s.clone())?;
    let d = st.doc.borrow();
    let node = st.target(&d);
    Some(match target {
        CastTarget::Bool => Value::Bool(match st.iter {
            Iter::Child => node.is_some(),
            Iter::AttrList => !st.attributes(&d, st.node).is_empty(),
            Iter::Element => !st.element_children(&d, st.node).is_empty(),
            Iter::None => true,
        }),
        CastTarget::Str => Value::string(&node.map(|n| node_text(&d, n)).unwrap_or_default()),
        CastTarget::Int => {
            Value::Int(Value::string(&node.map(|n| node_text(&d, n)).unwrap_or_default()).to_int())
        }
        CastTarget::Float => Value::Float(
            Value::string(&node.map(|n| node_text(&d, n)).unwrap_or_default()).to_float(),
        ),
    })
}

// ---- construction ----------------------------------------------------------

fn parse_into(ctx: &mut Ctx, who: &str, src: &[u8], options: i64) -> Result<Option<Doc>, Unwind> {
    let doc = DocData::new();
    let outcome = {
        let mut d = doc.borrow_mut();
        parser::parse(
            &mut d,
            src,
            &Options {
                substitute_entities: options & libxml::LIBXML_NOENT != 0,
                recover: options & 1 != 0,
                preserve_white_space: true,
                no_blanks: options & libxml::LIBXML_NOBLANKS != 0,
                no_cdata: options & libxml::LIBXML_NOCDATA != 0,
            },
        )
    };
    libxml::report(ctx, who, outcome.errors, options)?;
    if !outcome.ok {
        return Ok(None);
    }
    Ok(Some(doc))
}

fn root_state(doc: Doc, class: u32, ns: Option<Vec<u8>>, is_prefix: bool) -> SxState {
    let root = doc.borrow().document_element().unwrap_or(DOCUMENT);
    SxState {
        doc,
        node: root,
        iter: Iter::None,
        name: Vec::new(),
        ns: ns.filter(|n| !n.is_empty()),
        is_prefix,
        xpath_ns: Vec::new(),
        cursor: None,
        started: false,
        class,
    }
}

fn class_arg(ctx: &mut Ctx, args: &[Value], i: usize, who: &str) -> Result<u32, Unwind> {
    let default = ctx.lookup_class_or_error(b"SimpleXMLElement")?;
    match args.get(i).map(|v| v.deref().into_owned()) {
        None | Some(Value::Null) => Ok(default),
        Some(v) => {
            let name = v.to_php_bytes();
            let Some(cid) = ctx.lookup_class(&name)? else {
                return Err(Unwind::type_error(format!(
                    "{who}(): Argument #{} ($class_name) must be a class name derived from SimpleXMLElement or null, {} given",
                    i + 1,
                    String::from_utf8_lossy(&name)
                )));
            };
            if !ctx.is_subclass_or_eq(cid, default) {
                return Err(Unwind::type_error(format!(
                    "{who}(): Argument #{} ($class_name) must be a class name derived from SimpleXMLElement or null, {} given",
                    i + 1,
                    String::from_utf8_lossy(&name)
                )));
            }
            Ok(cid)
        }
    }
}

fn opt_bytes(args: &[Value], i: usize) -> Option<Vec<u8>> {
    match args.get(i).map(|v| v.deref().into_owned()) {
        None | Some(Value::Null) => None,
        Some(v) => Some(v.to_php_bytes()),
    }
}

/// `simplexml_load_string(string $data, ?string $class_name = SimpleXMLElement::class, int $options = 0, string $namespace_or_prefix = "", bool $is_prefix = false)`
fn load_string(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let src = args[0].deref().to_php_bytes();
    let class = class_arg(ctx, args, 1, "simplexml_load_string")?;
    let options = args.get(2).map_or(0, Value::to_int);
    let Some(doc) = parse_into(ctx, "simplexml_load_string()", &src, options)? else {
        return Ok(Value::Bool(false));
    };
    let st = root_state(
        doc,
        class,
        opt_bytes(args, 3),
        args.get(4).is_some_and(Value::to_bool),
    );
    Ok(Value::Object(make(ctx, st)?))
}

fn load_file(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let path = String::from_utf8_lossy(&args[0].deref().to_php_bytes()).into_owned();
    let class = class_arg(ctx, args, 1, "simplexml_load_file")?;
    let options = args.get(2).map_or(0, Value::to_int);
    let full = if std::path::Path::new(&path).is_absolute() {
        std::path::PathBuf::from(&path)
    } else {
        ctx.cwd.join(&path)
    };
    let Ok(src) = std::fs::read(&full) else {
        ctx.warn(&format!(
            "simplexml_load_file(): I/O warning : failed to load external entity \"{path}\""
        ))?;
        return Ok(Value::Bool(false));
    };
    let Some(doc) = parse_into(ctx, "simplexml_load_file()", &src, options)? else {
        return Ok(Value::Bool(false));
    };
    if let Ok(abs) = std::fs::canonicalize(&full) {
        doc.borrow_mut().document_uri = Some(abs.to_string_lossy().into_owned().into_bytes());
    }
    let st = root_state(
        doc,
        class,
        opt_bytes(args, 3),
        args.get(4).is_some_and(Value::to_bool),
    );
    Ok(Value::Object(make(ctx, st)?))
}

/// `simplexml_import_dom(object $node, ?string $class_name = ...)`
fn import_dom(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let Some(r) = crate::classes::node_arg(&args[0]) else {
        return Err(Unwind::type_error(
            "simplexml_import_dom(): Argument #1 ($node) must be of type SimpleXMLElement|DOMNode",
        ));
    };
    let class = class_arg(ctx, args, 1, "simplexml_import_dom")?;
    let kind = r.doc.borrow().node(r.id).kind;
    let node = match kind {
        NodeKind::Document => match r.doc.borrow().document_element() {
            Some(e) => e,
            None => {
                ctx.warn("simplexml_import_dom(): Invalid Nodetype to import")?;
                return Ok(Value::Null);
            }
        },
        NodeKind::Element | NodeKind::Attribute => r.id,
        _ => {
            ctx.warn("simplexml_import_dom(): Invalid Nodetype to import")?;
            return Ok(Value::Null);
        }
    };
    let st = SxState {
        doc: r.doc.clone(),
        node,
        iter: Iter::None,
        name: Vec::new(),
        ns: None,
        is_prefix: false,
        xpath_ns: Vec::new(),
        cursor: None,
        started: false,
        class,
    };
    Ok(Value::Object(make(ctx, st)?))
}

/// `dom_import_simplexml(object $node): DOMElement|DOMAttr`
fn dom_import(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let Value::Object(o) = args[0].deref().into_owned() else {
        return Err(Unwind::type_error(
            "dom_import_simplexml(): Argument #1 ($node) must be of type SimpleXMLElement",
        ));
    };
    let st = state(&o)?;
    let target = st.target(&st.doc.borrow());
    match target {
        Some(n) => Ok(Value::Object(wrap(ctx, &st.doc, n)?)),
        None => Err(Unwind::error("Invalid Nodetype to import")),
    }
}

/// `new SimpleXMLElement(string $data, int $options = 0, bool $dataIsURL = false, string $namespaceOrPrefix = "", bool $isPrefix = false)`
fn construct(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let data = args[0].deref().to_php_bytes();
    let options = args.get(1).map_or(0, Value::to_int);
    let is_url = args.get(2).is_some_and(Value::to_bool);
    let src = if is_url {
        let path = String::from_utf8_lossy(&data).into_owned();
        match std::fs::read(&path) {
            Ok(s) => s,
            Err(_) => {
                ctx.warn(&format!("SimpleXMLElement::__construct(): I/O warning : failed to load external entity \"{path}\""))?;
                return Err(Unwind::exception(
                    "Exception",
                    "String could not be parsed as XML",
                ));
            }
        }
    } else {
        data
    };
    let Some(doc) = parse_into(ctx, "SimpleXMLElement::__construct()", &src, options)? else {
        return Err(Unwind::exception(
            "Exception",
            "String could not be parsed as XML",
        ));
    };
    let class = ctx.class_of(o).id;
    let st = root_state(
        doc,
        class,
        opt_bytes(args, 3),
        args.get(4).is_some_and(Value::to_bool),
    );
    o.set_payload(Payload::Native(Box::new(st)));
    o.set_cast_handler(cast_handler);
    Ok(Value::Null)
}

// ---- properties ------------------------------------------------------------

fn get_prop(it: &mut Interp, o: &Object, name: &[u8]) -> Option<Result<Value, Unwind>> {
    let st = state(o).ok()?;
    let mut ctx = Ctx(it);
    let target = {
        let d = st.doc.borrow();
        st.target(&d)
    };
    // A child of the resolved element by name; without one, an empty
    // stand-in that reports itself unset.
    let node = target.unwrap_or(st.node);
    let child = SxState {
        doc: st.doc.clone(),
        node,
        iter: Iter::Child,
        name: name.to_vec(),
        ns: st.ns.clone(),
        is_prefix: st.is_prefix,
        xpath_ns: Vec::new(),
        cursor: None,
        started: false,
        class: st.class,
    };
    Some(make(&mut ctx, child).map(Value::Object))
}

fn isset_prop(_: &mut Interp, o: &Object, name: &[u8]) -> Option<bool> {
    let st = state(o).ok()?;
    let d = st.doc.borrow();
    let target = st.target(&d)?;
    let probe = SxState {
        name: name.to_vec(),
        iter: Iter::Child,
        node: target,
        ..st.clone()
    };
    Some(!probe.named_children(&d, target).is_empty())
}

/// Replace an element's content with one text node.
fn set_text(d: &mut DocData, id: NodeId, text: &[u8]) {
    if d.node(id).kind == NodeKind::Attribute {
        d.set_attr_value(id, text);
        return;
    }
    for c in d.children(id) {
        if matches!(
            d.node(c).kind,
            NodeKind::Text | NodeKind::CData | NodeKind::EntityRef
        ) {
            d.unlink(c);
        }
    }
    if !text.is_empty() {
        let t = d.create_text(NodeKind::Text, text);
        d.link_last(id, t);
    }
}

/// The text a php value writes into the tree.
fn write_value(v: &Value) -> Option<Vec<u8>> {
    match v {
        Value::Object(o) => {
            // A SimpleXMLElement assigned back (a nested write's write-back)
            // changes nothing.
            if o.with_payload::<SxState, _>(|_| ()).is_some() {
                return None;
            }
            Some(v.to_php_bytes())
        }
        other => Some(other.to_php_bytes()),
    }
}

fn set_prop(it: &mut Interp, o: &Object, name: &[u8], v: Value) -> Option<Result<(), Unwind>> {
    let st = state(o).ok()?;
    let _ = it;
    let Some(text) = write_value(&v) else {
        return Some(Ok(()));
    };
    let mut d = st.doc.borrow_mut();
    let Some(target) = st.target(&d) else {
        return Some(Ok(()));
    };
    let probe = SxState {
        name: name.to_vec(),
        iter: Iter::Child,
        node: target,
        ..st.clone()
    };
    let existing = probe.named_children(&d, target);
    match existing.first() {
        Some(&c) => set_text(&mut d, c, &text),
        None => {
            let (prefix, local) = split_qname(name);
            let ns = prefix.and_then(|p| d.lookup_ns(target, Some(p)));
            let e = d.create_element(local, prefix, ns.as_deref());
            d.link_last(target, e);
            set_text(&mut d, e, &text);
        }
    }
    Some(Ok(()))
}

fn unset_prop(_: &mut Interp, o: &Object, name: &[u8]) -> Option<Result<(), Unwind>> {
    let st = state(o).ok()?;
    let mut d = st.doc.borrow_mut();
    let Some(target) = st.target(&d) else {
        return Some(Ok(()));
    };
    let probe = SxState {
        name: name.to_vec(),
        iter: Iter::Child,
        node: target,
        ..st.clone()
    };
    for c in probe.named_children(&d, target) {
        d.unlink(c);
    }
    Some(Ok(()))
}

/// php's `sxe_prop_dict`: the property table of an instance, in its
/// `get_properties` (`debug` false) or `get_debug_info` form.
fn table(it: &mut Interp, o: &Object, debug: bool) -> Vec<(ArrayKey, Value)> {
    let Ok(st) = state(o) else {
        return Vec::new();
    };
    let mut ctx = Ctx(it);
    let d = st.doc.borrow();
    let mut out: Vec<(ArrayKey, Value)> = Vec::new();
    let Some(target) = st.target(&d) else {
        return out;
    };
    match d.node(target).kind {
        NodeKind::Attribute => {
            out.push((ArrayKey::Int(0), Value::string(&d.attr_value(target))));
            return out;
        }
        NodeKind::Element => {}
        // A comment (reached as `$x->comment`) has nothing to show.
        _ => return out,
    }
    // Attributes: a `children()` list has none in its property form.
    if debug || st.iter != Iter::Element {
        let attrs = st.attributes(&d, target);
        if !attrs.is_empty() {
            let mut a = Array::new();
            for at in &attrs {
                a.set(
                    ArrayKey::str(&d.node(*at).qualified_name()),
                    Value::string(&d.attr_value(*at)),
                );
            }
            out.push((ArrayKey::str(b"@attributes"), Value::Array(a)));
        }
    }
    if st.iter == Iter::AttrList {
        return out;
    }
    // `$x->name` lists every match by position.
    if st.iter == Iter::Child {
        for (i, m) in st.named_children(&d, st.node).into_iter().enumerate() {
            let leaf = !d
                .children(m)
                .iter()
                .any(|&g| matches!(d.node(g).kind, NodeKind::Element | NodeKind::Comment));
            let text = node_text(&d, m);
            let value = if leaf {
                if text.is_empty() {
                    continue;
                }
                Value::string(&text)
            } else {
                match make(&mut ctx, child_state(&st, m, Iter::None, b"")) {
                    Ok(obj) => Value::Object(obj),
                    Err(_) => continue,
                }
            };
            out.push((ArrayKey::Int(i as i64), value));
        }
        return out;
    }
    let children = st.element_children(&d, target);
    let has_comment = d
        .children(target)
        .iter()
        .any(|&c| d.node(c).kind == NodeKind::Comment);
    if children.is_empty() && !has_comment {
        if st.iter == Iter::None {
            let text = node_text(&d, target);
            if !text.is_empty() {
                out.push((ArrayKey::Int(0), Value::string(&text)));
            }
        }
        return out;
    }
    // Children grouped by name, in first-appearance order; comments under
    // `comment`.
    let mut groups: Vec<(Vec<u8>, Vec<Value>)> = Vec::new();
    for c in d.children(target) {
        let n = d.node(c);
        let (key, value) = match n.kind {
            NodeKind::Element if st.matches_ns(&d, c) => {
                let key = n.qualified_name();
                let leaf = !d
                    .children(c)
                    .iter()
                    .any(|&g| matches!(d.node(g).kind, NodeKind::Element | NodeKind::Comment));
                let text = node_text(&d, c);
                let value = if leaf && !text.is_empty() {
                    Value::string(&text)
                } else {
                    match make(&mut ctx, child_state(&st, c, Iter::None, b"")) {
                        Ok(obj) => Value::Object(obj),
                        Err(_) => continue,
                    }
                };
                (key, value)
            }
            NodeKind::Comment => match make(&mut ctx, child_state(&st, c, Iter::None, b"")) {
                Ok(obj) => (b"comment".to_vec(), Value::Object(obj)),
                Err(_) => continue,
            },
            _ => continue,
        };
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, vs)) => vs.push(value),
            None => groups.push((key, vec![value])),
        }
    }
    for (key, mut vs) in groups {
        if vs.len() == 1 {
            out.push((ArrayKey::str(&key), vs.remove(0)));
        } else {
            let mut a = Array::new();
            for v in vs {
                a.push(v);
            }
            out.push((ArrayKey::str(&key), Value::Array(a)));
        }
    }
    out
}

fn property_table(it: &mut Interp, o: &Object) -> Vec<(ArrayKey, Value)> {
    table(it, o, false)
}

fn debug_table(it: &mut Interp, o: &Object) -> Vec<(ArrayKey, Value)> {
    table(it, o, true)
}

const PROPS: NativeProps = NativeProps {
    names: &[],
    get: get_prop,
    set: set_prop,
    isset: Some(isset_prop),
    unset: Some(unset_prop),
    list: Some(property_table),
    debug: Some(debug_table),
    cast: None,
};

// ---- dimensions ------------------------------------------------------------

fn offset_get(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let st = state(this(o)?)?;
    let key = args[0].deref().into_owned();
    let d = st.doc.borrow();
    match key {
        Value::Int(i) => {
            let items = match st.iter {
                Iter::Child | Iter::AttrList => st.items(&d),
                _ => vec![st.node],
            };
            match usize::try_from(i).ok().and_then(|i| items.get(i).copied()) {
                Some(n) => {
                    let iter = if d.node(n).kind == NodeKind::Attribute {
                        Iter::None
                    } else {
                        Iter::None
                    };
                    drop(d);
                    Ok(Value::Object(make(ctx, child_state(&st, n, iter, b""))?))
                }
                None => Ok(Value::Null),
            }
        }
        other => {
            let name = other.to_php_bytes();
            let Some(target) = st.target(&d) else {
                return Ok(Value::Null);
            };
            let attr = st
                .attributes(&d, target)
                .into_iter()
                .find(|&a| d.node(a).qualified_name() == name);
            match attr {
                Some(a) => {
                    drop(d);
                    Ok(Value::Object(make(
                        ctx,
                        child_state(&st, a, Iter::None, b""),
                    )?))
                }
                None => Ok(Value::Null),
            }
        }
    }
}

fn offset_exists(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let st = state(this(o)?)?;
    let key = args[0].deref().into_owned();
    let d = st.doc.borrow();
    Ok(Value::Bool(match key {
        Value::Int(i) => {
            let items = match st.iter {
                Iter::Child | Iter::AttrList => st.items(&d),
                _ => vec![st.node],
            };
            usize::try_from(i).ok().is_some_and(|i| i < items.len())
        }
        other => {
            let name = other.to_php_bytes();
            st.target(&d).is_some_and(|t| {
                st.attributes(&d, t)
                    .into_iter()
                    .any(|a| d.node(a).qualified_name() == name)
            })
        }
    }))
}

fn offset_set(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let st = state(this(o)?)?;
    let key = args[0].deref().into_owned();
    let Some(text) = write_value(&args[1].deref()) else {
        return Ok(Value::Null);
    };
    let mut d = st.doc.borrow_mut();
    match key {
        Value::Int(i) => {
            let items = match st.iter {
                Iter::Child | Iter::AttrList => st.items(&d),
                _ => vec![st.node],
            };
            if let Some(&n) = usize::try_from(i).ok().and_then(|i| items.get(i)) {
                set_text(&mut d, n, &text);
            } else if st.iter == Iter::Child {
                // `$x->new[] = v` / past the end: append a child of the name.
                let (prefix, local) = split_qname(&st.name);
                let ns = prefix.and_then(|p| d.lookup_ns(st.node, Some(p)));
                let e = d.create_element(local, prefix, ns.as_deref());
                d.link_last(st.node, e);
                set_text(&mut d, e, &text);
            }
        }
        Value::Null if st.iter == Iter::Child => {
            let (prefix, local) = split_qname(&st.name);
            let ns = prefix.and_then(|p| d.lookup_ns(st.node, Some(p)));
            let e = d.create_element(local, prefix, ns.as_deref());
            d.link_last(st.node, e);
            set_text(&mut d, e, &text);
        }
        other => {
            let name = other.to_php_bytes();
            // A missing child (`$x->new['at'] = v`) is created for its attribute.
            let target = match st.target(&d) {
                Some(t) => t,
                None if st.iter == Iter::Child => {
                    let (prefix, local) = split_qname(&st.name);
                    let ns = prefix.and_then(|p| d.lookup_ns(st.node, Some(p)));
                    let e = d.create_element(local, prefix, ns.as_deref());
                    d.link_last(st.node, e);
                    e
                }
                None => return Ok(Value::Null),
            };
            if d.node(target).kind == NodeKind::Element {
                d.set_attr(target, &name, &text);
            }
        }
    }
    Ok(Value::Null)
}

fn offset_unset(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let st = state(this(o)?)?;
    let key = args[0].deref().into_owned();
    let mut d = st.doc.borrow_mut();
    match key {
        Value::Int(i) => {
            let items = match st.iter {
                Iter::Child | Iter::AttrList => st.items(&d),
                _ => vec![st.node],
            };
            if let Some(&n) = usize::try_from(i).ok().and_then(|i| items.get(i)) {
                d.unlink(n);
            }
        }
        other => {
            let name = other.to_php_bytes();
            if let Some(t) = st.target(&d) {
                if let Some(a) = st
                    .attributes(&d, t)
                    .into_iter()
                    .find(|&a| d.node(a).qualified_name() == name)
                {
                    d.unlink(a);
                }
            }
        }
    }
    Ok(Value::Null)
}

// ---- methods ---------------------------------------------------------------

fn to_string(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let st = state(this(o)?)?;
    let d = st.doc.borrow();
    Ok(Value::string(
        &st.target(&d).map(|n| node_text(&d, n)).unwrap_or_default(),
    ))
}

fn count(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let st = state(this(o)?)?;
    let d = st.doc.borrow();
    Ok(Value::Int(st.items(&d).len() as i64))
}

fn get_name(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let st = state(this(o)?)?;
    let d = st.doc.borrow();
    Ok(Value::string(
        &st.target(&d)
            .map(|n| d.node(n).name.clone())
            .unwrap_or_default(),
    ))
}

fn children(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let st = state(this(o)?)?;
    let target = st.target(&st.doc.borrow());
    let Some(target) = target else {
        return Ok(Value::Null);
    };
    let ns = opt_bytes(args, 0);
    let mut c = child_state(&st, target, Iter::Element, b"");
    c.ns = ns;
    c.is_prefix = args.get(1).is_some_and(Value::to_bool);
    Ok(Value::Object(make(ctx, c)?))
}

fn attributes(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let st = state(this(o)?)?;
    let target = st.target(&st.doc.borrow());
    let Some(target) = target else {
        return Ok(Value::Null);
    };
    let mut c = child_state(&st, target, Iter::AttrList, b"");
    c.ns = opt_bytes(args, 0);
    c.is_prefix = args.get(1).is_some_and(Value::to_bool);
    Ok(Value::Object(make(ctx, c)?))
}

fn as_xml(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let st = state(this(o)?)?;
    let d = st.doc.borrow();
    let Some(target) = st.target(&d) else {
        return Ok(Value::Bool(false));
    };
    let out = if d.document_element() == Some(target) {
        serialize::document(&d)
    } else if d.node(target).kind == NodeKind::Attribute {
        let mut v = b" ".to_vec();
        v.extend_from_slice(&d.node(target).qualified_name());
        v.extend_from_slice(b"=\"");
        serialize::escape_attr(&d.attr_value(target), &mut v);
        v.push(b'"');
        v
    } else {
        serialize::fragment(&d, target)
    };
    if let Some(path) = opt_bytes(args, 0) {
        let p = String::from_utf8_lossy(&path).into_owned();
        return match std::fs::write(&p, &out) {
            Ok(()) => Ok(Value::Bool(true)),
            Err(_) => {
                drop(d);
                ctx.warn(&format!(
                    "SimpleXMLElement::asXML(): Unable to open file {p}"
                ))?;
                Ok(Value::Bool(false))
            }
        };
    }
    Ok(Value::string(&out))
}

fn register_xpath_namespace(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let prefix = args[0].deref().to_php_bytes();
    let uri = args[1].deref().to_php_bytes();
    update(this(o)?, |s| {
        s.xpath_ns.retain(|(p, _)| *p != prefix);
        s.xpath_ns.push((prefix, uri));
    });
    Ok(Value::Bool(true))
}

struct SxHost {
    namespaces: Vec<(Vec<u8>, Vec<u8>)>,
}

impl crate::xpath::Host for SxHost {
    fn namespace(&self, prefix: &[u8]) -> Option<Vec<u8>> {
        self.namespaces
            .iter()
            .find(|(p, _)| p == prefix)
            .map(|(_, u)| u.clone())
    }

    fn call(
        &mut self,
        _: &DocData,
        _: Option<&[u8]>,
        name: &[u8],
        _: Vec<crate::xpath::XValue>,
    ) -> Result<crate::xpath::XValue, String> {
        Err(format!(
            "xmlXPathCompOpEval: function {} not found",
            String::from_utf8_lossy(name)
        ))
    }
}

/// `SimpleXMLElement::xpath(string $expression): array|null|false`
fn xpath(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let st = state(this(o)?)?;
    let expr = args[0].deref().to_php_bytes();
    let compiled = match crate::xpath::compile(&expr) {
        Ok(c) => c,
        Err(_) => {
            ctx.warn("SimpleXMLElement::xpath(): Invalid expression")?;
            return Ok(Value::Bool(false));
        }
    };
    let result = {
        let d = st.doc.borrow();
        let Some(context) = st.target(&d) else {
            return Ok(Value::Bool(false));
        };
        // The context node's in-scope declarations are usable by prefix.
        let mut namespaces: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
        let mut cur = Some(context);
        while let Some(n) = cur {
            if d.node(n).kind == NodeKind::Element {
                for dcl in &d.node(n).ns_decls {
                    if let Some(p) = &dcl.prefix {
                        if !namespaces.iter().any(|(q, _)| q == p) {
                            namespaces.push((p.clone(), dcl.uri.clone()));
                        }
                    }
                }
            }
            cur = d.node(n).parent;
        }
        for (p, u) in &st.xpath_ns {
            namespaces.retain(|(q, _)| q != p);
            namespaces.push((p.clone(), u.clone()));
        }
        let mut host = SxHost { namespaces };
        match compiled.check_prefixes(&host) {
            Err(e) => Err(e),
            Ok(()) => {
                let mut eval = crate::xpath::Eval::new(&d, &mut host);
                eval.evaluate(
                    &compiled,
                    &crate::xpath::Context {
                        node: context,
                        position: 0,
                        size: 0,
                        in_predicate: false,
                    },
                )
            }
        }
    };
    match result {
        Ok(crate::xpath::XValue::NodeSet(set)) => {
            let mut out = Array::new();
            for n in set {
                let kind = st.doc.borrow().node(n).kind;
                if !matches!(
                    kind,
                    NodeKind::Element | NodeKind::Attribute | NodeKind::Text | NodeKind::CData
                ) {
                    continue;
                }
                out.push(Value::Object(make(
                    ctx,
                    child_state(&st, n, Iter::None, b""),
                )?));
            }
            Ok(Value::Array(out))
        }
        Ok(_) => Ok(Value::Bool(false)),
        Err(msg) => {
            ctx.warn(&format!("SimpleXMLElement::xpath(): {msg}"))?;
            Ok(Value::Bool(false))
        }
    }
}

fn add_child(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let st = state(this(o)?)?;
    let qname = args[0].deref().to_php_bytes();
    if qname.is_empty() {
        return Err(Unwind::value_error(
            "SimpleXMLElement::addChild(): Argument #1 ($qualifiedName) cannot be empty",
        ));
    }
    let value = opt_bytes(args, 1);
    let ns = opt_bytes(args, 2);
    let (target, id) = {
        let mut d = st.doc.borrow_mut();
        let Some(target) = st.target(&d) else {
            drop(d);
            ctx.warn("SimpleXMLElement::addChild(): Cannot add child. Parent is not a permanent member of the XML tree")?;
            return Ok(Value::Null);
        };
        if d.node(target).kind == NodeKind::Attribute {
            drop(d);
            ctx.warn("SimpleXMLElement::addChild(): Cannot add element to attributes")?;
            return Ok(Value::Null);
        }
        let (prefix, local) = split_qname(&qname);
        let uri = ns
            .clone()
            .or_else(|| prefix.and_then(|p| d.lookup_ns(target, Some(p))))
            .or_else(|| {
                if prefix.is_none() {
                    d.lookup_ns(target, None)
                } else {
                    None
                }
            });
        let e = d.create_element(local, prefix, uri.as_deref());
        if let (Some(u), Some(p)) = (&ns, prefix) {
            if d.lookup_ns(target, Some(p)).as_deref() != Some(u.as_slice()) {
                d.node_mut(e).ns_decls.push(NsDecl {
                    prefix: Some(p.to_vec()),
                    uri: u.clone(),
                });
            }
        } else if let (Some(u), None) = (&ns, prefix) {
            if d.lookup_ns(target, None).as_deref() != Some(u.as_slice()) {
                d.node_mut(e).ns_decls.push(NsDecl {
                    prefix: None,
                    uri: u.clone(),
                });
            }
        }
        d.link_last(target, e);
        (target, e)
    };
    let _ = target;
    if let Some(v) = value {
        // libxml refuses an unescaped `&` that is not an entity reference.
        if let Some(rest) = bad_entity(&v) {
            ctx.warn(&format!(
                "SimpleXMLElement::addChild(): unterminated entity reference {rest}"
            ))?;
        } else {
            let mut d = st.doc.borrow_mut();
            let t = d.create_text(NodeKind::Text, &v);
            d.link_last(id, t);
        }
    }
    Ok(Value::Object(make(
        ctx,
        child_state(&st, id, Iter::None, b""),
    )?))
}

/// The text after a stray `&` (libxml's `xmlStringGetNodeList` failure),
/// `None` when every `&` starts an entity reference.
fn bad_entity(v: &[u8]) -> Option<String> {
    let mut i = 0;
    while i < v.len() {
        if v[i] == b'&' {
            let rest = &v[i + 1..];
            let end = rest.iter().position(|&b| b == b';');
            let ok = end.is_some_and(|e| {
                e > 0
                    && rest[..e]
                        .iter()
                        .all(|b| b.is_ascii_alphanumeric() || *b == b'#')
            });
            if !ok {
                // libxml's `%15s` of what follows the `&`.
                let shown = String::from_utf8_lossy(&v[i + 1..]).into_owned();
                return Some(format!("{shown:>15}"));
            }
        }
        i += 1;
    }
    None
}

fn add_attribute(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let st = state(this(o)?)?;
    let qname = args[0].deref().to_php_bytes();
    if qname.is_empty() {
        return Err(Unwind::value_error(
            "SimpleXMLElement::addAttribute(): Argument #1 ($qualifiedName) cannot be empty",
        ));
    }
    let value = args
        .get(1)
        .map(|v| v.deref().to_php_bytes())
        .unwrap_or_default();
    let ns = opt_bytes(args, 2);
    let mut d = st.doc.borrow_mut();
    let Some(target) = st.target(&d) else {
        return Ok(Value::Null);
    };
    if d.node(target).kind != NodeKind::Element {
        drop(d);
        ctx.warn("SimpleXMLElement::addAttribute(): Unable to locate parent Element")?;
        return Ok(Value::Null);
    }
    let (prefix, local) = split_qname(&qname);
    if d.find_attr(target, &qname).is_some() {
        drop(d);
        ctx.warn("SimpleXMLElement::addAttribute(): Attribute already exists")?;
        return Ok(Value::Null);
    }
    let uri = ns
        .clone()
        .or_else(|| prefix.and_then(|p| d.lookup_ns(target, Some(p))));
    let a = d.create_attr(local, prefix, uri.as_deref(), &value);
    if let (Some(u), Some(p)) = (&ns, prefix) {
        if d.lookup_ns(target, Some(p)).as_deref() != Some(u.as_slice()) {
            d.node_mut(target).ns_decls.push(NsDecl {
                prefix: Some(p.to_vec()),
                uri: u.clone(),
            });
        }
    }
    d.node_mut(a).parent = Some(target);
    d.node_mut(target).attrs.push(a);
    Ok(Value::Null)
}

/// `getNamespaces(bool $recursive = false)`: the namespaces the element
/// (and, recursively, its subtree) *uses*.
fn get_namespaces(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let st = state(this(o)?)?;
    let recursive = args.first().is_some_and(Value::to_bool);
    let d = st.doc.borrow();
    let mut out = Array::new();
    let Some(target) = st.target(&d) else {
        return Ok(Value::Array(out));
    };
    let nodes = if recursive {
        d.descendants(target)
    } else {
        vec![target]
    };
    for n in nodes {
        let node = d.node(n);
        if node.kind != NodeKind::Element {
            continue;
        }
        let mut uses: Vec<(Option<Vec<u8>>, Vec<u8>)> = Vec::new();
        if let Some(u) = &node.ns {
            uses.push((node.prefix.clone(), u.clone()));
        }
        for &a in &node.attrs {
            if let Some(u) = &d.node(a).ns {
                uses.push((d.node(a).prefix.clone(), u.clone()));
            }
        }
        for (p, u) in uses {
            let key = p.unwrap_or_default();
            if out.get(&ArrayKey::str(&key)).is_none() {
                out.set(ArrayKey::str(&key), Value::string(&u));
            }
        }
    }
    Ok(Value::Array(out))
}

/// `getDocNamespaces(bool $recursive = false, bool $fromRoot = true)`: the
/// namespaces *declared*.
fn get_doc_namespaces(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let st = state(this(o)?)?;
    let recursive = args.first().is_some_and(Value::to_bool);
    let from_root = args.get(1).is_none_or(Value::to_bool);
    let d = st.doc.borrow();
    let mut out = Array::new();
    let start = if from_root {
        d.document_element()
    } else {
        st.target(&d)
    };
    let Some(start) = start else {
        return Ok(Value::Array(out));
    };
    let nodes = if recursive {
        d.descendants(start)
    } else {
        vec![start]
    };
    for n in nodes {
        for dcl in &d.node(n).ns_decls {
            let key = dcl.prefix.clone().unwrap_or_default();
            if out.get(&ArrayKey::str(&key)).is_none() {
                out.set(ArrayKey::str(&key), Value::string(&dcl.uri));
            }
        }
    }
    Ok(Value::Array(out))
}

// ---- iteration -------------------------------------------------------------

fn rewind(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let st = state(o)?;
    let first = st.items(&st.doc.borrow()).first().copied();
    update(o, |s| {
        s.cursor = first;
        s.started = true;
    });
    Ok(Value::Null)
}

fn ensure_started(o: &Object) -> Result<SxState, Unwind> {
    let st = state(o)?;
    if !st.started {
        let first = st.items(&st.doc.borrow()).first().copied();
        update(o, |s| {
            s.cursor = first;
            s.started = true;
        });
        return state(o);
    }
    Ok(st)
}

fn valid(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let st = ensure_started(this(o)?)?;
    Ok(Value::Bool(st.cursor.is_some()))
}

fn current(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let st = ensure_started(this(o)?)?;
    match st.cursor {
        Some(n) => Ok(Value::Object(make(
            ctx,
            child_state(&st, n, Iter::None, b""),
        )?)),
        None => Ok(Value::Null),
    }
}

fn key(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let st = ensure_started(this(o)?)?;
    match st.cursor {
        Some(n) => Ok(Value::string(&st.doc.borrow().node(n).name)),
        None => Ok(Value::Bool(false)),
    }
}

fn next(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let st = ensure_started(o)?;
    let next = {
        let d = st.doc.borrow();
        let items = st.items(&d);
        st.cursor
            .and_then(|c| items.iter().position(|&i| i == c))
            .and_then(|p| items.get(p + 1).copied())
    };
    update(o, |s| s.cursor = next);
    Ok(Value::Null)
}

fn has_children(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let st = ensure_started(this(o)?)?;
    let d = st.doc.borrow();
    Ok(Value::Bool(st.cursor.is_some_and(|c| {
        d.node(c).kind == NodeKind::Element
            && d.children(c)
                .iter()
                .any(|&g| d.node(g).kind == NodeKind::Element)
    })))
}

fn get_children(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let st = ensure_started(this(o)?)?;
    match st.cursor {
        Some(n) => Ok(Value::Object(make(
            ctx,
            child_state(&st, n, Iter::Element, b""),
        )?)),
        None => Ok(Value::Null),
    }
}

pub static FUNCTIONS: &[NativeFn] = &[
    nf!("simplexml_load_string", 1, Some(5), load_string),
    nf!("simplexml_load_file", 1, Some(5), load_file),
    nf!("simplexml_import_dom", 1, Some(2), import_dom),
    nf!("dom_import_simplexml", 1, Some(1), dom_import),
];

pub fn register(r: &mut Registry) {
    if r.interp().class_by_name(b"SimpleXMLElement").is_some() {
        return;
    }
    r.functions(FUNCTIONS);
    r.class("SimpleXMLElement")
        .implements(&[
            "Stringable",
            "Countable",
            "RecursiveIterator",
            "ArrayAccess",
        ])
        .native_props(PROPS)
        .method("__construct", nm!(1, Some(5), construct))
        .method("__toString", nm!(0, Some(0), to_string))
        .method("count", nm!(0, Some(0), count))
        .method("getName", nm!(0, Some(0), get_name))
        .method("children", nm!(0, Some(2), children))
        .method("attributes", nm!(0, Some(2), attributes))
        .method("asXML", nm!(0, Some(1), as_xml))
        .method("saveXML", nm!(0, Some(1), as_xml))
        .method("xpath", nm!(1, Some(1), xpath))
        .method(
            "registerXPathNamespace",
            nm!(2, Some(2), register_xpath_namespace),
        )
        .method("addChild", nm!(1, Some(3), add_child))
        .method("addAttribute", nm!(1, Some(3), add_attribute))
        .method("getNamespaces", nm!(0, Some(1), get_namespaces))
        .method("getDocNamespaces", nm!(0, Some(2), get_doc_namespaces))
        .method("offsetGet", nm!(1, Some(1), offset_get))
        .method("offsetExists", nm!(1, Some(1), offset_exists))
        .method("offsetSet", nm!(2, Some(2), offset_set))
        .method("offsetUnset", nm!(1, Some(1), offset_unset))
        .method("rewind", nm!(0, Some(0), rewind))
        .method("valid", nm!(0, Some(0), valid))
        .method("current", nm!(0, Some(0), current))
        .method("key", nm!(0, Some(0), key))
        .method("next", nm!(0, Some(0), next))
        .method("hasChildren", nm!(0, Some(0), has_children))
        .method("getChildren", nm!(0, Some(0), get_children))
        .finish();
    r.class("SimpleXMLIterator")
        .extends("SimpleXMLElement")
        .finish();
}
