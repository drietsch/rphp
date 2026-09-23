//! `ext/xml`, `ext/xmlwriter` and `ext/xmlreader`: the event parser
//! (`xml_parser_create` and its handlers), `XMLWriter` and `XMLReader`.
#![forbid(unsafe_code)]

use rphp_runtime::Registry;

/// Register the extensions.
pub fn register(r: &mut Registry) {
    r.extension("xml");
    r.extension("xmlwriter");
    r.extension("xmlreader");
}
