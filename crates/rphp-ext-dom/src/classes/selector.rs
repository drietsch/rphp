//! `querySelector()` / `querySelectorAll()` / `closest()` / `matches()`:
//! a CSS selector engine over the tree — type, universal, class, id and
//! attribute selectors, the four combinators, selector lists, and the
//! structural pseudo-classes (`:not()`, `:is()`, `:first-child`,
//! `:nth-child(an+b)`, `:only-of-type`, `:empty`, `:root`, …).

use rphp_runtime::{Ctx, NativeResult, Unwind};
use rphp_value::{Object, Value};

use super::{node_ref, str_arg, this, wrap_value};
use crate::tree::{DocData, DomError, NodeId, NodeKind};

#[derive(Debug, Clone)]
enum Simple {
    Type(Vec<u8>),
    Universal,
    Id(Vec<u8>),
    Class(Vec<u8>),
    Attr {
        name: Vec<u8>,
        op: Option<AttrOp>,
        value: Vec<u8>,
        ci: bool,
    },
    Not(Vec<Complex>),
    Is(Vec<Complex>),
    FirstChild,
    LastChild,
    OnlyChild,
    FirstOfType,
    LastOfType,
    OnlyOfType,
    NthChild(i64, i64, bool),
    NthLastChild(i64, i64, bool),
    Empty,
    Root,
    Checked,
    Disabled,
    Enabled,
    Link,
}

#[derive(Debug, Clone, Copy)]
enum AttrOp {
    Eq,
    Includes,
    Dash,
    Prefix,
    Suffix,
    Substring,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Comb {
    Descendant,
    Child,
    Next,
    Subsequent,
}

#[derive(Debug, Clone)]
struct Compound(Vec<Simple>);

/// A complex selector: compounds joined by combinators, rightmost last.
#[derive(Debug, Clone)]
struct Complex {
    parts: Vec<(Option<Comb>, Compound)>,
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    fn ws(&mut self) -> bool {
        let start = self.i;
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r' | b'\x0c')) {
            self.i += 1;
        }
        self.i > start
    }

    fn ident(&mut self) -> Option<Vec<u8>> {
        let start = self.i;
        let mut out = Vec::new();
        while let Some(b) = self.peek() {
            if b == b'\\' {
                self.i += 1;
                if let Some(c) = self.peek() {
                    out.push(c);
                    self.i += 1;
                }
            } else if b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b >= 0x80 {
                out.push(b);
                self.i += 1;
            } else {
                break;
            }
        }
        if self.i == start
            || out.first() == Some(&b'-') && out.len() == 1
            || out.first().is_some_and(u8::is_ascii_digit)
        {
            self.i = start;
            return None;
        }
        Some(out)
    }

    fn string_or_ident(&mut self) -> Option<Vec<u8>> {
        match self.peek() {
            Some(q @ (b'"' | b'\'')) => {
                self.i += 1;
                let mut out = Vec::new();
                while let Some(b) = self.peek() {
                    self.i += 1;
                    if b == q {
                        return Some(out);
                    }
                    if b == b'\\' {
                        if let Some(c) = self.peek() {
                            out.push(c);
                            self.i += 1;
                        }
                        continue;
                    }
                    out.push(b);
                }
                None
            }
            _ => self.ident(),
        }
    }

    fn list(&mut self) -> Option<Vec<Complex>> {
        let mut out = Vec::new();
        loop {
            self.ws();
            out.push(self.complex()?);
            self.ws();
            if self.peek() == Some(b',') {
                self.i += 1;
                continue;
            }
            return Some(out);
        }
    }

    fn complex(&mut self) -> Option<Complex> {
        let mut parts = Vec::new();
        let first = self.compound()?;
        parts.push((None, first));
        loop {
            let had_ws = self.ws();
            let comb = match self.peek() {
                Some(b'>') => {
                    self.i += 1;
                    Comb::Child
                }
                Some(b'+') => {
                    self.i += 1;
                    Comb::Next
                }
                Some(b'~') => {
                    self.i += 1;
                    Comb::Subsequent
                }
                Some(b',' | b')') | None => return Some(Complex { parts }),
                _ if had_ws => Comb::Descendant,
                _ => return None,
            };
            self.ws();
            let c = self.compound()?;
            parts.push((Some(comb), c));
        }
    }

    fn compound(&mut self) -> Option<Compound> {
        let mut out = Vec::new();
        if self.peek() == Some(b'*') {
            self.i += 1;
            out.push(Simple::Universal);
        } else if let Some(t) = self.ident() {
            out.push(Simple::Type(t));
        }
        loop {
            match self.peek() {
                Some(b'#') => {
                    self.i += 1;
                    out.push(Simple::Id(self.ident()?));
                }
                Some(b'.') => {
                    self.i += 1;
                    out.push(Simple::Class(self.ident()?));
                }
                Some(b'[') => {
                    self.i += 1;
                    self.ws();
                    let name = self.ident()?;
                    self.ws();
                    let op = match self.peek() {
                        Some(b']') => None,
                        Some(b'=') => {
                            self.i += 1;
                            Some(AttrOp::Eq)
                        }
                        Some(c @ (b'~' | b'|' | b'^' | b'$' | b'*')) => {
                            self.i += 1;
                            if self.peek() != Some(b'=') {
                                return None;
                            }
                            self.i += 1;
                            Some(match c {
                                b'~' => AttrOp::Includes,
                                b'|' => AttrOp::Dash,
                                b'^' => AttrOp::Prefix,
                                b'$' => AttrOp::Suffix,
                                _ => AttrOp::Substring,
                            })
                        }
                        _ => return None,
                    };
                    let mut value = Vec::new();
                    let mut ci = false;
                    if op.is_some() {
                        self.ws();
                        value = self.string_or_ident()?;
                        self.ws();
                        if matches!(self.peek(), Some(b'i' | b'I')) {
                            self.i += 1;
                            ci = true;
                            self.ws();
                        } else if matches!(self.peek(), Some(b's' | b'S')) {
                            self.i += 1;
                            self.ws();
                        }
                    }
                    if self.peek() != Some(b']') {
                        return None;
                    }
                    self.i += 1;
                    out.push(Simple::Attr {
                        name,
                        op,
                        value,
                        ci,
                    });
                }
                Some(b':') => {
                    self.i += 1;
                    if self.peek() == Some(b':') {
                        return None;
                    }
                    let name = self.ident()?.to_ascii_lowercase();
                    let simple = match name.as_slice() {
                        b"not" | b"is" | b"where" => {
                            if self.peek() != Some(b'(') {
                                return None;
                            }
                            self.i += 1;
                            let inner = self.list()?;
                            self.ws();
                            if self.peek() != Some(b')') {
                                return None;
                            }
                            self.i += 1;
                            if name == b"not" {
                                Simple::Not(inner)
                            } else {
                                Simple::Is(inner)
                            }
                        }
                        b"nth-child" | b"nth-last-child" | b"nth-of-type" | b"nth-last-of-type" => {
                            if self.peek() != Some(b'(') {
                                return None;
                            }
                            self.i += 1;
                            self.ws();
                            let start = self.i;
                            while self.peek().is_some_and(|b| b != b')') {
                                self.i += 1;
                            }
                            let (a, b) = parse_nth(&self.s[start..self.i])?;
                            if self.peek() != Some(b')') {
                                return None;
                            }
                            self.i += 1;
                            let of_type = name.ends_with(b"of-type");
                            if name.starts_with(b"nth-last") {
                                Simple::NthLastChild(a, b, of_type)
                            } else {
                                Simple::NthChild(a, b, of_type)
                            }
                        }
                        b"first-child" => Simple::FirstChild,
                        b"last-child" => Simple::LastChild,
                        b"only-child" => Simple::OnlyChild,
                        b"first-of-type" => Simple::FirstOfType,
                        b"last-of-type" => Simple::LastOfType,
                        b"only-of-type" => Simple::OnlyOfType,
                        b"empty" => Simple::Empty,
                        b"root" => Simple::Root,
                        b"checked" => Simple::Checked,
                        b"disabled" => Simple::Disabled,
                        b"enabled" => Simple::Enabled,
                        b"link" | b"any-link" => Simple::Link,
                        _ => return None,
                    };
                    out.push(simple);
                }
                _ => break,
            }
        }
        if out.is_empty() {
            return None;
        }
        Some(Compound(out))
    }
}

/// `an+b`, `odd`, `even`, `3`, `-n+2`, `n`.
fn parse_nth(text: &[u8]) -> Option<(i64, i64)> {
    let t: Vec<u8> = text
        .iter()
        .filter(|b| !b.is_ascii_whitespace())
        .map(u8::to_ascii_lowercase)
        .collect();
    match t.as_slice() {
        b"odd" => return Some((2, 1)),
        b"even" => return Some((2, 0)),
        _ => {}
    }
    let s = std::str::from_utf8(&t).ok()?;
    if let Some(pos) = s.find('n') {
        let a_text = &s[..pos];
        let a = match a_text {
            "" | "+" => 1,
            "-" => -1,
            _ => a_text.parse().ok()?,
        };
        let b_text = &s[pos + 1..];
        let b = if b_text.is_empty() {
            0
        } else {
            b_text.trim_start_matches('+').parse().ok()?
        };
        Some((a, b))
    } else {
        Some((0, s.parse().ok()?))
    }
}

fn parse(text: &[u8]) -> Option<Vec<Complex>> {
    let mut p = Parser { s: text, i: 0 };
    let list = p.list()?;
    p.ws();
    if p.i != text.len() {
        return None;
    }
    Some(list)
}

// ---- matching --------------------------------------------------------------

fn is_html(doc: &DocData, id: NodeId) -> bool {
    doc.is_html
        && doc
            .node(id)
            .ns
            .as_deref()
            .is_none_or(|ns| ns == b"http://www.w3.org/1999/xhtml")
}

fn matches_compound(doc: &DocData, id: NodeId, c: &Compound, scope: Option<NodeId>) -> bool {
    c.0.iter().all(|s| matches_simple(doc, id, s, scope))
}

fn nth_index(doc: &DocData, id: NodeId, from_end: bool, of_type: bool) -> i64 {
    let n = doc.node(id);
    let mut i = 1;
    let mut cur = if from_end { n.next } else { n.prev };
    while let Some(c) = cur {
        let cn = doc.node(c);
        if cn.kind == NodeKind::Element && (!of_type || (cn.name == n.name && cn.ns == n.ns)) {
            i += 1;
        }
        cur = if from_end { cn.next } else { cn.prev };
    }
    i
}

fn nth_matches(a: i64, b: i64, index: i64) -> bool {
    if a == 0 {
        return index == b;
    }
    let d = index - b;
    d % a == 0 && d / a >= 0
}

fn matches_simple(doc: &DocData, id: NodeId, s: &Simple, scope: Option<NodeId>) -> bool {
    let n = doc.node(id);
    match s {
        Simple::Universal => true,
        Simple::Type(t) => {
            if is_html(doc, id) {
                n.name.eq_ignore_ascii_case(t)
            } else {
                n.name == *t
            }
        }
        Simple::Id(v) => doc
            .find_attr(id, b"id")
            .is_some_and(|a| doc.attr_value(a) == *v),
        Simple::Class(v) => doc.find_attr(id, b"class").is_some_and(|a| {
            doc.attr_value(a)
                .split(|b| b.is_ascii_whitespace())
                .any(|t| t == v.as_slice())
        }),
        Simple::Attr {
            name,
            op,
            value,
            ci,
        } => {
            let attr = if is_html(doc, id) {
                n.attrs
                    .iter()
                    .copied()
                    .find(|&a| doc.node(a).qualified_name().eq_ignore_ascii_case(name))
            } else {
                doc.find_attr(id, name)
            };
            let Some(attr) = attr else {
                return false;
            };
            let Some(op) = op else {
                return true;
            };
            let mut actual = doc.attr_value(attr);
            let mut want = value.clone();
            if *ci {
                actual.make_ascii_lowercase();
                want.make_ascii_lowercase();
            }
            match op {
                AttrOp::Eq => actual == want,
                AttrOp::Includes => {
                    !want.is_empty()
                        && actual
                            .split(|b| b.is_ascii_whitespace())
                            .any(|t| t == want.as_slice())
                }
                AttrOp::Dash => {
                    actual == want
                        || (actual.starts_with(&want) && actual.get(want.len()) == Some(&b'-'))
                }
                AttrOp::Prefix => !want.is_empty() && actual.starts_with(&want),
                AttrOp::Suffix => !want.is_empty() && actual.ends_with(&want),
                AttrOp::Substring => {
                    !want.is_empty()
                        && actual
                            .windows(want.len().max(1))
                            .any(|w| w == want.as_slice())
                }
            }
        }
        Simple::Not(list) => !list.iter().any(|c| matches_complex(doc, id, c, scope)),
        Simple::Is(list) => list.iter().any(|c| matches_complex(doc, id, c, scope)),
        Simple::FirstChild => nth_index(doc, id, false, false) == 1,
        Simple::LastChild => nth_index(doc, id, true, false) == 1,
        Simple::OnlyChild => {
            nth_index(doc, id, false, false) == 1 && nth_index(doc, id, true, false) == 1
        }
        Simple::FirstOfType => nth_index(doc, id, false, true) == 1,
        Simple::LastOfType => nth_index(doc, id, true, true) == 1,
        Simple::OnlyOfType => {
            nth_index(doc, id, false, true) == 1 && nth_index(doc, id, true, true) == 1
        }
        Simple::NthChild(a, b, of_type) => nth_matches(*a, *b, nth_index(doc, id, false, *of_type)),
        Simple::NthLastChild(a, b, of_type) => {
            nth_matches(*a, *b, nth_index(doc, id, true, *of_type))
        }
        Simple::Empty => doc
            .children(id)
            .iter()
            .all(|&c| matches!(doc.node(c).kind, NodeKind::Comment | NodeKind::Pi)),
        Simple::Root => n
            .parent
            .is_some_and(|p| doc.node(p).kind == NodeKind::Document),
        Simple::Checked => {
            doc.find_attr(id, b"checked").is_some() || doc.find_attr(id, b"selected").is_some()
        }
        Simple::Disabled => doc.find_attr(id, b"disabled").is_some(),
        Simple::Enabled => {
            matches!(
                n.name.as_slice(),
                b"input"
                    | b"button"
                    | b"select"
                    | b"textarea"
                    | b"optgroup"
                    | b"option"
                    | b"fieldset"
            ) && doc.find_attr(id, b"disabled").is_none()
        }
        Simple::Link => {
            matches!(n.name.as_slice(), b"a" | b"area") && doc.find_attr(id, b"href").is_some()
        }
    }
}

/// Match right to left: the rightmost compound on `id`, then the rest
/// along the combinators (descendant/subsequent backtrack).
fn matches_complex(doc: &DocData, id: NodeId, c: &Complex, scope: Option<NodeId>) -> bool {
    fn go(
        doc: &DocData,
        id: NodeId,
        parts: &[(Option<Comb>, Compound)],
        scope: Option<NodeId>,
    ) -> bool {
        let (comb, compound) = parts.last().expect("non-empty");
        if !matches_compound(doc, id, compound, scope) {
            return false;
        }
        let rest = &parts[..parts.len() - 1];
        let Some(comb) = comb else {
            return true;
        };
        match comb {
            Comb::Child => doc
                .node(id)
                .parent
                .is_some_and(|p| doc.node(p).kind == NodeKind::Element && go(doc, p, rest, scope)),
            Comb::Descendant => {
                let mut cur = doc.node(id).parent;
                while let Some(p) = cur {
                    if doc.node(p).kind != NodeKind::Element {
                        return false;
                    }
                    if go(doc, p, rest, scope) {
                        return true;
                    }
                    cur = doc.node(p).parent;
                }
                false
            }
            Comb::Next => {
                let mut cur = doc.node(id).prev;
                while let Some(p) = cur {
                    if doc.node(p).kind == NodeKind::Element {
                        return go(doc, p, rest, scope);
                    }
                    cur = doc.node(p).prev;
                }
                false
            }
            Comb::Subsequent => {
                let mut cur = doc.node(id).prev;
                while let Some(p) = cur {
                    if doc.node(p).kind == NodeKind::Element && go(doc, p, rest, scope) {
                        return true;
                    }
                    cur = doc.node(p).prev;
                }
                false
            }
        }
    }
    go(doc, id, &c.parts, scope)
}

fn matches_list(doc: &DocData, id: NodeId, list: &[Complex], scope: Option<NodeId>) -> bool {
    doc.node(id).kind == NodeKind::Element
        && list.iter().any(|c| matches_complex(doc, id, c, scope))
}

fn compile(ctx: &mut Ctx, who: &str, text: &[u8]) -> Result<Vec<Complex>, Unwind> {
    let _ = who;
    parse(text).ok_or_else(|| {
        super::dom_exception_with(
            ctx,
            DomError::Syntax,
            &format!("Invalid selector ({})", String::from_utf8_lossy(text)),
        )
    })
}

/// The matching descendants of `root`, in document order.
fn select(doc: &DocData, root: NodeId, list: &[Complex], first_only: bool) -> Vec<NodeId> {
    let mut out = Vec::new();
    for d in doc.descendants(root) {
        if d != root && matches_list(doc, d, list, Some(root)) {
            out.push(d);
            if first_only {
                break;
            }
        }
    }
    out
}

pub fn query_selector(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(this(o)?)?;
    let list = compile(ctx, "querySelector", &str_arg(args, 0))?;
    let found = {
        let d = r.doc.borrow();
        select(&d, r.id, &list, true).first().copied()
    };
    wrap_value(ctx, &r.doc, found)
}

pub fn query_selector_all(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(this(o)?)?;
    let list = compile(ctx, "querySelectorAll", &str_arg(args, 0))?;
    let found = {
        let d = r.doc.borrow();
        select(&d, r.id, &list, false)
    };
    Ok(Value::Object(super::lists::fixed_list(ctx, &r.doc, found)?))
}

pub fn closest(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(this(o)?)?;
    let list = compile(ctx, "closest", &str_arg(args, 0))?;
    let found = {
        let d = r.doc.borrow();
        let mut cur = Some(r.id);
        let mut hit = None;
        while let Some(id) = cur {
            if matches_list(&d, id, &list, None) {
                hit = Some(id);
                break;
            }
            cur = d.node(id).parent;
        }
        hit
    };
    wrap_value(ctx, &r.doc, found)
}

pub fn matches(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let r = node_ref(this(o)?)?;
    let list = compile(ctx, "matches", &str_arg(args, 0))?;
    let hit = matches_list(&r.doc.borrow(), r.id, &list, None);
    Ok(Value::Bool(hit))
}
