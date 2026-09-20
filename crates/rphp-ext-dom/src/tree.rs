//! The node tree: one arena per document (php's `xmlDoc`), every node a
//! slot in it, addressed by [`NodeId`]. A php object over a node holds the
//! arena and the id ([`NodeRef`]); the arena remembers the object made for
//! each node so the same node is always the same object (php's `dom_object`
//! map). Nodes are never freed while the document lives — a node cut out of
//! the tree stays reachable through its object, as libxml's do.
//!
//! What lives here is the tree *model* and the DOM mutation rules
//! (`appendChild` and friends with their `DOMException` codes); the php
//! class surface is `classes/`, parsing is `parser.rs`, serialization
//! `serialize.rs`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use rphp_value::{Object, WeakObject};

pub type NodeId = usize;

/// php's `XML_*_NODE` numbers.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NodeKind {
    Element = 1,
    Attribute = 2,
    Text = 3,
    CData = 4,
    EntityRef = 5,
    Entity = 6,
    Pi = 7,
    Comment = 8,
    Document = 9,
    DocumentType = 10,
    Fragment = 11,
    Notation = 12,
}

/// A namespace declaration on an element (`xmlns="…"` / `xmlns:p="…"`),
/// which libxml keeps apart from the attributes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NsDecl {
    pub prefix: Option<Vec<u8>>,
    pub uri: Vec<u8>,
}

/// The `<!DOCTYPE>` node's own fields.
#[derive(Clone, Debug, Default)]
pub struct DocTypeInfo {
    pub public_id: Vec<u8>,
    pub system_id: Vec<u8>,
    /// The internal subset text between `[` and `]`, as written.
    pub internal_subset: Option<Vec<u8>>,
    /// Internal general entities: name → replacement text.
    pub entities: Vec<(Vec<u8>, Vec<u8>)>,
    /// Every general entity declared, in order: name, replacement text
    /// (internal), public id, system id, notation name (unparsed).
    pub entity_decls: Vec<EntityDecl>,
    /// `<!NOTATION>` declarations, in order: name, public id, system id.
    pub notations: Vec<(Vec<u8>, Vec<u8>, Vec<u8>)>,
    /// The internal subset's markup declarations in order, as libxml
    /// re-serializes them (`saveXML()` prints these, not the text).
    pub decls: Vec<DtdDecl>,
}

#[derive(Clone, Debug, Default)]
pub struct EntityDecl {
    pub name: Vec<u8>,
    pub value: Option<Vec<u8>>,
    /// The value literal as written (what libxml prints).
    pub orig: Vec<u8>,
    pub public_id: Option<Vec<u8>>,
    pub system_id: Option<Vec<u8>>,
    pub notation: Option<Vec<u8>>,
}

/// One markup declaration of an internal subset.
#[derive(Clone, Debug)]
pub enum DtdDecl {
    /// `<!ENTITY [%] name …>`.
    Entity {
        decl: EntityDecl,
        parameter: bool,
    },
    /// `<!ELEMENT name content>`, the content model normalized.
    Element {
        name: Vec<u8>,
        content: Vec<u8>,
    },
    /// `<!ATTLIST element …>`: one entry per attribute, (name, type, default).
    Attlist {
        element: Vec<u8>,
        attrs: Vec<(Vec<u8>, Vec<u8>, Vec<u8>)>,
    },
    Comment(Vec<u8>),
    Pi {
        target: Vec<u8>,
        data: Vec<u8>,
    },
}

#[derive(Clone, Debug)]
pub struct Node {
    pub kind: NodeKind,
    /// The local name (element, attribute), the target (PI), the entity
    /// name (entity reference), the root name (doctype); empty otherwise.
    pub name: Vec<u8>,
    pub prefix: Option<Vec<u8>>,
    pub ns: Option<Vec<u8>>,
    /// Text, comment, CDATA and PI content. An attribute's value is its
    /// text children.
    pub value: Vec<u8>,
    pub parent: Option<NodeId>,
    pub first_child: Option<NodeId>,
    pub last_child: Option<NodeId>,
    pub prev: Option<NodeId>,
    pub next: Option<NodeId>,
    /// An element's attributes in document order (attribute nodes whose
    /// `parent` is the element).
    pub attrs: Vec<NodeId>,
    pub ns_decls: Vec<NsDecl>,
    pub line: u32,
    pub doctype: Option<Box<DocTypeInfo>>,
    /// An attribute created by the parser (`specified`) rather than a
    /// default from a DTD — always true here.
    pub specified: bool,
    /// An attribute `setIdAttribute()` marked (what `getElementById()`
    /// searches, along with `xml:id`).
    pub is_id: bool,
    /// An entity declaration node's own record.
    pub entity: Option<Box<EntityDecl>>,
}

impl Node {
    fn new(kind: NodeKind) -> Node {
        Node {
            kind,
            name: Vec::new(),
            prefix: None,
            ns: None,
            value: Vec::new(),
            parent: None,
            first_child: None,
            last_child: None,
            prev: None,
            next: None,
            attrs: Vec::new(),
            ns_decls: Vec::new(),
            line: 0,
            doctype: None,
            specified: true,
            is_id: false,
            entity: None,
        }
    }

    /// `nodeName`.
    pub fn node_name(&self) -> Vec<u8> {
        match self.kind {
            NodeKind::Element | NodeKind::Attribute => self.qualified_name(),
            NodeKind::Text => b"#text".to_vec(),
            NodeKind::CData => b"#cdata-section".to_vec(),
            NodeKind::Comment => b"#comment".to_vec(),
            NodeKind::Document => b"#document".to_vec(),
            NodeKind::Fragment => b"#document-fragment".to_vec(),
            NodeKind::Pi
            | NodeKind::EntityRef
            | NodeKind::Entity
            | NodeKind::DocumentType
            | NodeKind::Notation => self.name.clone(),
        }
    }

    /// `prefix:local` or `local`.
    pub fn qualified_name(&self) -> Vec<u8> {
        match &self.prefix {
            Some(p) if !p.is_empty() => {
                let mut q = p.clone();
                q.push(b':');
                q.extend_from_slice(&self.name);
                q
            }
            _ => self.name.clone(),
        }
    }
}

/// The DOM's exception codes (`DOMException::$code`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DomError {
    IndexSize = 1,
    HierarchyRequest = 3,
    WrongDocument = 4,
    InvalidCharacter = 5,
    NoModificationAllowed = 7,
    NotFound = 8,
    NotSupported = 9,
    InuseAttribute = 10,
    Syntax = 12,
    Namespace = 14,
    Validation = 16,
}

impl DomError {
    /// php's message for the code.
    pub fn message(self) -> &'static str {
        match self {
            DomError::IndexSize => "Index Size Error",
            DomError::HierarchyRequest => "Hierarchy Request Error",
            DomError::WrongDocument => "Wrong Document Error",
            DomError::InvalidCharacter => "Invalid Character Error",
            DomError::NoModificationAllowed => "No Modification Allowed Error",
            DomError::NotFound => "Not Found Error",
            DomError::NotSupported => "Not Supported Error",
            DomError::InuseAttribute => "Inuse Attribute Error",
            DomError::Syntax => "Syntax Error",
            DomError::Namespace => "Namespace Error",
            DomError::Validation => "Validation Error",
        }
    }
}

/// The document: its nodes, its declaration, php's per-document flags and
/// the objects made over its nodes.
pub struct DocData {
    pub nodes: Vec<Node>,
    /// The object over each node, while one exists.
    pub objects: HashMap<NodeId, WeakObject>,
    pub version: Option<Vec<u8>>,
    pub encoding: Option<Vec<u8>>,
    pub standalone: Option<bool>,
    pub document_uri: Option<Vec<u8>>,
    pub format_output: bool,
    pub preserve_white_space: bool,
    pub validate_on_parse: bool,
    pub resolve_externals: bool,
    pub strict_error_checking: bool,
    pub recover: bool,
    pub substitute_entities: bool,
    /// The document is an HTML one (`loadHTML`): `saveXML` still works, the
    /// class stays `DOMDocument`.
    pub is_html: bool,
    /// A php 8.4 `Dom\HTMLDocument` / `Dom\XMLDocument`: its nodes wrap in
    /// the `Dom\*` classes, HTML element names read uppercase.
    pub modern: bool,
    /// `registerNodeClass()`: base class (lowercase) → the class to make.
    pub node_classes: Vec<(Vec<u8>, Vec<u8>)>,
    /// A holding document for a node made with `new DOMElement(...)` and the
    /// like, which has no document of its own until it is appended
    /// somewhere: its nodes may be adopted by any document.
    pub orphan: bool,
}

pub type Doc = Rc<RefCell<DocData>>;

/// A node handle a php object carries.
#[derive(Clone)]
pub struct NodeRef {
    pub doc: Doc,
    pub id: NodeId,
}

impl NodeRef {
    pub fn same(&self, other: &NodeRef) -> bool {
        Rc::ptr_eq(&self.doc, &other.doc) && self.id == other.id
    }
}

pub const DOCUMENT: NodeId = 0;

impl DocData {
    pub fn new() -> Doc {
        let mut d = DocData {
            nodes: Vec::new(),
            objects: HashMap::new(),
            version: Some(b"1.0".to_vec()),
            encoding: None,
            standalone: None,
            document_uri: None,
            format_output: false,
            preserve_white_space: true,
            validate_on_parse: false,
            resolve_externals: false,
            strict_error_checking: true,
            recover: false,
            substitute_entities: false,
            is_html: false,
            modern: false,
            node_classes: Vec::new(),
            orphan: false,
        };
        d.nodes.push(Node::new(NodeKind::Document));
        Rc::new(RefCell::new(d))
    }

    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id]
    }

    pub fn node_mut(&mut self, id: NodeId) -> &mut Node {
        &mut self.nodes[id]
    }

    pub fn push(&mut self, node: Node) -> NodeId {
        self.nodes.push(node);
        self.nodes.len() - 1
    }

    // ---- creation --------------------------------------------------------

    pub fn create_element(
        &mut self,
        name: &[u8],
        prefix: Option<&[u8]>,
        ns: Option<&[u8]>,
    ) -> NodeId {
        let mut n = Node::new(NodeKind::Element);
        n.name = name.to_vec();
        n.prefix = prefix.map(<[u8]>::to_vec);
        n.ns = ns.map(<[u8]>::to_vec);
        self.push(n)
    }

    pub fn create_text(&mut self, kind: NodeKind, value: &[u8]) -> NodeId {
        let mut n = Node::new(kind);
        n.value = value.to_vec();
        self.push(n)
    }

    pub fn create_pi(&mut self, target: &[u8], data: &[u8]) -> NodeId {
        let mut n = Node::new(NodeKind::Pi);
        n.name = target.to_vec();
        n.value = data.to_vec();
        self.push(n)
    }

    pub fn create_entity_ref(&mut self, name: &[u8]) -> NodeId {
        let mut n = Node::new(NodeKind::EntityRef);
        n.name = name.to_vec();
        self.push(n)
    }

    pub fn create_fragment(&mut self) -> NodeId {
        self.push(Node::new(NodeKind::Fragment))
    }

    pub fn create_doctype(&mut self, name: &[u8], info: DocTypeInfo) -> NodeId {
        let mut n = Node::new(NodeKind::DocumentType);
        n.name = name.to_vec();
        let decls = info.entity_decls.clone();
        let notations = info.notations.clone();
        n.doctype = Some(Box::new(info));
        let dt = self.push(n);
        // The declarations as nodes (`$doctype->entities` / `notations`):
        // an entity hangs off the doctype (its `parentNode`) without being
        // a child; a notation stands alone.
        for decl in decls {
            let mut e = Node::new(NodeKind::Entity);
            e.name = decl.name.clone();
            let value = decl.value.clone();
            e.entity = Some(Box::new(decl));
            let id = self.push(e);
            if let Some(v) = value {
                let t = self.create_text(NodeKind::Text, &v);
                self.link_last(id, t);
            }
            self.link_last(dt, id);
        }
        for (name, public_id, system_id) in notations {
            let mut n = Node::new(NodeKind::Notation);
            n.name = name;
            n.doctype = Some(Box::new(DocTypeInfo {
                public_id,
                system_id,
                ..DocTypeInfo::default()
            }));
            n.specified = true;
            let id = self.push(n);
            self.nodes[id].line = dt as u32;
        }
        dt
    }

    /// The entity / notation declaration nodes of a doctype, in order.
    pub fn decls_of(&self, dt: NodeId, kind: NodeKind) -> Vec<NodeId> {
        if kind == NodeKind::Entity {
            return self
                .children(dt)
                .into_iter()
                .filter(|&c| self.nodes[c].kind == NodeKind::Entity)
                .collect();
        }
        (0..self.nodes.len())
            .filter(|&i| self.nodes[i].kind == kind && self.nodes[i].line as usize == dt)
            .collect()
    }

    /// A detached attribute with `value` as its one text child.
    pub fn create_attr(
        &mut self,
        name: &[u8],
        prefix: Option<&[u8]>,
        ns: Option<&[u8]>,
        value: &[u8],
    ) -> NodeId {
        let mut n = Node::new(NodeKind::Attribute);
        n.name = name.to_vec();
        n.prefix = prefix.map(<[u8]>::to_vec);
        n.ns = ns.map(<[u8]>::to_vec);
        let a = self.push(n);
        if !value.is_empty() {
            let t = self.create_text(NodeKind::Text, value);
            self.link_last(a, t);
        }
        a
    }

    // ---- reading ---------------------------------------------------------

    pub fn children(&self, id: NodeId) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut cur = self.nodes[id].first_child;
        while let Some(c) = cur {
            out.push(c);
            cur = self.nodes[c].next;
        }
        out
    }

    /// The text an attribute holds (its text children joined).
    pub fn attr_value(&self, attr: NodeId) -> Vec<u8> {
        let mut out = Vec::new();
        for c in self.children(attr) {
            match self.nodes[c].kind {
                NodeKind::EntityRef => {
                    out.extend_from_slice(&self.entity_value(&self.nodes[c].name))
                }
                _ => out.extend_from_slice(&self.nodes[c].value),
            }
        }
        out
    }

    /// `textContent`: every text-like descendant, entity references
    /// expanded; `None` for a document / doctype (php answers null).
    pub fn text_content(&self, id: NodeId) -> Option<Vec<u8>> {
        let n = &self.nodes[id];
        match n.kind {
            NodeKind::DocumentType | NodeKind::Notation => None,
            NodeKind::Document => {
                // php: the text of the whole document (its root's).
                let mut out = Vec::new();
                self.collect_text(id, &mut out);
                Some(out)
            }
            NodeKind::Text | NodeKind::CData | NodeKind::Comment | NodeKind::Pi => {
                Some(n.value.clone())
            }
            NodeKind::EntityRef => Some(self.entity_value(&n.name)),
            NodeKind::Attribute => Some(self.attr_value(id)),
            NodeKind::Element | NodeKind::Fragment | NodeKind::Entity => {
                let mut out = Vec::new();
                self.collect_text(id, &mut out);
                Some(out)
            }
        }
    }

    fn collect_text(&self, id: NodeId, out: &mut Vec<u8>) {
        for c in self.children(id) {
            match self.nodes[c].kind {
                NodeKind::Text | NodeKind::CData => out.extend_from_slice(&self.nodes[c].value),
                NodeKind::EntityRef => {
                    out.extend_from_slice(&self.entity_value(&self.nodes[c].name))
                }
                NodeKind::Element | NodeKind::Entity => self.collect_text(c, out),
                _ => {}
            }
        }
    }

    /// The replacement text of an internal entity (`<!ENTITY n "…">`),
    /// or the five predefined ones; empty when undeclared.
    pub fn entity_value(&self, name: &[u8]) -> Vec<u8> {
        match name {
            b"lt" => return b"<".to_vec(),
            b"gt" => return b">".to_vec(),
            b"amp" => return b"&".to_vec(),
            b"quot" => return b"\"".to_vec(),
            b"apos" => return b"'".to_vec(),
            _ => {}
        }
        self.doctype()
            .and_then(|dt| self.nodes[dt].doctype.as_ref())
            .and_then(|info| {
                info.entities
                    .iter()
                    .find(|(n, _)| n == name)
                    .map(|(_, v)| v.clone())
            })
            .unwrap_or_default()
    }

    pub fn doctype(&self) -> Option<NodeId> {
        self.children(DOCUMENT)
            .into_iter()
            .find(|&c| self.nodes[c].kind == NodeKind::DocumentType)
    }

    pub fn document_element(&self) -> Option<NodeId> {
        self.children(DOCUMENT)
            .into_iter()
            .find(|&c| self.nodes[c].kind == NodeKind::Element)
    }

    /// The nearest ancestor-or-self element declaring `prefix` (`None` =
    /// the default namespace): its URI.
    pub fn lookup_ns(&self, from: NodeId, prefix: Option<&[u8]>) -> Option<Vec<u8>> {
        let mut cur = Some(from);
        while let Some(id) = cur {
            let n = &self.nodes[id];
            if n.kind == NodeKind::Element {
                for d in &n.ns_decls {
                    if d.prefix.as_deref() == prefix {
                        return Some(d.uri.clone());
                    }
                }
            }
            cur = n.parent;
        }
        if prefix == Some(b"xml") {
            return Some(b"http://www.w3.org/XML/1998/namespace".to_vec());
        }
        None
    }

    /// The prefix an ancestor-or-self declares for `uri`.
    pub fn lookup_prefix(&self, from: NodeId, uri: &[u8]) -> Option<Option<Vec<u8>>> {
        let mut cur = Some(from);
        while let Some(id) = cur {
            let n = &self.nodes[id];
            if n.kind == NodeKind::Element {
                for d in &n.ns_decls {
                    if d.uri == uri {
                        return Some(d.prefix.clone());
                    }
                }
            }
            cur = n.parent;
        }
        None
    }

    pub fn is_ancestor(&self, ancestor: NodeId, of: NodeId) -> bool {
        let mut cur = self.nodes[of].parent;
        while let Some(p) = cur {
            if p == ancestor {
                return true;
            }
            cur = self.nodes[p].parent;
        }
        false
    }

    /// Whether `id` hangs off the document (php's `isConnected`).
    pub fn is_connected(&self, id: NodeId) -> bool {
        id == DOCUMENT || self.is_ancestor(DOCUMENT, id) || {
            // An attribute of a connected element.
            let n = &self.nodes[id];
            n.kind == NodeKind::Attribute && n.parent.is_some_and(|p| self.is_connected(p))
        }
    }

    /// Document order of every node under `root` (root included),
    /// attributes and namespace declarations excluded.
    pub fn descendants(&self, root: NodeId) -> Vec<NodeId> {
        let mut out = Vec::new();
        self.walk(root, &mut out);
        out
    }

    fn walk(&self, id: NodeId, out: &mut Vec<NodeId>) {
        out.push(id);
        for c in self.children(id) {
            self.walk(c, out);
        }
    }

    /// The elements under `root` (root excluded) named `name` /
    /// `ns:local`; `*` matches every name.
    pub fn elements_by_name(&self, root: NodeId, name: &[u8]) -> Vec<NodeId> {
        // php 8.4's HTML documents match their HTML elements by lowercased
        // name, and foreign (svg/math) elements by their exact name.
        let html_doc = self.modern && self.is_html;
        let lower = name.to_ascii_lowercase();
        self.descendants(root)
            .into_iter()
            .skip(1)
            .filter(|&id| {
                let n = &self.nodes[id];
                if n.kind != NodeKind::Element || name == b"*" {
                    return n.kind == NodeKind::Element;
                }
                let qn = n.qualified_name();
                if html_doc && n.ns.as_deref() == Some(b"http://www.w3.org/1999/xhtml".as_slice()) {
                    qn == lower
                } else {
                    qn == name
                }
            })
            .collect()
    }

    pub fn elements_by_ns(&self, root: NodeId, ns: &[u8], local: &[u8]) -> Vec<NodeId> {
        self.descendants(root)
            .into_iter()
            .skip(1)
            .filter(|&id| {
                let n = &self.nodes[id];
                n.kind == NodeKind::Element
                    && (local == b"*" || n.name == local)
                    && (ns == b"*" || n.ns.as_deref().unwrap_or(b"") == ns)
            })
            .collect()
    }

    // ---- linking ---------------------------------------------------------

    /// Detach `id` from its parent and siblings (an attribute from its
    /// element's list).
    pub fn unlink(&mut self, id: NodeId) {
        let (parent, prev, next, kind) = {
            let n = &self.nodes[id];
            (n.parent, n.prev, n.next, n.kind)
        };
        let Some(p) = parent else {
            return;
        };
        if kind == NodeKind::Attribute {
            self.nodes[p].attrs.retain(|&a| a != id);
            self.nodes[id].parent = None;
            return;
        }
        match prev {
            Some(pr) => self.nodes[pr].next = next,
            None => self.nodes[p].first_child = next,
        }
        match next {
            Some(nx) => self.nodes[nx].prev = prev,
            None => self.nodes[p].last_child = prev,
        }
        let n = &mut self.nodes[id];
        n.parent = None;
        n.prev = None;
        n.next = None;
    }

    /// Link `child` as the last child of `parent` (no checks).
    pub fn link_last(&mut self, parent: NodeId, child: NodeId) {
        self.unlink(child);
        let last = self.nodes[parent].last_child;
        self.nodes[child].parent = Some(parent);
        self.nodes[child].prev = last;
        self.nodes[child].next = None;
        match last {
            Some(l) => self.nodes[l].next = Some(child),
            None => self.nodes[parent].first_child = Some(child),
        }
        self.nodes[parent].last_child = Some(child);
    }

    /// Link `child` immediately before `before` (a child of `parent`).
    pub fn link_before(&mut self, parent: NodeId, child: NodeId, before: NodeId) {
        self.unlink(child);
        let prev = self.nodes[before].prev;
        self.nodes[child].parent = Some(parent);
        self.nodes[child].prev = prev;
        self.nodes[child].next = Some(before);
        self.nodes[before].prev = Some(child);
        match prev {
            Some(p) => self.nodes[p].next = Some(child),
            None => self.nodes[parent].first_child = Some(child),
        }
    }

    /// Whether `child` may become a child of `parent` (the DOM's
    /// hierarchy rules, as php checks them).
    fn hierarchy_ok(&self, parent: NodeId, child: NodeId) -> bool {
        let pk = self.nodes[parent].kind;
        let ck = self.nodes[child].kind;
        if child == parent || self.is_ancestor(child, parent) {
            return false;
        }
        match pk {
            NodeKind::Document => match ck {
                NodeKind::Element => self.document_element().is_none(),
                NodeKind::DocumentType => self.doctype().is_none(),
                // libxml (and so php) lets text hang off the document.
                NodeKind::Comment | NodeKind::Pi | NodeKind::Text | NodeKind::CData => true,
                NodeKind::Fragment => self.children(child).iter().all(|&c| {
                    matches!(self.nodes[c].kind, NodeKind::Comment | NodeKind::Pi)
                        || (self.nodes[c].kind == NodeKind::Element
                            && self.document_element().is_none())
                }),
                _ => false,
            },
            NodeKind::Element | NodeKind::Fragment | NodeKind::Entity => matches!(
                ck,
                NodeKind::Element
                    | NodeKind::Text
                    | NodeKind::CData
                    | NodeKind::Comment
                    | NodeKind::Pi
                    | NodeKind::EntityRef
                    | NodeKind::Fragment
            ),
            NodeKind::Attribute => matches!(ck, NodeKind::Text | NodeKind::EntityRef),
            _ => false,
        }
    }

    /// `appendChild`: the DOM checks, then the link (a fragment empties into
    /// the parent). Returns what php returns: the appended node.
    pub fn append_child(&mut self, parent: NodeId, child: NodeId) -> Result<NodeId, DomError> {
        if !self.hierarchy_ok(parent, child) {
            return Err(DomError::HierarchyRequest);
        }
        if self.nodes[child].kind == NodeKind::Fragment {
            for c in self.children(child) {
                self.link_last(parent, c);
            }
            return Ok(child);
        }
        self.link_last(parent, child);
        Ok(child)
    }

    /// `insertBefore($new, $ref)`; `ref` `None` appends.
    pub fn insert_before(
        &mut self,
        parent: NodeId,
        child: NodeId,
        before: Option<NodeId>,
    ) -> Result<NodeId, DomError> {
        let Some(before) = before else {
            return self.append_child(parent, child);
        };
        if self.nodes[before].parent != Some(parent) {
            return Err(DomError::NotFound);
        }
        if !self.hierarchy_ok(parent, child) {
            return Err(DomError::HierarchyRequest);
        }
        if child == before {
            return Ok(child);
        }
        if self.nodes[child].kind == NodeKind::Fragment {
            for c in self.children(child) {
                self.link_before(parent, c, before);
            }
            return Ok(child);
        }
        self.link_before(parent, child, before);
        Ok(child)
    }

    /// `removeChild`.
    pub fn remove_child(&mut self, parent: NodeId, child: NodeId) -> Result<NodeId, DomError> {
        if self.nodes[child].parent != Some(parent) || self.nodes[child].kind == NodeKind::Attribute
        {
            return Err(DomError::NotFound);
        }
        self.unlink(child);
        Ok(child)
    }

    /// `replaceChild($new, $old)`: returns the old node.
    pub fn replace_child(
        &mut self,
        parent: NodeId,
        new: NodeId,
        old: NodeId,
    ) -> Result<NodeId, DomError> {
        if self.nodes[old].parent != Some(parent) {
            return Err(DomError::NotFound);
        }
        if !self.hierarchy_ok(parent, new) && new != old {
            return Err(DomError::HierarchyRequest);
        }
        if new == old {
            return Ok(old);
        }
        let next = self.nodes[old].next;
        self.unlink(old);
        match next {
            Some(nx) => self.insert_before(parent, new, Some(nx))?,
            None => self.append_child(parent, new)?,
        };
        Ok(old)
    }

    // ---- attributes ------------------------------------------------------

    pub fn find_attr(&self, element: NodeId, qname: &[u8]) -> Option<NodeId> {
        self.nodes[element]
            .attrs
            .iter()
            .copied()
            .find(|&a| self.nodes[a].qualified_name() == qname)
    }

    pub fn find_attr_ns(&self, element: NodeId, ns: Option<&[u8]>, local: &[u8]) -> Option<NodeId> {
        self.nodes[element].attrs.iter().copied().find(|&a| {
            let n = &self.nodes[a];
            n.name == local && n.ns.as_deref().unwrap_or(b"") == ns.unwrap_or(b"")
        })
    }

    /// Attach a detached attribute node to `element`, replacing one of the
    /// same name; the replaced one comes back (php's `setAttributeNode`).
    pub fn set_attr_node(
        &mut self,
        element: NodeId,
        attr: NodeId,
    ) -> Result<Option<NodeId>, DomError> {
        if let Some(owner) = self.nodes[attr].parent {
            if owner != element {
                return Err(DomError::InuseAttribute);
            }
            return Ok(None);
        }
        let qname = self.nodes[attr].qualified_name();
        let existing = self.find_attr(element, &qname);
        if let Some(e) = existing {
            let pos = self.nodes[element]
                .attrs
                .iter()
                .position(|&a| a == e)
                .unwrap_or(0);
            self.nodes[element].attrs[pos] = attr;
            self.nodes[e].parent = None;
        } else {
            self.nodes[element].attrs.push(attr);
        }
        self.nodes[attr].parent = Some(element);
        Ok(existing)
    }

    /// `setAttribute(name, value)`: update in place or add.
    pub fn set_attr(&mut self, element: NodeId, qname: &[u8], value: &[u8]) -> NodeId {
        if let Some(a) = self.find_attr(element, qname) {
            self.set_attr_value(a, value);
            return a;
        }
        let (prefix, local) = split_qname(qname);
        let ns = prefix.and_then(|p| self.lookup_ns(element, Some(p)));
        let a = self.create_attr(local, prefix, ns.as_deref(), value);
        self.nodes[a].parent = Some(element);
        self.nodes[element].attrs.push(a);
        a
    }

    /// Replace an attribute's children with one text node holding `value`.
    pub fn set_attr_value(&mut self, attr: NodeId, value: &[u8]) {
        for c in self.children(attr) {
            self.unlink(c);
        }
        if !value.is_empty() {
            let t = self.create_text(NodeKind::Text, value);
            self.link_last(attr, t);
        }
    }

    pub fn remove_attr(&mut self, element: NodeId, qname: &[u8]) -> bool {
        match self.find_attr(element, qname) {
            Some(a) => {
                self.unlink(a);
                true
            }
            None => false,
        }
    }

    // ---- copying ---------------------------------------------------------

    /// A copy of `src` (from `src_doc`, which may be this document) made in
    /// this document; `deep` copies the subtree. Attributes always come
    /// along with an element.
    pub fn import(&mut self, src_doc: &DocData, src: NodeId, deep: bool) -> NodeId {
        let s = src_doc.node(src).clone();
        let mut n = Node::new(s.kind);
        n.name = s.name.clone();
        n.prefix = s.prefix.clone();
        n.ns = s.ns.clone();
        n.value = s.value.clone();
        n.ns_decls = s.ns_decls.clone();
        n.line = s.line;
        n.doctype = s.doctype.clone();
        n.specified = s.specified;
        n.is_id = s.is_id;
        let id = self.push(n);
        for &a in &s.attrs {
            let copy = self.import(src_doc, a, true);
            self.nodes[copy].parent = Some(id);
            self.nodes[id].attrs.push(copy);
        }
        if deep || s.kind == NodeKind::Attribute {
            for c in src_doc.children(src) {
                let copy = self.import(src_doc, c, true);
                self.link_last(id, copy);
            }
        }
        id
    }

    /// `cloneNode($deep)` within the document (the source's ids stay valid
    /// while the copies are pushed after them).
    pub fn clone_node(&mut self, src: NodeId, deep: bool) -> NodeId {
        let s = self.nodes[src].clone();
        let mut n = Node::new(s.kind);
        n.name = s.name.clone();
        n.prefix = s.prefix.clone();
        n.ns = s.ns.clone();
        n.value = s.value.clone();
        n.ns_decls = s.ns_decls.clone();
        n.line = s.line;
        n.doctype = s.doctype.clone();
        n.specified = s.specified;
        n.is_id = s.is_id;
        let id = self.push(n);
        for &a in &s.attrs {
            let copy = self.clone_node(a, true);
            self.nodes[copy].parent = Some(id);
            self.nodes[id].attrs.push(copy);
        }
        if deep || s.kind == NodeKind::Attribute {
            for c in self.children(src) {
                let copy = self.clone_node(c, true);
                self.link_last(id, copy);
            }
        }
        id
    }

    // ---- objects ---------------------------------------------------------

    pub fn object_of(&self, id: NodeId) -> Option<Object> {
        self.objects.get(&id).and_then(WeakObject::upgrade)
    }

    pub fn remember(&mut self, id: NodeId, o: &Object) {
        self.objects.insert(id, o.downgrade());
    }

    /// Normalize: merge adjacent text nodes, drop empty ones (php's
    /// `normalize()`), under `id`.
    pub fn normalize(&mut self, id: NodeId) {
        let children = self.children(id);
        let mut i = 0;
        while i < children.len() {
            let c = children[i];
            if self.nodes[c].kind == NodeKind::Text {
                let mut j = i + 1;
                while j < children.len() && self.nodes[children[j]].kind == NodeKind::Text {
                    let v = self.nodes[children[j]].value.clone();
                    self.nodes[c].value.extend_from_slice(&v);
                    self.unlink(children[j]);
                    j += 1;
                }
                if self.nodes[c].value.is_empty() {
                    self.unlink(c);
                }
                i = j;
            } else {
                if self.nodes[c].kind == NodeKind::Element {
                    self.normalize(c);
                }
                i += 1;
            }
        }
    }
}

/// `p:local` → `(Some(p), local)`.
pub fn split_qname(qname: &[u8]) -> (Option<&[u8]>, &[u8]) {
    match qname.iter().position(|&b| b == b':') {
        Some(i) if i > 0 && i + 1 < qname.len() => (Some(&qname[..i]), &qname[i + 1..]),
        _ => (None, qname),
    }
}

/// XML 1.0 `Name` validity (php's `INVALID_CHARACTER_ERR` check).
pub fn is_valid_name(name: &[u8]) -> bool {
    let s = match std::str::from_utf8(name) {
        Ok(s) => s,
        Err(_) => return false,
    };
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !is_name_start(first) {
        return false;
    }
    chars.all(is_name_char)
}

pub fn is_name_start(c: char) -> bool {
    c == ':'
        || c == '_'
        || c.is_ascii_alphabetic()
        || ('\u{C0}'..='\u{D6}').contains(&c)
        || ('\u{D8}'..='\u{F6}').contains(&c)
        || ('\u{F8}'..='\u{2FF}').contains(&c)
        || ('\u{370}'..='\u{37D}').contains(&c)
        || ('\u{37F}'..='\u{1FFF}').contains(&c)
        || ('\u{200C}'..='\u{200D}').contains(&c)
        || ('\u{2070}'..='\u{218F}').contains(&c)
        || ('\u{2C00}'..='\u{2FEF}').contains(&c)
        || ('\u{3001}'..='\u{D7FF}').contains(&c)
        || ('\u{F900}'..='\u{FDCF}').contains(&c)
        || ('\u{FDF0}'..='\u{FFFD}').contains(&c)
        || ('\u{10000}'..='\u{EFFFF}').contains(&c)
}

pub fn is_name_char(c: char) -> bool {
    is_name_start(c)
        || c == '-'
        || c == '.'
        || c.is_ascii_digit()
        || c == '\u{B7}'
        || ('\u{300}'..='\u{36F}').contains(&c)
        || ('\u{203F}'..='\u{2040}').contains(&c)
}
