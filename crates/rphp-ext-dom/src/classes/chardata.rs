//! The leaf classes: `DOMCharacterData` and its `DOMText`, `DOMComment`,
//! `DOMCdataSection`; `DOMProcessingInstruction`, `DOMAttr`,
//! `DOMEntityReference`, `DOMDocumentType`, `DOMDocumentFragment`,
//! `DOMEntity`, `DOMNotation`.

use rphp_runtime::{nm, Ctx, NativeResult, Registry, Unwind};
use rphp_value::{Object, Value};

use super::{dom_exception, node_ref, opt_str_arg, orphan_doc, set_node_ref, str_arg, wrap};
use crate::parser::{self, Options};
use crate::tree::{is_valid_name, split_qname, DocData, DomError, NodeKind, NodeRef};

fn s(v: &[u8]) -> Value {
    Value::string(v)
}

/// A detached node of `kind` in a holding document, for the `new DOM…`
/// constructors.
fn construct_leaf(o: &Object, kind: NodeKind, value: &[u8]) {
    let doc = orphan_doc();
    let id = doc.borrow_mut().create_text(kind, value);
    set_node_ref(
        o,
        NodeRef {
            doc: doc.clone(),
            id,
        },
    );
    doc.borrow_mut().remember(id, o);
}

fn text_construct(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    construct_leaf(super::this(o)?, NodeKind::Text, &str_arg(args, 0));
    Ok(Value::Null)
}

fn comment_construct(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    construct_leaf(super::this(o)?, NodeKind::Comment, &str_arg(args, 0));
    Ok(Value::Null)
}

fn cdata_construct(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    construct_leaf(super::this(o)?, NodeKind::CData, &str_arg(args, 0));
    Ok(Value::Null)
}

fn pi_construct(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = super::this(o)?;
    let target = str_arg(args, 0);
    if !is_valid_name(&target) {
        return Err(dom_exception(ctx, DomError::InvalidCharacter));
    }
    let doc = orphan_doc();
    let id = doc
        .borrow_mut()
        .create_pi(&target, &opt_str_arg(args, 1).unwrap_or_default());
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

fn attr_construct(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = super::this(o)?;
    let name = str_arg(args, 0);
    if !is_valid_name(&name) {
        return Err(dom_exception(ctx, DomError::InvalidCharacter));
    }
    let doc = orphan_doc();
    let id = {
        let (prefix, local) = split_qname(&name);
        doc.borrow_mut().create_attr(
            local,
            prefix,
            None,
            &opt_str_arg(args, 1).unwrap_or_default(),
        )
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

fn entity_ref_construct(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = super::this(o)?;
    let name = str_arg(args, 0);
    if !is_valid_name(&name) {
        return Err(dom_exception(ctx, DomError::InvalidCharacter));
    }
    let doc = orphan_doc();
    let id = doc.borrow_mut().create_entity_ref(&name);
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

fn fragment_construct(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = super::this(o)?;
    let doc = orphan_doc();
    let id = doc.borrow_mut().create_fragment();
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

// ---- character data --------------------------------------------------------

/// A character offset/count pair on the node's text, php's
/// `INDEX_SIZE_ERR` when out of range.
fn char_range(
    ctx: &mut Ctx,
    text: &str,
    offset: i64,
    count: Option<i64>,
) -> Result<(usize, usize), Unwind> {
    let len = text.chars().count() as i64;
    if offset < 0 || offset > len || count.is_some_and(|c| c < 0) {
        return Err(dom_exception(ctx, DomError::IndexSize));
    }
    let start = text
        .char_indices()
        .nth(offset as usize)
        .map_or(text.len(), |(i, _)| i);
    let end = match count {
        Some(c) => text
            .char_indices()
            .nth((offset + c).min(len) as usize)
            .map_or(text.len(), |(i, _)| i),
        None => text.len(),
    };
    Ok((start, end))
}

fn text_of(r: &NodeRef) -> String {
    String::from_utf8_lossy(&r.doc.borrow().node(r.id).value).into_owned()
}

pub fn substring_data(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let text = text_of(&r);
    let (a, b) = char_range(ctx, &text, args[0].to_int(), Some(args[1].to_int()))?;
    Ok(s(text[a..b].as_bytes()))
}

pub fn append_data(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    r.doc
        .borrow_mut()
        .node_mut(r.id)
        .value
        .extend_from_slice(&str_arg(args, 0));
    Ok(Value::Bool(true))
}

pub fn insert_data(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let text = text_of(&r);
    let (a, _) = char_range(ctx, &text, args[0].to_int(), None)?;
    let mut v = text.into_bytes();
    let ins = str_arg(args, 1);
    v.splice(a..a, ins);
    r.doc.borrow_mut().node_mut(r.id).value = v;
    Ok(Value::Bool(true))
}

pub fn delete_data(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let text = text_of(&r);
    let (a, b) = char_range(ctx, &text, args[0].to_int(), Some(args[1].to_int()))?;
    let mut v = text.into_bytes();
    v.drain(a..b);
    r.doc.borrow_mut().node_mut(r.id).value = v;
    Ok(Value::Bool(true))
}

pub fn replace_data(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let text = text_of(&r);
    let (a, b) = char_range(ctx, &text, args[0].to_int(), Some(args[1].to_int()))?;
    let mut v = text.into_bytes();
    v.splice(a..b, str_arg(args, 2));
    r.doc.borrow_mut().node_mut(r.id).value = v;
    Ok(Value::Bool(true))
}

pub fn split_text(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let text = text_of(&r);
    let offset = args[0].to_int();
    let len = text.chars().count() as i64;
    if offset < 0 || offset > len {
        return Ok(Value::Bool(false));
    }
    let (a, _) = char_range(ctx, &text, offset, None)?;
    let (head, tail) = text.split_at(a);
    let (head, tail) = (head.as_bytes().to_vec(), tail.as_bytes().to_vec());
    let new = {
        let mut d = r.doc.borrow_mut();
        let kind = d.node(r.id).kind;
        d.node_mut(r.id).value = head;
        let new = d.create_text(kind, &tail);
        if let Some(p) = d.node(r.id).parent {
            let next = d.node(r.id).next;
            match next {
                Some(n) => d.link_before(p, new, n),
                None => d.link_last(p, new),
            }
        }
        new
    };
    Ok(Value::Object(wrap(ctx, &r.doc, new)?))
}

fn is_whitespace_in_element_content(
    _: &mut Ctx,
    o: Option<&Object>,
    _: &mut [Value],
) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let d = r.doc.borrow();
    let n = d.node(r.id);
    let blank = n.value.iter().all(|b| b.is_ascii_whitespace());
    let in_element = n
        .parent
        .is_some_and(|p| d.node(p).kind == NodeKind::Element);
    Ok(Value::Bool(blank && in_element))
}

// ---- attr ------------------------------------------------------------------

pub fn attr_is_id(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let d = r.doc.borrow();
    let n = d.node(r.id);
    Ok(Value::Bool(
        n.is_id || (n.name == b"id" && n.prefix.as_deref() == Some(b"xml")),
    ))
}

// ---- fragment --------------------------------------------------------------

/// `DOMDocumentFragment::appendXML()`: the markup is parsed as the content
/// of a wrapper element and its children moved in.
pub fn append_xml(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(super::this(o)?)?;
    let src = str_arg(args, 0);
    let mut wrapped = b"<r>".to_vec();
    wrapped.extend_from_slice(&src);
    wrapped.extend_from_slice(b"</r>");
    let tmp = DocData::new();
    let outcome = {
        let mut d = tmp.borrow_mut();
        parser::parse(
            &mut d,
            &wrapped,
            &Options {
                substitute_entities: false,
                recover: true,
                preserve_white_space: true,
                no_blanks: false,
                no_cdata: false,
            },
        )
    };
    if !outcome.ok || outcome.errors.iter().any(|e| e.level == 3) {
        crate::libxml::report(ctx, "DOMDocumentFragment::appendXML()", outcome.errors, 0)?;
        return Ok(Value::Bool(false));
    }
    let root = tmp.borrow().document_element();
    if let Some(root) = root {
        let src_doc = tmp.borrow();
        let mut d = r.doc.borrow_mut();
        for c in src_doc.children(root) {
            let copy = d.import(&src_doc, c, true);
            d.link_last(r.id, copy);
        }
    }
    Ok(Value::Bool(true))
}

pub fn register(r: &mut Registry) {
    r.class("DOMCharacterData")
        .extends("DOMNode")
        .implements(&["DOMChildNode"])
        .native_props(super::node::CHARDATA_PROPS)
        .method("substringData", nm!(2, Some(2), substring_data))
        .method("appendData", nm!(1, Some(1), append_data))
        .method("insertData", nm!(2, Some(2), insert_data))
        .method("deleteData", nm!(2, Some(2), delete_data))
        .method("replaceData", nm!(3, Some(3), replace_data))
        .method("before", nm!(0, None, super::element::before))
        .method("after", nm!(0, None, super::element::after))
        .method("remove", nm!(0, Some(0), super::element::remove))
        .method("replaceWith", nm!(0, None, super::element::replace_with))
        .finish();
    r.class("DOMText")
        .extends("DOMCharacterData")
        .native_props(super::node::TEXT_PROPS)
        .method("__construct", nm!(0, Some(1), text_construct))
        .method("splitText", nm!(1, Some(1), split_text))
        .method(
            "isWhitespaceInElementContent",
            nm!(0, Some(0), is_whitespace_in_element_content),
        )
        .method(
            "isElementContentWhitespace",
            nm!(0, Some(0), is_whitespace_in_element_content),
        )
        .finish();
    r.class("DOMComment")
        .extends("DOMCharacterData")
        .method("__construct", nm!(0, Some(1), comment_construct))
        .finish();
    r.class("DOMCdataSection")
        .extends("DOMText")
        .method("__construct", nm!(1, Some(1), cdata_construct))
        .finish();
    r.class("DOMProcessingInstruction")
        .extends("DOMNode")
        .native_props(super::node::PI_PROPS)
        .method("__construct", nm!(1, Some(2), pi_construct))
        .finish();
    r.class("DOMAttr")
        .extends("DOMNode")
        .native_props(super::node::ATTR_PROPS)
        .method("__construct", nm!(1, Some(2), attr_construct))
        .method("isId", nm!(0, Some(0), attr_is_id))
        .finish();
    r.class("DOMEntityReference")
        .extends("DOMNode")
        .method("__construct", nm!(1, Some(1), entity_ref_construct))
        .finish();
    r.class("DOMDocumentType")
        .extends("DOMNode")
        .native_props(super::node::DOCTYPE_PROPS)
        .finish();
    r.class("DOMEntity")
        .extends("DOMNode")
        .native_props(super::node::ENTITY_PROPS)
        .finish();
    r.class("DOMNotation")
        .extends("DOMNode")
        .native_props(super::node::NOTATION_PROPS)
        .finish();
    r.class("DOMDocumentFragment")
        .extends("DOMNode")
        .implements(&["DOMParentNode"])
        .native_props(super::node::FRAGMENT_PROPS)
        .method("__construct", nm!(0, Some(0), fragment_construct))
        .method("appendXML", nm!(1, Some(1), append_xml))
        .method("append", nm!(0, None, super::element::append))
        .method("prepend", nm!(0, None, super::element::prepend))
        .method(
            "replaceChildren",
            nm!(0, None, super::element::replace_children),
        )
        .finish();
}
