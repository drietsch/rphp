//! `DOMImplementation`.

use rphp_runtime::{nm, Ctx, NativeResult, Registry, Unwind};
use rphp_value::{Object, Value};

use super::{node_arg, opt_str_arg, str_arg};
use crate::tree::{split_qname, DocData, DocTypeInfo, NodeKind, NsDecl, DOCUMENT};

pub fn new_implementation(ctx: &mut Ctx) -> Result<Object, Unwind> {
    let cid = ctx.lookup_class_or_error(b"DOMImplementation")?;
    Ok(ctx.instantiate(cid))
}

fn has_feature(_: &mut Ctx, _: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Ok(Value::Bool(false))
}

fn create_document_type(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    create_document_type_in(ctx, args, false)
}

/// A doctype node in a holding document (modern → a `Dom\DocumentType`).
pub fn create_document_type_in(ctx: &mut Ctx, args: &[Value], modern: bool) -> NativeResult {
    let name = str_arg(args, 0);
    let doc = super::orphan_doc();
    doc.borrow_mut().modern = modern;
    let id = doc.borrow_mut().create_doctype(
        &name,
        DocTypeInfo {
            public_id: opt_str_arg(args, 1).unwrap_or_default(),
            system_id: opt_str_arg(args, 2).unwrap_or_default(),
            internal_subset: None,
            ..DocTypeInfo::default()
        },
    );
    Ok(Value::Object(super::wrap(ctx, &doc, id)?))
}

fn create_document(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let ns = opt_str_arg(args, 0).filter(|u| !u.is_empty());
    let qname = opt_str_arg(args, 1).unwrap_or_default();
    let doc = DocData::new();
    if let Some(v) = args.get(2).map(|v| v.deref().into_owned()) {
        if let Some(dt) = node_arg(&v) {
            if dt.doc.borrow().node(dt.id).kind == NodeKind::DocumentType {
                let src = dt.doc.borrow();
                let mut d = doc.borrow_mut();
                let copy = d.import(&src, dt.id, true);
                d.link_last(DOCUMENT, copy);
            }
        }
    }
    if !qname.is_empty() {
        let (prefix, local) = split_qname(&qname);
        let mut d = doc.borrow_mut();
        let e = d.create_element(local, prefix, ns.as_deref());
        if let Some(uri) = &ns {
            d.node_mut(e).ns_decls.push(NsDecl {
                prefix: prefix.map(<[u8]>::to_vec),
                uri: uri.clone(),
            });
        }
        d.link_last(DOCUMENT, e);
    }
    Ok(Value::Object(super::document::new_document_object(
        ctx, None, doc,
    )?))
}

pub fn register(r: &mut Registry) {
    r.class("DOMImplementation")
        .method("hasFeature", nm!(2, Some(2), has_feature))
        .method("createDocumentType", nm!(1, Some(3), create_document_type))
        .method("createDocument", nm!(0, Some(3), create_document))
        .finish();
}
