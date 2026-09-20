//! `ext/dom`, `ext/libxml` and `ext/simplexml` in Rust: an XML parser and
//! serializer with libxml's diagnostics and output rules, a node arena
//! per document, php's `DOM*` class surface, XPath 1.0 (`DOMXPath`) and
//! `SimpleXMLElement` over the same tree.
//!
//! What php decides, this crate decides the same way — measured against
//! php 8.5's `ext/dom` (over libxml2) rather than read off the DOM spec:
//! the `DOMException` codes and texts, which nodes `nodeValue` /
//! `textContent` answer for, when `formatOutput` indents, how attribute
//! values are escaped, what `libxml_get_errors()` carries.
#![forbid(unsafe_code)]

mod classes;
mod html;
mod libxml;
mod parser;
mod serialize;
mod simplexml;
mod tree;
mod xpath;

pub use tree::{DocData, NodeKind};

/// Register the extension: `libxml` first (its constants and error
/// channel), then the DOM classes.
pub fn register(r: &mut rphp_runtime::Registry) {
    if r.interp().class_by_name(b"DOMNode").is_some() {
        return;
    }
    r.extension("libxml");
    r.extension("dom");
    r.extension("SimpleXML");
    r.functions(libxml::FUNCTIONS);
    libxml::register_constants(r);
    libxml::register_classes(r);
    classes::register(r);
    simplexml::register(r);
}
