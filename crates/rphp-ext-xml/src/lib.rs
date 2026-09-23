//! `ext/xml`, `ext/xmlwriter` and `ext/xmlreader`: the event parser
//! (`xml_parser_create` and its handlers), `XMLWriter` and `XMLReader`.
#![forbid(unsafe_code)]

mod reader;
mod sax;
mod writer;
mod xml;

use rphp_runtime::Registry;

/// Register the extensions.
pub fn register(r: &mut Registry) {
    if r.interp().class_by_name(b"XMLParser").is_some() {
        return;
    }
    r.extension("xml");
    xml::register(r);
    r.extension("xmlwriter");
    writer::register(r);
    r.extension("xmlreader");
    reader::register(r);
}
