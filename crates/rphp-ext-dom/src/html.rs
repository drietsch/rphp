//! `loadHTML()` / `saveHTML()`: libxml's HTML parser in outline — the
//! implied `html`/`head`/`body`, auto-closed `p`/`li`/`option`/table
//! cells, void elements, raw-text `script`/`style`, case-folded names,
//! unquoted attributes, HTML 4 named entities — and its HTML serializer
//! (no self-closing, the document's non-ASCII text as named entities, the
//! default `HTML 4.0 Transitional` doctype).

use rphp_runtime::{Ctx, NativeResult, Unwind};
use rphp_value::{Object, Value};

use crate::classes::{node_ref, set_node_ref};
use crate::libxml;
use crate::parser::XmlError;
use crate::tree::{DocData, DocTypeInfo, NodeId, NodeKind, NodeRef, DOCUMENT};

/// HTML 4.01's named entities: code point → name.
pub static ENTITIES: &[(u32, &str)] = &[
    (34, "quot"),
    (38, "amp"),
    (60, "lt"),
    (62, "gt"),
    (160, "nbsp"),
    (161, "iexcl"),
    (162, "cent"),
    (163, "pound"),
    (164, "curren"),
    (165, "yen"),
    (166, "brvbar"),
    (167, "sect"),
    (168, "uml"),
    (169, "copy"),
    (170, "ordf"),
    (171, "laquo"),
    (172, "not"),
    (173, "shy"),
    (174, "reg"),
    (175, "macr"),
    (176, "deg"),
    (177, "plusmn"),
    (178, "sup2"),
    (179, "sup3"),
    (180, "acute"),
    (181, "micro"),
    (182, "para"),
    (183, "middot"),
    (184, "cedil"),
    (185, "sup1"),
    (186, "ordm"),
    (187, "raquo"),
    (188, "frac14"),
    (189, "frac12"),
    (190, "frac34"),
    (191, "iquest"),
    (192, "Agrave"),
    (193, "Aacute"),
    (194, "Acirc"),
    (195, "Atilde"),
    (196, "Auml"),
    (197, "Aring"),
    (198, "AElig"),
    (199, "Ccedil"),
    (200, "Egrave"),
    (201, "Eacute"),
    (202, "Ecirc"),
    (203, "Euml"),
    (204, "Igrave"),
    (205, "Iacute"),
    (206, "Icirc"),
    (207, "Iuml"),
    (208, "ETH"),
    (209, "Ntilde"),
    (210, "Ograve"),
    (211, "Oacute"),
    (212, "Ocirc"),
    (213, "Otilde"),
    (214, "Ouml"),
    (215, "times"),
    (216, "Oslash"),
    (217, "Ugrave"),
    (218, "Uacute"),
    (219, "Ucirc"),
    (220, "Uuml"),
    (221, "Yacute"),
    (222, "THORN"),
    (223, "szlig"),
    (224, "agrave"),
    (225, "aacute"),
    (226, "acirc"),
    (227, "atilde"),
    (228, "auml"),
    (229, "aring"),
    (230, "aelig"),
    (231, "ccedil"),
    (232, "egrave"),
    (233, "eacute"),
    (234, "ecirc"),
    (235, "euml"),
    (236, "igrave"),
    (237, "iacute"),
    (238, "icirc"),
    (239, "iuml"),
    (240, "eth"),
    (241, "ntilde"),
    (242, "ograve"),
    (243, "oacute"),
    (244, "ocirc"),
    (245, "otilde"),
    (246, "ouml"),
    (247, "divide"),
    (248, "oslash"),
    (249, "ugrave"),
    (250, "uacute"),
    (251, "ucirc"),
    (252, "uuml"),
    (253, "yacute"),
    (254, "thorn"),
    (255, "yuml"),
    (338, "OElig"),
    (339, "oelig"),
    (352, "Scaron"),
    (353, "scaron"),
    (376, "Yuml"),
    (402, "fnof"),
    (710, "circ"),
    (732, "tilde"),
    (913, "Alpha"),
    (914, "Beta"),
    (915, "Gamma"),
    (916, "Delta"),
    (917, "Epsilon"),
    (918, "Zeta"),
    (919, "Eta"),
    (920, "Theta"),
    (921, "Iota"),
    (922, "Kappa"),
    (923, "Lambda"),
    (924, "Mu"),
    (925, "Nu"),
    (926, "Xi"),
    (927, "Omicron"),
    (928, "Pi"),
    (929, "Rho"),
    (931, "Sigma"),
    (932, "Tau"),
    (933, "Upsilon"),
    (934, "Phi"),
    (935, "Chi"),
    (936, "Psi"),
    (937, "Omega"),
    (945, "alpha"),
    (946, "beta"),
    (947, "gamma"),
    (948, "delta"),
    (949, "epsilon"),
    (950, "zeta"),
    (951, "eta"),
    (952, "theta"),
    (953, "iota"),
    (954, "kappa"),
    (955, "lambda"),
    (956, "mu"),
    (957, "nu"),
    (958, "xi"),
    (959, "omicron"),
    (960, "pi"),
    (961, "rho"),
    (962, "sigmaf"),
    (963, "sigma"),
    (964, "tau"),
    (965, "upsilon"),
    (966, "phi"),
    (967, "chi"),
    (968, "psi"),
    (969, "omega"),
    (977, "thetasym"),
    (978, "upsih"),
    (982, "piv"),
    (8194, "ensp"),
    (8195, "emsp"),
    (8201, "thinsp"),
    (8204, "zwnj"),
    (8205, "zwj"),
    (8206, "lrm"),
    (8207, "rlm"),
    (8211, "ndash"),
    (8212, "mdash"),
    (8216, "lsquo"),
    (8217, "rsquo"),
    (8218, "sbquo"),
    (8220, "ldquo"),
    (8221, "rdquo"),
    (8222, "bdquo"),
    (8224, "dagger"),
    (8225, "Dagger"),
    (8226, "bull"),
    (8230, "hellip"),
    (8240, "permil"),
    (8242, "prime"),
    (8243, "Prime"),
    (8249, "lsaquo"),
    (8250, "rsaquo"),
    (8254, "oline"),
    (8260, "frasl"),
    (8364, "euro"),
    (8465, "image"),
    (8472, "weierp"),
    (8476, "real"),
    (8482, "trade"),
    (8501, "alefsym"),
    (8592, "larr"),
    (8593, "uarr"),
    (8594, "rarr"),
    (8595, "darr"),
    (8596, "harr"),
    (8629, "crarr"),
    (8656, "lArr"),
    (8657, "uArr"),
    (8658, "rArr"),
    (8659, "dArr"),
    (8660, "hArr"),
    (8704, "forall"),
    (8706, "part"),
    (8707, "exist"),
    (8709, "empty"),
    (8711, "nabla"),
    (8712, "isin"),
    (8713, "notin"),
    (8715, "ni"),
    (8719, "prod"),
    (8721, "sum"),
    (8722, "minus"),
    (8727, "lowast"),
    (8730, "radic"),
    (8733, "prop"),
    (8734, "infin"),
    (8736, "ang"),
    (8743, "and"),
    (8744, "or"),
    (8745, "cap"),
    (8746, "cup"),
    (8747, "int"),
    (8756, "there4"),
    (8764, "sim"),
    (8773, "cong"),
    (8776, "asymp"),
    (8800, "ne"),
    (8801, "equiv"),
    (8804, "le"),
    (8805, "ge"),
    (8834, "sub"),
    (8835, "sup"),
    (8836, "nsub"),
    (8838, "sube"),
    (8839, "supe"),
    (8853, "oplus"),
    (8855, "otimes"),
    (8869, "perp"),
    (8901, "sdot"),
    (8968, "lceil"),
    (8969, "rceil"),
    (8970, "lfloor"),
    (8971, "rfloor"),
    (9001, "lang"),
    (9002, "rang"),
    (9674, "loz"),
    (9824, "spades"),
    (9827, "clubs"),
    (9829, "hearts"),
    (9830, "diams"),
];

pub fn entity_for(cp: u32) -> Option<&'static str> {
    ENTITIES.iter().find(|(c, _)| *c == cp).map(|(_, n)| *n)
}

pub fn codepoint_for(name: &[u8]) -> Option<u32> {
    match name {
        b"apos" => return Some(39),
        _ => {}
    }
    ENTITIES
        .iter()
        .find(|(_, n)| n.as_bytes() == name)
        .map(|(c, _)| *c)
}

const VOID: &[&[u8]] = &[
    b"area",
    b"base",
    b"basefont",
    b"br",
    b"col",
    b"embed",
    b"frame",
    b"hr",
    b"img",
    b"input",
    b"isindex",
    b"keygen",
    b"link",
    b"meta",
    b"param",
    b"source",
    b"track",
    b"wbr",
];

/// Elements whose start tag closes an open `p`.
const CLOSES_P: &[&[u8]] = &[
    b"address",
    b"article",
    b"aside",
    b"blockquote",
    b"details",
    b"div",
    b"dl",
    b"fieldset",
    b"figcaption",
    b"figure",
    b"footer",
    b"form",
    b"h1",
    b"h2",
    b"h3",
    b"h4",
    b"h5",
    b"h6",
    b"header",
    b"hgroup",
    b"hr",
    b"main",
    b"menu",
    b"nav",
    b"ol",
    b"p",
    b"pre",
    b"section",
    b"table",
    b"ul",
];

const HEAD_ONLY: &[&[u8]] = &[
    b"title",
    b"meta",
    b"link",
    b"style",
    b"base",
    b"script",
    b"noscript",
];

struct Html<'a> {
    src: &'a [u8],
    pos: usize,
    line: i64,
    doc: DocData,
    errors: Vec<XmlError>,
    /// Open elements, innermost last.
    stack: Vec<NodeId>,
    implied: bool,
    body_open: bool,
    head_open: bool,
    /// php 8.4's HTML5 parser rules.
    modern: bool,
    /// Tree-construction errors, reported after the tokenizer's.
    tree_errors: Vec<XmlError>,
    /// Past the initial insertion mode (a doctype or the first token).
    initial_done: bool,
    /// A `head` exists (explicit or implied) — whitespace after it is kept.
    head_seen: bool,
}

pub fn load(ctx: &mut Ctx, o: &Object, who: &str, src: &[u8], options: i64) -> NativeResult {
    let r = node_ref(o)?;
    let (format_output, preserve, node_classes) = {
        let old = r.doc.borrow();
        (
            old.format_output,
            old.preserve_white_space,
            old.node_classes.clone(),
        )
    };
    let mut doc = parse_document(ctx, who, src, options, false)?;
    doc.format_output = format_output;
    doc.preserve_white_space = preserve;
    doc.node_classes = node_classes;
    let fresh = std::rc::Rc::new(std::cell::RefCell::new(doc));
    r.doc.borrow_mut().objects.remove(&DOCUMENT);
    set_node_ref(
        o,
        NodeRef {
            doc: fresh.clone(),
            id: DOCUMENT,
        },
    );
    fresh.borrow_mut().remember(DOCUMENT, o);
    Ok(Value::Bool(true))
}

/// Parse `src` as HTML into a fresh document, reporting the diagnostics
/// php's way (`libxml_use_internal_errors()` aware). `modern` is php
/// 8.4's HTML5 parser in outline: `<?…>` is a bogus comment, no default
/// doctype, no `LIBXML_HTML_NOIMPLIED`.
pub fn parse_document(
    ctx: &mut Ctx,
    who: &str,
    src: &[u8],
    options: i64,
    modern: bool,
) -> Result<DocData, Unwind> {
    let src = src.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(src);
    let no_implied = !modern && options & libxml::LIBXML_HTML_NOIMPLIED != 0;
    let no_dtd = modern || options & libxml::LIBXML_HTML_NODEFDTD != 0;
    let fresh = DocData::new();
    let mut doc = std::rc::Rc::try_unwrap(fresh)
        .ok()
        .expect("fresh document is unshared")
        .into_inner();
    doc.version = None;
    doc.standalone = Some(true);
    doc.is_html = true;
    let mut p = Html {
        src,
        pos: 0,
        line: 1,
        doc,
        errors: Vec::new(),
        stack: Vec::new(),
        implied: !no_implied,
        body_open: false,
        head_open: false,
        modern,
        tree_errors: Vec::new(),
        initial_done: false,
        head_seen: false,
    };
    p.parse();
    let mut doc = p.doc;
    let mut errors = p.errors;
    errors.extend(p.tree_errors);
    if !no_dtd && doc.doctype().is_none() {
        let dt = doc.create_doctype(
            b"html",
            DocTypeInfo {
                public_id: b"-//W3C//DTD HTML 4.0 Transitional//EN".to_vec(),
                system_id: b"http://www.w3.org/TR/REC-html40/loose.dtd".to_vec(),
                internal_subset: None,
                ..DocTypeInfo::default()
            },
        );
        let first = doc.node(DOCUMENT).first_child;
        match first {
            Some(f) => doc.link_before(DOCUMENT, dt, f),
            None => doc.link_last(DOCUMENT, dt),
        }
    }
    if modern {
        // The HTML5 parser always has html/head/body.
        if doc.document_element().is_none() {
            let html = doc.create_element(b"html", None, None);
            doc.link_last(DOCUMENT, html);
        }
        let html = doc.document_element().expect("html element");
        let kids = doc.children(html);
        if !kids.iter().any(|&c| doc.node(c).name == b"head") {
            let head = doc.create_element(b"head", None, None);
            match kids.first() {
                Some(&f) => doc.link_before(html, head, f),
                None => doc.link_last(html, head),
            }
        }
        if !doc
            .children(html)
            .iter()
            .any(|&c| matches!(doc.node(c).name.as_slice(), b"body" | b"frameset"))
        {
            let body = doc.create_element(b"body", None, None);
            doc.link_last(html, body);
        }
    }
    libxml::report(ctx, who, errors, options)?;
    Ok(doc)
}

const SVG_NS: &[u8] = b"http://www.w3.org/2000/svg";
const MATHML_NS: &[u8] = b"http://www.w3.org/1998/Math/MathML";

/// HTML5's case adjustments for SVG element names.
const SVG_ELEMENTS: &[&str] = &[
    "altGlyph",
    "altGlyphDef",
    "altGlyphItem",
    "animateColor",
    "animateMotion",
    "animateTransform",
    "clipPath",
    "feBlend",
    "feColorMatrix",
    "feComponentTransfer",
    "feComposite",
    "feConvolveMatrix",
    "feDiffuseLighting",
    "feDisplacementMap",
    "feDistantLight",
    "feDropShadow",
    "feFlood",
    "feFuncA",
    "feFuncB",
    "feFuncG",
    "feFuncR",
    "feGaussianBlur",
    "feImage",
    "feMerge",
    "feMergeNode",
    "feMorphology",
    "feOffset",
    "fePointLight",
    "feSpecularLighting",
    "feSpotLight",
    "feTile",
    "feTurbulence",
    "foreignObject",
    "glyphRef",
    "linearGradient",
    "radialGradient",
    "textPath",
];

/// HTML5's case adjustments for SVG attribute names.
const SVG_ATTRS: &[&str] = &[
    "attributeName",
    "attributeType",
    "baseFrequency",
    "baseProfile",
    "calcMode",
    "clipPathUnits",
    "diffuseConstant",
    "edgeMode",
    "filterUnits",
    "glyphRef",
    "gradientTransform",
    "gradientUnits",
    "kernelMatrix",
    "kernelUnitLength",
    "keyPoints",
    "keySplines",
    "keyTimes",
    "lengthAdjust",
    "limitingConeAngle",
    "markerHeight",
    "markerUnits",
    "markerWidth",
    "maskContentUnits",
    "maskUnits",
    "numOctaves",
    "pathLength",
    "patternContentUnits",
    "patternTransform",
    "patternUnits",
    "pointsAtX",
    "pointsAtY",
    "pointsAtZ",
    "preserveAlpha",
    "preserveAspectRatio",
    "primitiveUnits",
    "refX",
    "refY",
    "repeatCount",
    "repeatDur",
    "requiredExtensions",
    "requiredFeatures",
    "specularConstant",
    "specularExponent",
    "spreadMethod",
    "startOffset",
    "stdDeviation",
    "stitchTiles",
    "surfaceScale",
    "systemLanguage",
    "tableValues",
    "targetX",
    "targetY",
    "textLength",
    "viewBox",
    "viewTarget",
    "xChannelSelector",
    "yChannelSelector",
    "zoomAndPan",
];

/// The formatting elements the adoption agency reopens.
const FORMATTING: &[&[u8]] = &[
    b"a", b"b", b"big", b"code", b"em", b"font", b"i", b"nobr", b"s", b"small", b"strike",
    b"strong", b"tt", b"u",
];

/// Raw-text elements (no markup, no entities inside).
const RAW: &[&[u8]] = &[
    b"script",
    b"style",
    b"xmp",
    b"iframe",
    b"noembed",
    b"noframes",
];

/// RCDATA elements (no markup, entities decoded) — the HTML5 parser's.
const RCDATA: &[&[u8]] = &[b"textarea", b"title"];

impl<'a> Html<'a> {
    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    fn starts_with(&self, s: &[u8]) -> bool {
        self.src[self.pos..].starts_with(s)
    }

    fn starts_with_ci(&self, s: &[u8]) -> bool {
        self.src.len() >= self.pos + s.len()
            && self.src[self.pos..self.pos + s.len()].eq_ignore_ascii_case(s)
    }

    fn bump(&mut self) -> Option<u8> {
        let b = self.peek()?;
        self.pos += 1;
        if b == b'\n' {
            self.line += 1;
        }
        Some(b)
    }

    fn advance(&mut self, n: usize) {
        for _ in 0..n {
            self.bump();
        }
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
            self.bump();
        }
    }

    /// Line and 1-based column of a byte offset of the source.
    fn pos_of(&self, abs: usize) -> (i64, i64) {
        let abs = abs.min(self.src.len());
        let line = 1 + self.src[..abs].iter().filter(|&&b| b == b'\n').count() as i64;
        let line_start = self.src[..abs]
            .iter()
            .rposition(|&b| b == b'\n')
            .map_or(0, |i| i + 1);
        (line, (abs - line_start) as i64 + 1)
    }

    /// Decode the character references of `src[start..end]`, reporting
    /// what each parser reports.
    fn decode(&mut self, start: usize, end: usize, in_attr: bool) -> Vec<u8> {
        let (text, errors) = decode_entities_in(&self.src[start..end], self.modern, in_attr);
        for (off, e) in errors {
            let (line, col) = self.pos_of(start + off);
            if self.modern {
                let what = match e {
                    RefError::NamedNoSemicolon | RefError::NumericNoSemicolon { .. } => {
                        "missing-semicolon-after-character-reference"
                    }
                    RefError::UnknownName => "unknown-named-character-reference",
                    RefError::Control => "control-character-reference",
                    RefError::Null => "null-character-reference",
                    RefError::Surrogate => "surrogate-character-reference",
                    RefError::NoName | RefError::NoDigits { .. } => continue,
                };
                let msg = format!("tokenizer error {what} in Entity, line: {line}, column: {col}");
                self.errors.push(XmlError {
                    level: 2,
                    code: 1,
                    line,
                    column: col,
                    message: msg,
                });
            } else {
                let (code, msg) = match e {
                    RefError::NoName => (68, "htmlParseEntityRef: no name"),
                    RefError::NamedNoSemicolon => (23, "htmlParseEntityRef: expecting ';'"),
                    RefError::NumericNoSemicolon { hex: true }
                    | RefError::NoDigits { hex: true } => {
                        (6, "htmlParseCharRef: missing semicolon")
                    }
                    RefError::NumericNoSemicolon { hex: false }
                    | RefError::NoDigits { hex: false } => {
                        (7, "htmlParseCharRef: missing semicolon")
                    }
                    _ => continue,
                };
                self.errors.push(XmlError {
                    level: 2,
                    code,
                    line,
                    column: col,
                    message: format!("{msg}\n"),
                });
                if matches!(e, RefError::NoDigits { .. }) {
                    self.errors.push(XmlError {
                        level: 2,
                        code: 9,
                        line,
                        column: col,
                        message: "htmlParseCharRef: invalid xmlChar value 0\n".to_string(),
                    });
                }
            }
        }
        text
    }

    /// The 1-based column of the current position.
    fn column(&self) -> i64 {
        let line_start = self.src[..self.pos]
            .iter()
            .rposition(|&b| b == b'\n')
            .map_or(0, |i| i + 1);
        (self.pos - line_start) as i64
    }

    /// The HTML5 parser's tree-construction errors (reported after the
    /// tokenizer's), at the token's name (`column: 3-9` for a range).
    fn tree_error(&mut self, what: &str, from: i64, to: i64) {
        let range = if to > from {
            format!("{from}-{to}")
        } else {
            from.to_string()
        };
        let msg = format!(
            "tree error {what} in Entity, line: {}, column: {range}",
            self.line
        );
        self.tree_errors.push(XmlError {
            level: 2,
            code: 1,
            line: self.line,
            column: from,
            message: msg,
        });
    }

    /// The HTML5 parser's initial mode: the first token, unless a doctype,
    /// is an error.
    fn leave_initial(&mut self, from: i64, to: i64) {
        if self.modern && !self.initial_done {
            self.initial_done = true;
            self.tree_error("unexpected-token-in-initial-mode", from, to);
        }
    }

    fn current(&self) -> NodeId {
        self.stack.last().copied().unwrap_or(DOCUMENT)
    }

    fn in_scope(&self, name: &[u8]) -> bool {
        self.stack.iter().any(|&id| self.doc.node(id).name == name)
    }

    /// The namespace new content takes here: SVG/MathML inside a foreign
    /// element, HTML again inside an integration point.
    fn foreign_ns(&self) -> Option<&'static [u8]> {
        for &id in self.stack.iter().rev() {
            let n = self.doc.node(id);
            match n.ns.as_deref() {
                Some(ns) if ns == SVG_NS => {
                    return if matches!(n.name.as_slice(), b"foreignObject" | b"desc" | b"title") {
                        None
                    } else {
                        Some(SVG_NS)
                    };
                }
                Some(ns) if ns == MATHML_NS => {
                    return if matches!(
                        n.name.as_slice(),
                        b"mi" | b"mo" | b"mn" | b"ms" | b"mtext" | b"annotation-xml"
                    ) {
                        None
                    } else {
                        Some(MATHML_NS)
                    };
                }
                _ => {}
            }
        }
        None
    }

    /// Make sure `html` (and `body` unless `head_only`) are open.
    fn ensure_body(&mut self, head_only: bool) {
        if !self.implied {
            return;
        }
        if self.stack.is_empty() {
            let html = self.doc.create_element(b"html", None, None);
            self.doc.link_last(DOCUMENT, html);
            self.stack.push(html);
        }
        if head_only {
            if !self.head_open && !self.body_open {
                let head = self.doc.create_element(b"head", None, None);
                let html = self.stack[0];
                self.doc.link_last(html, head);
                self.stack.truncate(1);
                self.stack.push(head);
                self.head_open = true;
                self.head_seen = true;
            }
            return;
        }
        if !self.body_open {
            if self.head_open {
                self.stack.truncate(1);
                self.head_open = false;
            }
            if self.modern && !self.head_seen {
                let head = self.doc.create_element(b"head", None, None);
                let html = self.stack[0];
                self.doc.link_last(html, head);
                self.head_seen = true;
            }
            let body = self.doc.create_element(b"body", None, None);
            let html = self.stack[0];
            self.doc.link_last(html, body);
            self.stack.truncate(1);
            self.stack.push(body);
            self.body_open = true;
        }
    }

    fn parse(&mut self) {
        loop {
            if self.pos >= self.src.len() {
                break;
            }
            if self.starts_with(b"<!--") {
                self.advance(4);
                let start = self.pos;
                let end = find(self.src, self.pos, b"-->").unwrap_or(self.src.len());
                let v = self.src[start..end].to_vec();
                self.advance(end - start + 3.min(self.src.len() - end));
                let c = self.doc.create_text(NodeKind::Comment, &v);
                let parent = self.current();
                self.doc.link_last(parent, c);
                continue;
            }
            if self.starts_with_ci(b"<!doctype") {
                let col = self.column() + 3;
                self.advance(9);
                let end = find(self.src, self.pos, b">").unwrap_or(self.src.len());
                let text = self.src[self.pos..end].to_vec();
                self.advance(end - self.pos + 1.min(self.src.len() - end));
                if self.doc.doctype().is_none() && self.stack.is_empty() {
                    let (mut name, public, system) = parse_doctype(&text);
                    if self.modern {
                        name.make_ascii_lowercase();
                    }
                    // lexbor: a doctype other than `html`, or with a public id.
                    if self.modern && (name != b"html" || !public.is_empty()) {
                        self.tree_error("bad-doctype-token-in-initial-mode", col, col + 6);
                    }
                    let dt = self.doc.create_doctype(
                        &name,
                        DocTypeInfo {
                            public_id: public,
                            system_id: system,
                            ..DocTypeInfo::default()
                        },
                    );
                    self.doc.link_last(DOCUMENT, dt);
                } else if self.modern {
                    self.tree_error("doctype-token-in-body-mode", col, col + 6);
                }
                self.initial_done = true;
                continue;
            }
            if self.starts_with(b"<?") || (self.starts_with(b"<!") && !self.starts_with(b"<!--")) {
                // libxml: a processing instruction; HTML5: a bogus comment.
                let start = self.pos + 1;
                let end = find(self.src, self.pos, b">").unwrap_or(self.src.len());
                let body = self.src[start..end].to_vec();
                let col = self.column() + 2;
                self.advance(end - self.pos + 1.min(self.src.len() - end));
                let parent = self.current();
                if self.modern {
                    if body.first() == Some(&b'?') {
                        let msg = format!("tokenizer error unexpected-question-mark-instead-of-tag-name in Entity, line: {}, column: {}", self.line, col);
                        self.errors.push(XmlError {
                            level: 2,
                            code: 1,
                            line: self.line,
                            column: col,
                            message: msg,
                        });
                    }
                    self.leave_initial(col, col);
                    let c = self.doc.create_text(NodeKind::Comment, &body);
                    self.doc.link_last(parent, c);
                } else if body.first() == Some(&b'?') {
                    // libxml's SGML-style PI: everything up to `>` is data.
                    let inner = &body[1..];
                    let split = inner
                        .iter()
                        .position(|b| b.is_ascii_whitespace())
                        .unwrap_or(inner.len());
                    let (target, data) = (
                        &inner[..split],
                        inner[split..]
                            .iter()
                            .skip_while(|b| b.is_ascii_whitespace())
                            .copied()
                            .collect::<Vec<u8>>(),
                    );
                    let pi = self.doc.create_pi(target, &data);
                    self.doc.link_last(parent, pi);
                }
                continue;
            }
            if self.starts_with(b"</") {
                let col = self.column() + 3;
                self.advance(2);
                let name = self.tag_name().to_ascii_lowercase();
                let end = find(self.src, self.pos, b">").map_or(self.src.len(), |e| e + 1);
                self.advance(end - self.pos);
                let to = col + name.len() as i64 - 1;
                self.leave_initial(col, to);
                self.close_at(&name, col, to);
                continue;
            }
            if self.peek() == Some(b'<')
                && self
                    .src
                    .get(self.pos + 1)
                    .is_some_and(|b| b.is_ascii_alphabetic())
            {
                let col = self.column() + 2;
                self.bump();
                let name = self.tag_name().to_ascii_lowercase();
                self.leave_initial(col, col + name.len() as i64 - 1);
                self.start_tag(name);
                continue;
            }
            // Text.
            let start = self.pos;
            let col = self.column() + 1;
            while let Some(b) = self.peek() {
                if b == b'<'
                    && (self.src.get(self.pos + 1).is_some_and(|c| {
                        c.is_ascii_alphabetic() || *c == b'/' || *c == b'!' || *c == b'?'
                    }))
                {
                    break;
                }
                self.bump();
            }
            let raw = self.src[start..self.pos].to_vec();
            let blank = raw.iter().all(|b| b.is_ascii_whitespace());
            if blank {
                if self.modern {
                    // Ignored before the head; kept inside the head, and
                    // between the head and the body (on `html`).
                    if self.stack.is_empty() || (self.stack.len() == 1 && !self.head_seen) {
                        continue;
                    }
                } else if self.stack.is_empty() || (self.head_open && !self.body_open) {
                    continue;
                }
            }
            let end = self.pos;
            let text = self.decode(start, end, false);
            if text.is_empty() {
                continue;
            }
            if !blank {
                self.leave_initial(col, col);
                self.ensure_body(false);
            }
            let parent = self.current();
            if let Some(last) = self
                .doc
                .node(parent)
                .last_child
                .filter(|&l| self.doc.node(l).kind == NodeKind::Text)
            {
                self.doc.node_mut(last).value.extend_from_slice(&text);
            } else {
                let t = self.doc.create_text(NodeKind::Text, &text);
                self.doc.link_last(parent, t);
            }
        }
        while let Some(&top) = self.stack.last() {
            let _ = top;
            self.stack.pop();
        }
    }

    fn tag_name(&mut self) -> Vec<u8> {
        let start = self.pos;
        while let Some(b) = self.peek() {
            if b.is_ascii_whitespace() || b == b'>' || b == b'/' {
                break;
            }
            self.bump();
        }
        self.src[start..self.pos].to_vec()
    }

    fn start_tag(&mut self, name: Vec<u8>) {
        // Attributes.
        let mut attrs: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
        let mut self_closed = false;
        loop {
            self.skip_ws();
            match self.peek() {
                None => break,
                Some(b'>') => {
                    self.bump();
                    break;
                }
                Some(b'/') => {
                    self.bump();
                    if self.peek() == Some(b'>') {
                        self.bump();
                        self_closed = true;
                        break;
                    }
                    continue;
                }
                _ => {}
            }
            let start = self.pos;
            while let Some(b) = self.peek() {
                if b.is_ascii_whitespace()
                    || b == b'='
                    || b == b'>'
                    || (b == b'/' && self.src.get(self.pos + 1) == Some(&b'>'))
                {
                    break;
                }
                self.bump();
            }
            if self.pos == start {
                self.bump();
                continue;
            }
            let aname = self.src[start..self.pos].to_ascii_lowercase();
            self.skip_ws();
            let value = if self.peek() == Some(b'=') {
                self.bump();
                self.skip_ws();
                match self.peek() {
                    Some(q @ (b'"' | b'\'')) => {
                        self.bump();
                        let vs = self.pos;
                        while let Some(b) = self.peek() {
                            if b == q {
                                break;
                            }
                            self.bump();
                        }
                        let ve = self.pos;
                        self.bump();
                        self.decode(vs, ve, true)
                    }
                    _ => {
                        let vs = self.pos;
                        while let Some(b) = self.peek() {
                            if b.is_ascii_whitespace() || b == b'>' {
                                break;
                            }
                            self.bump();
                        }
                        let ve = self.pos;
                        self.decode(vs, ve, true)
                    }
                }
            } else {
                Vec::new()
            };
            if !attrs.iter().any(|(n, _)| *n == aname) {
                attrs.push((aname, value));
            }
        }
        let foreign = if self.modern { self.foreign_ns() } else { None };
        match name.as_slice() {
            b"html" if self.implied && foreign.is_none() => {
                if self.stack.is_empty() {
                    let html = self.doc.create_element(b"html", None, None);
                    self.doc.link_last(DOCUMENT, html);
                    self.stack.push(html);
                }
                let html = self.stack[0];
                for (k, v) in attrs {
                    if self.doc.find_attr(html, &k).is_none() {
                        self.doc.set_attr(html, &k, &v);
                    }
                }
                return;
            }
            b"head" if self.implied && foreign.is_none() => {
                self.ensure_body(true);
                self.head_seen = true;
                let head = self.current();
                for (k, v) in attrs {
                    self.doc.set_attr(head, &k, &v);
                }
                return;
            }
            b"body" if self.implied && foreign.is_none() => {
                self.ensure_body(false);
                let body = self.current();
                for (k, v) in attrs {
                    if self.doc.find_attr(body, &k).is_none() {
                        self.doc.set_attr(body, &k, &v);
                    }
                }
                return;
            }
            _ => {}
        }
        if self.implied {
            let head_only = HEAD_ONLY.contains(&name.as_slice()) && !self.body_open;
            self.ensure_body(head_only);
            if head_only {
                self.head_seen = true;
            }
        }
        if foreign.is_none() {
            // Auto-closing.
            if CLOSES_P.contains(&name.as_slice()) && self.in_scope(b"p") {
                self.close(b"p");
            }
            match name.as_slice() {
                b"li" => self.close_if_open(b"li", &[b"ul", b"ol", b"menu"]),
                b"dt" | b"dd" => {
                    self.close_if_open(b"dt", &[b"dl"]);
                    self.close_if_open(b"dd", &[b"dl"]);
                }
                b"option" => self.close_if_open(b"option", &[b"select", b"datalist", b"optgroup"]),
                b"tr" => {
                    self.close_if_open(b"td", &[b"table"]);
                    self.close_if_open(b"th", &[b"table"]);
                    self.close_if_open(b"tr", &[b"table"]);
                    if self.modern && self.doc.node(self.current()).name == b"table" {
                        self.open_implied(b"tbody");
                    }
                }
                b"td" | b"th" => {
                    self.close_if_open(b"td", &[b"tr", b"table"]);
                    self.close_if_open(b"th", &[b"tr", b"table"]);
                    if self.modern {
                        if self.doc.node(self.current()).name == b"table" {
                            self.open_implied(b"tbody");
                        }
                        if matches!(
                            self.doc.node(self.current()).name.as_slice(),
                            b"tbody" | b"thead" | b"tfoot"
                        ) {
                            self.open_implied(b"tr");
                        }
                    }
                }
                b"a" if self.modern => self.close_if_open(b"a", &[b"body"]),
                b"h1" | b"h2" | b"h3" | b"h4" | b"h5" | b"h6" if self.modern => {
                    if let Some(&top) = self.stack.last() {
                        if matches!(
                            self.doc.node(top).name.as_slice(),
                            b"h1" | b"h2" | b"h3" | b"h4" | b"h5" | b"h6"
                        ) {
                            self.stack.pop();
                        }
                    }
                }
                _ => {}
            }
        }
        let (local, ns): (Vec<u8>, Option<&[u8]>) = match (foreign, name.as_slice()) {
            (None, b"svg") if self.modern => (name.clone(), Some(SVG_NS)),
            (None, b"math") if self.modern => (name.clone(), Some(MATHML_NS)),
            (Some(ns), _) if ns == SVG_NS => (
                SVG_ELEMENTS
                    .iter()
                    .find(|e| e.eq_ignore_ascii_case(std::str::from_utf8(&name).unwrap_or("")))
                    .map_or(name.clone(), |e| e.as_bytes().to_vec()),
                Some(ns),
            ),
            (Some(ns), _) => (name.clone(), Some(ns)),
            (None, _) => (name.clone(), None),
        };
        let id = self.doc.create_element(&local, None, ns);
        self.doc.node_mut(id).line = self.line as u32;
        for (k, v) in attrs {
            let k = if ns == Some(SVG_NS) {
                SVG_ATTRS
                    .iter()
                    .find(|a| a.eq_ignore_ascii_case(std::str::from_utf8(&k).unwrap_or("")))
                    .map_or(k, |a| a.as_bytes().to_vec())
            } else {
                k
            };
            let a = self.doc.create_attr(&k, None, None, &v);
            self.doc.node_mut(a).parent = Some(id);
            self.doc.node_mut(id).attrs.push(a);
        }
        let parent = self.current();
        self.doc.link_last(parent, id);
        if ns.is_none() && VOID.contains(&name.as_slice()) {
            return;
        }
        if self_closed && (ns.is_some() || !self.modern) {
            return;
        }
        let raw_kind = ns.is_none()
            && if self.modern {
                RAW.contains(&name.as_slice()) || RCDATA.contains(&name.as_slice())
            } else {
                matches!(name.as_slice(), b"script" | b"style")
            };
        if raw_kind {
            // Raw text up to the matching end tag.
            let close = [b"</".as_slice(), &name].concat();
            let end = find_ci(self.src, self.pos, &close).unwrap_or(self.src.len());
            let mut rs = self.pos;
            self.advance(end - self.pos);
            // The end tag itself.
            if self.pos < self.src.len() {
                let tag_end = find(self.src, self.pos, b">").map_or(self.src.len(), |e| e + 1);
                self.advance(tag_end - self.pos);
            }
            if self.modern && name == b"textarea" {
                if self.src[rs..end].starts_with(b"\r\n") {
                    rs += 2;
                } else if self.src[rs..end].starts_with(b"\n") {
                    rs += 1;
                }
            }
            let text = if self.modern && RCDATA.contains(&name.as_slice()) {
                self.decode(rs, end, false)
            } else {
                self.src[rs..end].to_vec()
            };
            if !text.is_empty() {
                let t = self.doc.create_text(NodeKind::Text, &text);
                self.doc.link_last(id, t);
            }
            return;
        }
        if self.modern && ns.is_none() && name == b"plaintext" {
            let raw = self.src[self.pos..].to_vec();
            self.advance(raw.len());
            if !raw.is_empty() {
                let t = self.doc.create_text(NodeKind::Text, &raw);
                self.doc.link_last(id, t);
            }
            return;
        }
        if self.modern && ns.is_none() && matches!(name.as_slice(), b"pre" | b"listing") {
            if self.starts_with(b"\r\n") {
                self.advance(2);
            } else if self.starts_with(b"\n") {
                self.advance(1);
            }
        }
        self.stack.push(id);
    }

    /// Open an implied element (`tbody`, `tr`) under the current one.
    fn open_implied(&mut self, name: &[u8]) {
        let id = self.doc.create_element(name, None, None);
        let parent = self.current();
        self.doc.link_last(parent, id);
        self.stack.push(id);
    }

    /// Close an open `name` unless a `stop` element is nearer.
    fn close_if_open(&mut self, name: &[u8], stop: &[&[u8]]) {
        for &id in self.stack.iter().rev() {
            let n = self.doc.node(id).name.clone();
            if n == name {
                self.close(name);
                return;
            }
            if stop.contains(&n.as_slice()) {
                return;
            }
        }
    }

    fn close(&mut self, name: &[u8]) {
        self.close_at(name, 0, 0);
    }

    /// Elements the HTML5 parser closes silently under an end tag.
    const IMPLIED_END: &'static [&'static [u8]] = &[
        b"dd",
        b"dt",
        b"li",
        b"optgroup",
        b"option",
        b"p",
        b"rb",
        b"rp",
        b"rt",
        b"rtc",
        b"caption",
        b"colgroup",
        b"tbody",
        b"td",
        b"tfoot",
        b"th",
        b"thead",
        b"tr",
    ];

    /// An end tag at `from..to` (its name's columns; 0 = no diagnostics).
    fn close_at(&mut self, name: &[u8], from: i64, to: i64) {
        if name == b"br" {
            // `</br>` is a `<br>` to both parsers.
            if self.modern {
                self.ensure_body(false);
                let br = self.doc.create_element(b"br", None, None);
                let parent = self.current();
                self.doc.link_last(parent, br);
            }
            return;
        }
        if let Some(pos) = self
            .stack
            .iter()
            .rposition(|&id| self.doc.node(id).name.eq_ignore_ascii_case(name))
        {
            if name == b"body" || name == b"html" {
                // Stay open: trailing content still lands in the body.
                return;
            }
            // The adoption agency in outline: formatting elements closed
            // implicitly are reopened after the closed one.
            let reopen: Vec<NodeId> = if self.modern {
                self.stack[pos + 1..]
                    .iter()
                    .copied()
                    .filter(|&id| FORMATTING.contains(&self.doc.node(id).name.as_slice()))
                    .collect()
            } else {
                Vec::new()
            };
            // A formatting element's end tag complains unless it is the
            // current node (the adoption agency); any other unless only
            // optional-end-tag elements sit above it.
            let complain = if FORMATTING.contains(&name) {
                pos + 1 < self.stack.len()
            } else {
                self.stack[pos + 1..]
                    .iter()
                    .any(|&id| !Self::IMPLIED_END.contains(&self.doc.node(id).name.as_slice()))
            };
            if self.modern && from > 0 && complain {
                self.tree_error("unexpected-element-in-open-elements-stack", from, to);
            }
            self.stack.truncate(pos);
            for src in reopen {
                let copy = self.doc.clone_node(src, false);
                let parent = self.current();
                self.doc.link_last(parent, copy);
                self.stack.push(copy);
            }
        } else if self.modern {
            if from > 0 && !FORMATTING.contains(&name) {
                self.tree_error("unexpected-closed-token", from, to);
            }
            if name == b"p" {
                // `</p>` with no open `p` makes an empty one.
                self.ensure_body(false);
                let p = self.doc.create_element(b"p", None, None);
                let parent = self.current();
                self.doc.link_last(parent, p);
            }
        } else {
            let msg = format!("Unexpected end tag : {}\n", String::from_utf8_lossy(name));
            let col = self.column() + 1;
            self.errors.push(XmlError {
                level: 2,
                code: 76,
                line: self.line,
                column: col,
                message: msg,
            });
        }
        if name == b"head" {
            self.head_open = false;
        }
    }
}

fn find(h: &[u8], from: usize, n: &[u8]) -> Option<usize> {
    (from..=h.len().saturating_sub(n.len())).find(|&i| h[i..].starts_with(n))
}

fn find_ci(h: &[u8], from: usize, n: &[u8]) -> Option<usize> {
    (from..=h.len().saturating_sub(n.len())).find(|&i| h[i..i + n.len()].eq_ignore_ascii_case(n))
}

/// The pieces of `<!DOCTYPE …>`: name, public id, system id.
fn parse_doctype(text: &[u8]) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let mut i = 0;
    let ws = |b: u8| b.is_ascii_whitespace();
    while i < text.len() && ws(text[i]) {
        i += 1;
    }
    let ns = i;
    while i < text.len() && !ws(text[i]) {
        i += 1;
    }
    let name = text[ns..i].to_vec();
    let mut quoted = Vec::new();
    while i < text.len() {
        match text[i] {
            q @ (b'"' | b'\'') => {
                let end = text[i + 1..]
                    .iter()
                    .position(|&b| b == q)
                    .map_or(text.len(), |e| i + 1 + e);
                quoted.push(text[i + 1..end].to_vec());
                i = end + 1;
            }
            _ => i += 1,
        }
    }
    let rest = String::from_utf8_lossy(&text[ns..]).to_ascii_uppercase();
    let (public, system) = if rest.contains("PUBLIC") {
        (
            quoted.first().cloned().unwrap_or_default(),
            quoted.get(1).cloned().unwrap_or_default(),
        )
    } else if rest.contains("SYSTEM") {
        (Vec::new(), quoted.first().cloned().unwrap_or_default())
    } else {
        (Vec::new(), Vec::new())
    };
    (name, public, system)
}

/// Windows-1252's C1 range, as the HTML5 parser maps `&#128;`–`&#159;`.
const C1_REMAP: [u32; 32] = [
    0x20AC, 0x81, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039,
    0x0152, 0x8D, 0x017D, 0x8F, 0x90, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0x9D, 0x017E, 0x0178,
];

/// What the decoder found wrong with a character reference, at a byte
/// offset of the input: the parsers report these differently.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum RefError {
    /// `&` with no name after it (libxml only).
    NoName,
    /// A named reference without its `;`.
    NamedNoSemicolon,
    /// A `;`-terminated name that is no entity (HTML5's ambiguous
    /// ampersand; lexbor reports it since php 8.5.10).
    UnknownName,
    /// A numeric reference without its `;` (`hex` tells which form).
    NumericNoSemicolon {
        hex: bool,
    },
    /// `&#` with no digits.
    NoDigits {
        hex: bool,
    },
    Control,
    Null,
    Surrogate,
}

/// Character references in text or attribute values. libxml decodes a
/// named reference only with its `;` (keeping `&name` literal otherwise);
/// the HTML5 parser also takes the legacy names without one (not in an
/// attribute where `=` or a letter follows), and remaps the C1 range.
/// The errors come back with the offset they were found at.
pub fn decode_entities_in(
    raw: &[u8],
    modern: bool,
    in_attr: bool,
) -> (Vec<u8>, Vec<(usize, RefError)>) {
    let mut out = Vec::with_capacity(raw.len());
    let mut errors = Vec::new();
    let mut i = 0;
    while i < raw.len() {
        if raw[i] != b'&' {
            out.push(raw[i]);
            i += 1;
            continue;
        }
        let amp = i;
        if raw.get(i + 1) == Some(&b'#') {
            let (hex, start) = if matches!(raw.get(i + 2), Some(b'x' | b'X')) {
                (true, i + 3)
            } else {
                (false, i + 2)
            };
            let mut j = start;
            while j < raw.len()
                && (if hex {
                    raw[j].is_ascii_hexdigit()
                } else {
                    raw[j].is_ascii_digit()
                })
            {
                j += 1;
            }
            if j == start {
                if modern {
                    // HTML5: `&#` with no digits is text.
                    out.extend_from_slice(&raw[amp..start.min(raw.len())]);
                } else {
                    errors.push((start, RefError::NoDigits { hex }));
                }
                i = start.min(raw.len());
                continue;
            }
            let digits = std::str::from_utf8(&raw[start..j]).unwrap_or("0");
            let mut cp = u32::from_str_radix(digits, if hex { 16 } else { 10 }).unwrap_or(0x110000);
            let terminated = raw.get(j) == Some(&b';');
            if !terminated {
                errors.push((j, RefError::NumericNoSemicolon { hex }));
            }
            if modern {
                if (0x80..=0x9F).contains(&cp) {
                    errors.push((amp + 1, RefError::Control));
                    cp = C1_REMAP[(cp - 0x80) as usize];
                } else if cp == 0 {
                    errors.push((amp + 1, RefError::Null));
                } else if (0xD800..=0xDFFF).contains(&cp) {
                    errors.push((amp + 1, RefError::Surrogate));
                }
            }
            if cp == 0 || cp > 0x10FFFF || (0xD800..=0xDFFF).contains(&cp) {
                cp = 0xFFFD;
            }
            let ch = char::from_u32(cp).unwrap_or('\u{FFFD}');
            let mut buf = [0u8; 4];
            out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
            i = if terminated { j + 1 } else { j };
            continue;
        }
        let mut j = i + 1;
        while j < raw.len() && raw[j].is_ascii_alphanumeric() {
            j += 1;
        }
        if j == i + 1 {
            if !modern {
                errors.push((amp + 1, RefError::NoName));
            }
            out.push(b'&');
            i += 1;
            continue;
        }
        let terminated = raw.get(j) == Some(&b';');
        if modern {
            // The longest legacy name that is a prefix of what follows.
            let mut matched = None;
            if terminated {
                if let Some(cp) = codepoint_for(&raw[i + 1..j]) {
                    matched = Some((cp, j + 1));
                }
            }
            if matched.is_none() {
                let mut k = j;
                while k > i + 1 {
                    if let Some(cp) = codepoint_for(&raw[i + 1..k]) {
                        let next = raw.get(k);
                        if in_attr && next.is_some_and(|&b| b == b'=' || b.is_ascii_alphanumeric())
                        {
                            break;
                        }
                        errors.push((k, RefError::NamedNoSemicolon));
                        matched = Some((cp, k));
                        break;
                    }
                    k -= 1;
                }
                if matched.is_none() && terminated {
                    errors.push((j, RefError::UnknownName));
                }
            }
            match matched {
                Some((cp, end)) => {
                    let ch = char::from_u32(cp).unwrap_or('\u{FFFD}');
                    let mut buf = [0u8; 4];
                    out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                    i = end;
                }
                None => {
                    out.push(b'&');
                    i += 1;
                }
            }
            continue;
        }
        if !terminated {
            errors.push((j, RefError::NamedNoSemicolon));
            out.extend_from_slice(&raw[amp..j]);
            i = j;
            continue;
        }
        match codepoint_for(&raw[i + 1..j]) {
            Some(cp) => {
                let ch = char::from_u32(cp).unwrap_or('\u{FFFD}');
                let mut buf = [0u8; 4];
                out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                i = j + 1;
            }
            None => {
                out.extend_from_slice(&raw[amp..=j]);
                i = j + 1;
            }
        }
    }
    (out, errors)
}

// ---- serialization ---------------------------------------------------------

/// `saveHTML()`: the document (doctype line, root, each on its line) or one
/// node. Non-ASCII text goes out as named entities for the document, raw
/// for a node, as libxml's two output paths do.
pub fn save(doc: &DocData, node: Option<NodeId>) -> Vec<u8> {
    let mut out = Vec::new();
    match node {
        // libxml's HTML serializer prints nothing for a doctype on its own.
        Some(id) if doc.node(id).kind == NodeKind::DocumentType => {}
        Some(id) => html_node(doc, id, false, &mut out),
        None => {
            for c in doc.children(DOCUMENT) {
                html_node(doc, c, true, &mut out);
                out.push(b'\n');
            }
        }
    }
    out
}

const RAW_TEXT: &[&[u8]] = &[b"script", b"style"];

fn html_node(doc: &DocData, id: NodeId, entities: bool, out: &mut Vec<u8>) {
    let n = doc.node(id);
    match n.kind {
        NodeKind::Element => {
            let name = n.qualified_name();
            out.push(b'<');
            out.extend_from_slice(&name);
            for &a in &n.attrs {
                out.push(b' ');
                out.extend_from_slice(&doc.node(a).qualified_name());
                let v = doc.attr_value(a);
                out.extend_from_slice(b"=\"");
                html_escape(&v, true, entities, out);
                out.push(b'"');
            }
            out.push(b'>');
            if VOID.contains(&name.as_slice()) {
                return;
            }
            let raw = RAW_TEXT.contains(&name.as_slice());
            for c in doc.children(id) {
                if raw && doc.node(c).kind == NodeKind::Text {
                    out.extend_from_slice(&doc.node(c).value);
                } else {
                    html_node(doc, c, entities, out);
                }
            }
            out.extend_from_slice(b"</");
            out.extend_from_slice(&name);
            out.push(b'>');
        }
        NodeKind::Text => html_escape(&n.value, false, entities, out),
        NodeKind::CData => out.extend_from_slice(&n.value),
        NodeKind::Comment => {
            out.extend_from_slice(b"<!--");
            out.extend_from_slice(&n.value);
            out.extend_from_slice(b"-->");
        }
        NodeKind::Pi => {
            out.extend_from_slice(b"<?");
            out.extend_from_slice(&n.name);
            if !n.value.is_empty() {
                out.push(b' ');
                out.extend_from_slice(&n.value);
            }
            out.push(b'>');
        }
        NodeKind::DocumentType => {
            out.extend_from_slice(b"<!DOCTYPE ");
            out.extend_from_slice(&n.name);
            if let Some(info) = &n.doctype {
                if !info.public_id.is_empty() {
                    out.extend_from_slice(b" PUBLIC \"");
                    out.extend_from_slice(&info.public_id);
                    out.push(b'"');
                    if !info.system_id.is_empty() {
                        out.extend_from_slice(b" \"");
                        out.extend_from_slice(&info.system_id);
                        out.push(b'"');
                    }
                } else if !info.system_id.is_empty() {
                    out.extend_from_slice(b" SYSTEM \"");
                    out.extend_from_slice(&info.system_id);
                    out.push(b'"');
                }
            }
            out.push(b'>');
        }
        NodeKind::EntityRef => {
            out.push(b'&');
            out.extend_from_slice(&n.name);
            out.push(b';');
        }
        NodeKind::Document | NodeKind::Fragment => {
            for c in doc.children(id) {
                html_node(doc, c, entities, out);
            }
        }
        NodeKind::Attribute => out.extend_from_slice(&doc.attr_value(id)),
        NodeKind::Entity | NodeKind::Notation => {}
    }
}

fn html_escape(v: &[u8], in_attr: bool, entities: bool, out: &mut Vec<u8>) {
    let s = String::from_utf8_lossy(v);
    for c in s.chars() {
        match c {
            '&' => out.extend_from_slice(b"&amp;"),
            '<' if !in_attr => out.extend_from_slice(b"&lt;"),
            '>' if !in_attr => out.extend_from_slice(b"&gt;"),
            '"' if in_attr => out.extend_from_slice(b"&quot;"),
            c if entities && (c as u32) >= 0x80 => match entity_for(c as u32) {
                Some(name) => {
                    out.push(b'&');
                    out.extend_from_slice(name.as_bytes());
                    out.push(b';');
                }
                None => out.extend_from_slice(format!("&#{};", c as u32).as_bytes()),
            },
            c => {
                let mut buf = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            }
        }
    }
}

// ---- php 8.4's HTML serializer ---------------------------------------------

/// Elements whose text is written raw by the HTML5 serializer.
const RAW_OUT: &[&[u8]] = &[
    b"style",
    b"script",
    b"xmp",
    b"iframe",
    b"noembed",
    b"noframes",
    b"plaintext",
];

/// `Dom\HTMLDocument::saveHtml()`: the HTML5 serialization algorithm —
/// `<!DOCTYPE name>`, void elements without end tags, `&nbsp;` for
/// U+00A0, everything else raw.
pub fn save_modern(doc: &DocData, node: Option<NodeId>) -> Vec<u8> {
    let mut out = Vec::new();
    match node {
        Some(id) => modern_node(doc, id, &mut out),
        None => {
            for c in doc.children(DOCUMENT) {
                modern_node(doc, c, &mut out);
            }
        }
    }
    out
}

fn modern_node(doc: &DocData, id: NodeId, out: &mut Vec<u8>) {
    let n = doc.node(id);
    match n.kind {
        NodeKind::Element => {
            let html_ns =
                n.ns.is_none() || n.ns.as_deref() == Some(b"http://www.w3.org/1999/xhtml");
            let name = if html_ns || matches!(n.ns.as_deref(), Some(SVG_NS) | Some(MATHML_NS)) {
                n.name.clone()
            } else {
                n.qualified_name()
            };
            out.push(b'<');
            out.extend_from_slice(&name);
            for &a in &n.attrs {
                out.push(b' ');
                out.extend_from_slice(&doc.node(a).qualified_name());
                out.extend_from_slice(b"=\"");
                modern_escape(&doc.attr_value(a), true, out);
                out.push(b'"');
            }
            out.push(b'>');
            if html_ns && VOID.contains(&name.as_slice()) && name != b"isindex" {
                return;
            }
            if html_ns && matches!(name.as_slice(), b"pre" | b"textarea" | b"listing") {
                if let Some(first) = n.first_child.filter(|&f| {
                    doc.node(f).kind == NodeKind::Text && doc.node(f).value.starts_with(b"\n")
                }) {
                    let _ = first;
                    out.push(b'\n');
                }
            }
            let raw = html_ns && RAW_OUT.contains(&name.as_slice());
            for c in doc.children(id) {
                if raw && doc.node(c).kind == NodeKind::Text {
                    out.extend_from_slice(&doc.node(c).value);
                } else {
                    modern_node(doc, c, out);
                }
            }
            out.extend_from_slice(b"</");
            out.extend_from_slice(&name);
            out.push(b'>');
        }
        NodeKind::Text => modern_escape(&n.value, false, out),
        NodeKind::CData => {
            out.extend_from_slice(b"<![CDATA[");
            out.extend_from_slice(&n.value);
            out.extend_from_slice(b"]]>");
        }
        NodeKind::Comment => {
            out.extend_from_slice(b"<!--");
            out.extend_from_slice(&n.value);
            out.extend_from_slice(b"-->");
        }
        NodeKind::Pi => {
            out.extend_from_slice(b"<?");
            out.extend_from_slice(&n.name);
            out.push(b' ');
            out.extend_from_slice(&n.value);
            out.push(b'>');
        }
        NodeKind::DocumentType => {
            out.extend_from_slice(b"<!DOCTYPE ");
            out.extend_from_slice(&n.name);
            out.push(b'>');
        }
        NodeKind::EntityRef => {
            out.push(b'&');
            out.extend_from_slice(&n.name);
            out.push(b';');
        }
        NodeKind::Document | NodeKind::Fragment => {
            for c in doc.children(id) {
                modern_node(doc, c, out);
            }
        }
        NodeKind::Attribute => out.extend_from_slice(&doc.attr_value(id)),
        NodeKind::Entity | NodeKind::Notation => {}
    }
}

fn modern_escape(v: &[u8], in_attr: bool, out: &mut Vec<u8>) {
    let mut i = 0;
    while i < v.len() {
        match v[i] {
            b'&' => out.extend_from_slice(b"&amp;"),
            b'<' if !in_attr => out.extend_from_slice(b"&lt;"),
            b'>' if !in_attr => out.extend_from_slice(b"&gt;"),
            b'"' if in_attr => out.extend_from_slice(b"&quot;"),
            0xC2 if v.get(i + 1) == Some(&0xA0) => {
                out.extend_from_slice(b"&nbsp;");
                i += 1;
            }
            b => out.push(b),
        }
        i += 1;
    }
}
