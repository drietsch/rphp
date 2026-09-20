//! XPath 1.0 over the tree: the lexer with the spec's disambiguation
//! rules (§3.7), a recursive-descent parser to an AST, and an evaluator
//! with the four value types, all thirteen axes, the core function
//! library and libxml's number-to-string spelling. Node-sets are kept in
//! document order without duplicates; a namespace node is not modelled
//! (php's `DOMNameSpaceNode`), the `namespace` axis is empty.

use crate::tree::{DocData, NodeId, NodeKind};

// ---- values ----------------------------------------------------------------

#[derive(Clone, Debug)]
pub enum XValue {
    NodeSet(Vec<NodeId>),
    Bool(bool),
    Number(f64),
    Str(Vec<u8>),
}

/// XPath's number → string (`1`, `1.5`, `NaN`, `Infinity`, `-0` → `0`).
pub fn number_to_string(n: f64) -> String {
    if n.is_nan() {
        return "NaN".to_string();
    }
    if n.is_infinite() {
        return if n > 0.0 {
            "Infinity".to_string()
        } else {
            "-Infinity".to_string()
        };
    }
    if n == n.trunc() && n.abs() < 1e17 {
        return format!("{}", n as i64);
    }
    let s = format!("{n}");
    s
}

fn string_to_number(s: &[u8]) -> f64 {
    let t = String::from_utf8_lossy(s);
    let t = t.trim_matches(|c: char| c == ' ' || c == '\t' || c == '\n' || c == '\r');
    if t.is_empty() {
        return f64::NAN;
    }
    // XPath's Number: optional '-', digits, optional fraction; no exponent.
    let bytes = t.as_bytes();
    let mut i = 0;
    if bytes[0] == b'-' {
        i = 1;
    }
    let mut seen_digit = false;
    let mut seen_dot = false;
    while i < bytes.len() {
        match bytes[i] {
            b'0'..=b'9' => seen_digit = true,
            b'.' if !seen_dot => seen_dot = true,
            _ => return f64::NAN,
        }
        i += 1;
    }
    if !seen_digit {
        return f64::NAN;
    }
    t.parse::<f64>().unwrap_or(f64::NAN)
}

// ---- lexer -----------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Number(f64),
    Literal(Vec<u8>),
    /// A name (possibly `prefix:local` or `prefix:*`), not followed by `(`.
    Name(Vec<u8>),
    /// A name followed by `(`.
    Func(Vec<u8>),
    /// An axis name followed by `::`.
    Axis(Vec<u8>),
    NodeType(Vec<u8>),
    Var(Vec<u8>),
    Op(&'static str),
    Star,
    At,
    Slash,
    DoubleSlash,
    Dot,
    DotDot,
    LParen,
    RParen,
    LBracket,
    RBracket,
    Comma,
    Pipe,
}

fn lex(src: &[u8]) -> Result<Vec<Tok>, String> {
    let mut toks: Vec<Tok> = Vec::new();
    let mut i = 0;
    let is_name_start = |b: u8| b.is_ascii_alphabetic() || b == b'_' || b >= 0x80;
    let is_name_char =
        |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.' || b >= 0x80;
    while i < src.len() {
        let b = src[i];
        if b.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        // Operator disambiguation: after these tokens a `*` is a name test
        // and a name is never an operator.
        let prev_is_operand = matches!(
            toks.last(),
            Some(
                Tok::Number(_)
                    | Tok::Literal(_)
                    | Tok::Name(_)
                    | Tok::Star
                    | Tok::RParen
                    | Tok::RBracket
                    | Tok::Dot
                    | Tok::DotDot
                    | Tok::Var(_)
            )
        );
        match b {
            b'(' => {
                toks.push(Tok::LParen);
                i += 1;
            }
            b')' => {
                toks.push(Tok::RParen);
                i += 1;
            }
            b'[' => {
                toks.push(Tok::LBracket);
                i += 1;
            }
            b']' => {
                toks.push(Tok::RBracket);
                i += 1;
            }
            b',' => {
                toks.push(Tok::Comma);
                i += 1;
            }
            b'|' => {
                toks.push(Tok::Pipe);
                i += 1;
            }
            b'@' => {
                toks.push(Tok::At);
                i += 1;
            }
            b'/' => {
                if src.get(i + 1) == Some(&b'/') {
                    toks.push(Tok::DoubleSlash);
                    i += 2;
                } else {
                    toks.push(Tok::Slash);
                    i += 1;
                }
            }
            b'.' => {
                if src.get(i + 1) == Some(&b'.') {
                    toks.push(Tok::DotDot);
                    i += 2;
                } else if src.get(i + 1).is_some_and(u8::is_ascii_digit) {
                    let start = i;
                    i += 1;
                    while i < src.len() && src[i].is_ascii_digit() {
                        i += 1;
                    }
                    let t = std::str::from_utf8(&src[start..i]).unwrap_or("0");
                    toks.push(Tok::Number(format!("0{t}").parse().unwrap_or(f64::NAN)));
                } else {
                    toks.push(Tok::Dot);
                    i += 1;
                }
            }
            b'*' => {
                if prev_is_operand {
                    toks.push(Tok::Op("*"));
                } else {
                    toks.push(Tok::Star);
                }
                i += 1;
            }
            b'+' => {
                toks.push(Tok::Op("+"));
                i += 1;
            }
            b'-' => {
                toks.push(Tok::Op("-"));
                i += 1;
            }
            b'=' => {
                toks.push(Tok::Op("="));
                i += 1;
            }
            b'!' if src.get(i + 1) == Some(&b'=') => {
                toks.push(Tok::Op("!="));
                i += 2;
            }
            b'<' => {
                if src.get(i + 1) == Some(&b'=') {
                    toks.push(Tok::Op("<="));
                    i += 2;
                } else {
                    toks.push(Tok::Op("<"));
                    i += 1;
                }
            }
            b'>' => {
                if src.get(i + 1) == Some(&b'=') {
                    toks.push(Tok::Op(">="));
                    i += 2;
                } else {
                    toks.push(Tok::Op(">"));
                    i += 1;
                }
            }
            b'"' | b'\'' => {
                let q = b;
                let start = i + 1;
                let mut j = start;
                while j < src.len() && src[j] != q {
                    j += 1;
                }
                if j >= src.len() {
                    return Err("Invalid expression".to_string());
                }
                toks.push(Tok::Literal(src[start..j].to_vec()));
                i = j + 1;
            }
            b'$' => {
                let start = i + 1;
                let mut j = start;
                while j < src.len() && (is_name_char(src[j]) || src[j] == b':') {
                    j += 1;
                }
                toks.push(Tok::Var(src[start..j].to_vec()));
                i = j;
            }
            b'0'..=b'9' => {
                let start = i;
                while i < src.len() && src[i].is_ascii_digit() {
                    i += 1;
                }
                if i < src.len() && src[i] == b'.' {
                    i += 1;
                    while i < src.len() && src[i].is_ascii_digit() {
                        i += 1;
                    }
                }
                let t = std::str::from_utf8(&src[start..i]).unwrap_or("0");
                toks.push(Tok::Number(t.parse().unwrap_or(f64::NAN)));
            }
            _ if is_name_start(b) => {
                let start = i;
                while i < src.len() && is_name_char(src[i]) {
                    i += 1;
                }
                // `prefix:local` or `prefix:*`.
                if i < src.len() && src[i] == b':' && src.get(i + 1) != Some(&b':') {
                    i += 1;
                    if i < src.len() && src[i] == b'*' {
                        i += 1;
                    } else {
                        while i < src.len() && is_name_char(src[i]) {
                            i += 1;
                        }
                    }
                }
                let name = src[start..i].to_vec();
                if prev_is_operand && matches!(name.as_slice(), b"and" | b"or" | b"mod" | b"div") {
                    toks.push(Tok::Op(match name.as_slice() {
                        b"and" => "and",
                        b"or" => "or",
                        b"mod" => "mod",
                        _ => "div",
                    }));
                    continue;
                }
                // Look past whitespace.
                let mut j = i;
                while j < src.len() && src[j].is_ascii_whitespace() {
                    j += 1;
                }
                if src[j..].starts_with(b"::") {
                    toks.push(Tok::Axis(name));
                    i = j + 2;
                } else if src.get(j) == Some(&b'(') {
                    if matches!(
                        name.as_slice(),
                        b"node" | b"text" | b"comment" | b"processing-instruction"
                    ) {
                        toks.push(Tok::NodeType(name));
                    } else {
                        toks.push(Tok::Func(name));
                    }
                    i = j + 1;
                } else {
                    toks.push(Tok::Name(name));
                }
            }
            _ => return Err("Invalid expression".to_string()),
        }
    }
    Ok(toks)
}

// ---- AST -------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Axis {
    Ancestor,
    AncestorOrSelf,
    Attribute,
    Child,
    Descendant,
    DescendantOrSelf,
    Following,
    FollowingSibling,
    Namespace,
    Parent,
    Preceding,
    PrecedingSibling,
    SelfAxis,
}

impl Axis {
    fn parse(name: &[u8]) -> Option<Axis> {
        Some(match name {
            b"ancestor" => Axis::Ancestor,
            b"ancestor-or-self" => Axis::AncestorOrSelf,
            b"attribute" => Axis::Attribute,
            b"child" => Axis::Child,
            b"descendant" => Axis::Descendant,
            b"descendant-or-self" => Axis::DescendantOrSelf,
            b"following" => Axis::Following,
            b"following-sibling" => Axis::FollowingSibling,
            b"namespace" => Axis::Namespace,
            b"parent" => Axis::Parent,
            b"preceding" => Axis::Preceding,
            b"preceding-sibling" => Axis::PrecedingSibling,
            b"self" => Axis::SelfAxis,
            _ => return None,
        })
    }

    fn is_reverse(self) -> bool {
        matches!(
            self,
            Axis::Ancestor | Axis::AncestorOrSelf | Axis::Preceding | Axis::PrecedingSibling
        )
    }
}

#[derive(Clone, Debug)]
enum NodeTest {
    /// `*`
    Any,
    /// `prefix:*`
    AnyInNs(Vec<u8>),
    /// `local` or `prefix:local`
    Name(Option<Vec<u8>>, Vec<u8>),
    Node,
    Text,
    Comment,
    Pi(Option<Vec<u8>>),
}

#[derive(Clone, Debug)]
struct Step {
    axis: Axis,
    test: NodeTest,
    predicates: Vec<Expr>,
}

#[derive(Clone, Debug)]
enum Expr {
    Number(f64),
    Literal(Vec<u8>),
    Var(Vec<u8>),
    Call(Vec<u8>, Vec<Expr>),
    /// A filter expression: primary with predicates.
    Filter(Box<Expr>, Vec<Expr>),
    /// `absolute`: from the root; steps applied in order.
    Path {
        absolute: bool,
        start: Option<Box<Expr>>,
        steps: Vec<Step>,
    },
    Or(Box<Expr>, Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Eq(Box<Expr>, Box<Expr>, bool),
    Rel(Box<Expr>, Box<Expr>, &'static str),
    Arith(Box<Expr>, Box<Expr>, &'static str),
    Neg(Box<Expr>),
    Union(Box<Expr>, Box<Expr>),
}

struct P {
    toks: Vec<Tok>,
    pos: usize,
}

impl P {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).cloned();
        self.pos += 1;
        t
    }

    fn eat(&mut self, t: &Tok) -> bool {
        if self.peek() == Some(t) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn eat_op(&mut self, op: &str) -> bool {
        if matches!(self.peek(), Some(Tok::Op(o)) if *o == op) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expr(&mut self) -> Result<Expr, String> {
        self.or_expr()
    }

    fn or_expr(&mut self) -> Result<Expr, String> {
        let mut l = self.and_expr()?;
        while self.eat_op("or") {
            let r = self.and_expr()?;
            l = Expr::Or(Box::new(l), Box::new(r));
        }
        Ok(l)
    }

    fn and_expr(&mut self) -> Result<Expr, String> {
        let mut l = self.equality()?;
        while self.eat_op("and") {
            let r = self.equality()?;
            l = Expr::And(Box::new(l), Box::new(r));
        }
        Ok(l)
    }

    fn equality(&mut self) -> Result<Expr, String> {
        let mut l = self.relational()?;
        loop {
            if self.eat_op("=") {
                let r = self.relational()?;
                l = Expr::Eq(Box::new(l), Box::new(r), true);
            } else if self.eat_op("!=") {
                let r = self.relational()?;
                l = Expr::Eq(Box::new(l), Box::new(r), false);
            } else {
                return Ok(l);
            }
        }
    }

    fn relational(&mut self) -> Result<Expr, String> {
        let mut l = self.additive()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Op(o @ ("<" | ">" | "<=" | ">="))) => *o,
                _ => return Ok(l),
            };
            self.pos += 1;
            let r = self.additive()?;
            l = Expr::Rel(Box::new(l), Box::new(r), op);
        }
    }

    fn additive(&mut self) -> Result<Expr, String> {
        let mut l = self.multiplicative()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Op(o @ ("+" | "-"))) => *o,
                _ => return Ok(l),
            };
            self.pos += 1;
            let r = self.multiplicative()?;
            l = Expr::Arith(Box::new(l), Box::new(r), op);
        }
    }

    fn multiplicative(&mut self) -> Result<Expr, String> {
        let mut l = self.unary()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Op(o @ ("*" | "div" | "mod"))) => *o,
                _ => return Ok(l),
            };
            self.pos += 1;
            let r = self.unary()?;
            l = Expr::Arith(Box::new(l), Box::new(r), op);
        }
    }

    fn unary(&mut self) -> Result<Expr, String> {
        if self.eat_op("-") {
            let e = self.unary()?;
            return Ok(Expr::Neg(Box::new(e)));
        }
        self.union()
    }

    fn union(&mut self) -> Result<Expr, String> {
        let mut l = self.path()?;
        while self.eat(&Tok::Pipe) {
            let r = self.path()?;
            l = Expr::Union(Box::new(l), Box::new(r));
        }
        Ok(l)
    }

    fn path(&mut self) -> Result<Expr, String> {
        match self.peek() {
            Some(Tok::Slash) => {
                self.pos += 1;
                let steps = if self.starts_step() {
                    self.relative_steps()?
                } else {
                    Vec::new()
                };
                Ok(Expr::Path {
                    absolute: true,
                    start: None,
                    steps,
                })
            }
            Some(Tok::DoubleSlash) => {
                self.pos += 1;
                let mut steps = vec![Step {
                    axis: Axis::DescendantOrSelf,
                    test: NodeTest::Node,
                    predicates: Vec::new(),
                }];
                steps.extend(self.relative_steps()?);
                Ok(Expr::Path {
                    absolute: true,
                    start: None,
                    steps,
                })
            }
            Some(Tok::Number(_) | Tok::Literal(_) | Tok::Var(_) | Tok::Func(_) | Tok::LParen) => {
                let primary = self.primary()?;
                let mut predicates = Vec::new();
                while self.peek() == Some(&Tok::LBracket) {
                    predicates.push(self.predicate()?);
                }
                let filter = if predicates.is_empty() {
                    primary
                } else {
                    Expr::Filter(Box::new(primary), predicates)
                };
                match self.peek() {
                    Some(Tok::Slash) => {
                        self.pos += 1;
                        let steps = self.relative_steps()?;
                        Ok(Expr::Path {
                            absolute: false,
                            start: Some(Box::new(filter)),
                            steps,
                        })
                    }
                    Some(Tok::DoubleSlash) => {
                        self.pos += 1;
                        let mut steps = vec![Step {
                            axis: Axis::DescendantOrSelf,
                            test: NodeTest::Node,
                            predicates: Vec::new(),
                        }];
                        steps.extend(self.relative_steps()?);
                        Ok(Expr::Path {
                            absolute: false,
                            start: Some(Box::new(filter)),
                            steps,
                        })
                    }
                    _ => Ok(filter),
                }
            }
            _ => {
                let steps = self.relative_steps()?;
                Ok(Expr::Path {
                    absolute: false,
                    start: None,
                    steps,
                })
            }
        }
    }

    fn starts_step(&self) -> bool {
        matches!(
            self.peek(),
            Some(
                Tok::Name(_)
                    | Tok::Star
                    | Tok::At
                    | Tok::Dot
                    | Tok::DotDot
                    | Tok::Axis(_)
                    | Tok::NodeType(_)
            )
        )
    }

    fn relative_steps(&mut self) -> Result<Vec<Step>, String> {
        let mut steps = vec![self.step()?];
        loop {
            match self.peek() {
                Some(Tok::Slash) => {
                    self.pos += 1;
                    steps.push(self.step()?);
                }
                Some(Tok::DoubleSlash) => {
                    self.pos += 1;
                    steps.push(Step {
                        axis: Axis::DescendantOrSelf,
                        test: NodeTest::Node,
                        predicates: Vec::new(),
                    });
                    steps.push(self.step()?);
                }
                _ => return Ok(steps),
            }
        }
    }

    fn step(&mut self) -> Result<Step, String> {
        match self.peek() {
            Some(Tok::Dot) => {
                self.pos += 1;
                return Ok(Step {
                    axis: Axis::SelfAxis,
                    test: NodeTest::Node,
                    predicates: Vec::new(),
                });
            }
            Some(Tok::DotDot) => {
                self.pos += 1;
                return Ok(Step {
                    axis: Axis::Parent,
                    test: NodeTest::Node,
                    predicates: Vec::new(),
                });
            }
            _ => {}
        }
        let axis = match self.peek() {
            Some(Tok::At) => {
                self.pos += 1;
                Axis::Attribute
            }
            Some(Tok::Axis(a)) => {
                let a = a.clone();
                self.pos += 1;
                Axis::parse(&a).ok_or_else(|| "Invalid expression".to_string())?
            }
            _ => Axis::Child,
        };
        let test = match self.next() {
            Some(Tok::Star) => NodeTest::Any,
            Some(Tok::Name(n)) => {
                if let Some(p) = n.strip_suffix(b":*") {
                    NodeTest::AnyInNs(p.to_vec())
                } else {
                    match n.iter().position(|&b| b == b':') {
                        Some(i) => NodeTest::Name(Some(n[..i].to_vec()), n[i + 1..].to_vec()),
                        None => NodeTest::Name(None, n),
                    }
                }
            }
            Some(Tok::NodeType(t)) => {
                let test = match t.as_slice() {
                    b"node" => NodeTest::Node,
                    b"text" => NodeTest::Text,
                    b"comment" => NodeTest::Comment,
                    _ => {
                        let target = match self.peek() {
                            Some(Tok::Literal(l)) => {
                                let l = l.clone();
                                self.pos += 1;
                                Some(l)
                            }
                            _ => None,
                        };
                        NodeTest::Pi(target)
                    }
                };
                if !self.eat(&Tok::RParen) {
                    return Err("Invalid expression".to_string());
                }
                test
            }
            _ => return Err("Invalid expression".to_string()),
        };
        let mut predicates = Vec::new();
        while self.peek() == Some(&Tok::LBracket) {
            predicates.push(self.predicate()?);
        }
        Ok(Step {
            axis,
            test,
            predicates,
        })
    }

    fn predicate(&mut self) -> Result<Expr, String> {
        self.pos += 1; // '['
        let e = self.expr()?;
        if !self.eat(&Tok::RBracket) {
            return Err("Invalid expression".to_string());
        }
        Ok(e)
    }

    fn primary(&mut self) -> Result<Expr, String> {
        match self.next() {
            Some(Tok::Number(n)) => Ok(Expr::Number(n)),
            Some(Tok::Literal(l)) => Ok(Expr::Literal(l)),
            Some(Tok::Var(v)) => Ok(Expr::Var(v)),
            Some(Tok::LParen) => {
                let e = self.expr()?;
                if !self.eat(&Tok::RParen) {
                    return Err("Invalid expression".to_string());
                }
                Ok(e)
            }
            Some(Tok::Func(name)) => {
                let mut args = Vec::new();
                if !self.eat(&Tok::RParen) {
                    loop {
                        if self.peek().is_none() {
                            // libxml compiles `f(` and fails at evaluation
                            // (an unknown `f` reports itself first).
                            break;
                        }
                        args.push(self.expr()?);
                        if self.eat(&Tok::Comma) {
                            continue;
                        }
                        if self.eat(&Tok::RParen) {
                            break;
                        }
                        if self.peek().is_none() {
                            break;
                        }
                        return Err("Invalid expression".to_string());
                    }
                }
                Ok(Expr::Call(name, args))
            }
            _ => Err("Invalid expression".to_string()),
        }
    }
}

/// A compiled expression.
pub struct Compiled {
    root: Expr,
}

impl Compiled {
    /// Every namespace prefix a name test uses must be bound — libxml
    /// refuses the expression otherwise ("Undefined namespace prefix").
    pub fn check_prefixes(&self, host: &dyn Host) -> Result<(), String> {
        fn walk(e: &Expr, host: &dyn Host) -> Result<(), String> {
            match e {
                Expr::Number(_) | Expr::Literal(_) | Expr::Var(_) => Ok(()),
                Expr::Call(_, args) => args.iter().try_for_each(|a| walk(a, host)),
                Expr::Filter(p, preds) => {
                    walk(p, host)?;
                    preds.iter().try_for_each(|a| walk(a, host))
                }
                Expr::Path { start, steps, .. } => {
                    if let Some(s) = start {
                        walk(s, host)?;
                    }
                    for st in steps {
                        let prefix = match &st.test {
                            NodeTest::AnyInNs(p) => Some(p),
                            NodeTest::Name(Some(p), _) => Some(p),
                            _ => None,
                        };
                        if let Some(p) = prefix {
                            if host.namespace(p).is_none() {
                                return Err("Undefined namespace prefix".to_string());
                            }
                        }
                        st.predicates.iter().try_for_each(|a| walk(a, host))?;
                    }
                    Ok(())
                }
                Expr::Or(a, b)
                | Expr::And(a, b)
                | Expr::Eq(a, b, _)
                | Expr::Rel(a, b, _)
                | Expr::Arith(a, b, _)
                | Expr::Union(a, b) => {
                    walk(a, host)?;
                    walk(b, host)
                }
                Expr::Neg(a) => walk(a, host),
            }
        }
        walk(&self.root, host)
    }
}

pub fn compile(src: &[u8]) -> Result<Compiled, String> {
    let toks = lex(src)?;
    if toks.is_empty() {
        return Err("Invalid expression".to_string());
    }
    let mut p = P { toks, pos: 0 };
    let root = p.expr()?;
    if p.pos < p.toks.len() {
        return Err("Invalid expression".to_string());
    }
    Ok(Compiled { root })
}

// ---- evaluation ------------------------------------------------------------

/// What the host supplies: namespace prefixes, variables, and the
/// extension functions (`php:function`).
pub trait Host {
    fn namespace(&self, prefix: &[u8]) -> Option<Vec<u8>>;
    fn variable(&self, _name: &[u8]) -> Option<XValue> {
        None
    }
    /// A function outside the core library: `ns` is the resolved prefix's
    /// URI (`None` for an unprefixed unknown name).
    fn call(
        &mut self,
        doc: &DocData,
        ns: Option<&[u8]>,
        name: &[u8],
        args: Vec<XValue>,
    ) -> Result<XValue, String>;
}

pub struct Context {
    pub node: NodeId,
    pub position: usize,
    pub size: usize,
    /// Whether `position`/`size` are meaningful (not at the top level).
    pub in_predicate: bool,
}

pub struct Eval<'a, H: Host> {
    pub doc: &'a DocData,
    pub host: &'a mut H,
    /// Document order index of every node, for sorting node-sets.
    order: Vec<usize>,
}

impl<'a, H: Host> Eval<'a, H> {
    pub fn new(doc: &'a DocData, host: &'a mut H) -> Self {
        let mut order = vec![usize::MAX; doc.nodes.len()];
        let mut n = 0;
        Self::number(doc, crate::tree::DOCUMENT, &mut order, &mut n);
        Eval { doc, host, order }
    }

    fn number(doc: &DocData, id: NodeId, order: &mut Vec<usize>, n: &mut usize) {
        order[id] = *n;
        *n += 1;
        for &a in &doc.node(id).attrs {
            order[a] = *n;
            *n += 1;
        }
        for c in doc.children(id) {
            Self::number(doc, c, order, n);
        }
    }

    fn doc_order(&self, id: NodeId) -> usize {
        // A node outside the tree (detached) sorts by id after everything.
        let o = self.order.get(id).copied().unwrap_or(usize::MAX);
        if o == usize::MAX {
            usize::MAX / 2 + id
        } else {
            o
        }
    }

    fn sort_dedup(&self, mut set: Vec<NodeId>) -> Vec<NodeId> {
        set.sort_by_key(|&id| self.doc_order(id));
        set.dedup();
        set
    }

    pub fn evaluate(&mut self, c: &Compiled, ctx: &Context) -> Result<XValue, String> {
        self.eval(&c.root, ctx)
    }

    fn eval(&mut self, e: &Expr, ctx: &Context) -> Result<XValue, String> {
        Ok(match e {
            Expr::Number(n) => XValue::Number(*n),
            Expr::Literal(l) => XValue::Str(l.clone()),
            Expr::Var(v) => self
                .host
                .variable(v)
                .ok_or_else(|| "Undefined variable".to_string())?,
            Expr::Call(name, args) => return self.call(name, args, ctx),
            Expr::Filter(primary, predicates) => {
                let v = self.eval(primary, ctx)?;
                let XValue::NodeSet(set) = v else {
                    return Err("Invalid type".to_string());
                };
                let mut set = self.sort_dedup(set);
                for p in predicates {
                    set = self.filter(set, p, false)?;
                }
                XValue::NodeSet(set)
            }
            Expr::Path {
                absolute,
                start,
                steps,
            } => {
                let mut set: Vec<NodeId> = if *absolute {
                    vec![crate::tree::DOCUMENT]
                } else if let Some(s) = start {
                    match self.eval(s, ctx)? {
                        XValue::NodeSet(set) => self.sort_dedup(set),
                        _ => return Err("Invalid type".to_string()),
                    }
                } else {
                    vec![ctx.node]
                };
                for step in steps {
                    set = self.step(&set, step)?;
                }
                XValue::NodeSet(set)
            }
            Expr::Or(a, b) => {
                let l = self.eval(a, ctx)?;
                if self.to_bool(&l) {
                    return Ok(XValue::Bool(true));
                }
                let r = self.eval(b, ctx)?;
                XValue::Bool(self.to_bool(&r))
            }
            Expr::And(a, b) => {
                let l = self.eval(a, ctx)?;
                if !self.to_bool(&l) {
                    return Ok(XValue::Bool(false));
                }
                let r = self.eval(b, ctx)?;
                XValue::Bool(self.to_bool(&r))
            }
            Expr::Eq(a, b, equal) => {
                let l = self.eval(a, ctx)?;
                let r = self.eval(b, ctx)?;
                XValue::Bool(self.compare_eq(&l, &r, *equal))
            }
            Expr::Rel(a, b, op) => {
                let l = self.eval(a, ctx)?;
                let r = self.eval(b, ctx)?;
                XValue::Bool(self.compare_rel(&l, &r, op))
            }
            Expr::Arith(a, b, op) => {
                let l = self.eval(a, ctx)?;
                let r = self.eval(b, ctx)?;
                let (x, y) = (self.to_number(&l), self.to_number(&r));
                XValue::Number(match *op {
                    "+" => x + y,
                    "-" => x - y,
                    "*" => x * y,
                    "div" => x / y,
                    _ => x % y,
                })
            }
            Expr::Neg(a) => {
                let v = self.eval(a, ctx)?;
                XValue::Number(-self.to_number(&v))
            }
            Expr::Union(a, b) => {
                let l = self.eval(a, ctx)?;
                let r = self.eval(b, ctx)?;
                match (l, r) {
                    (XValue::NodeSet(mut x), XValue::NodeSet(y)) => {
                        x.extend(y);
                        XValue::NodeSet(self.sort_dedup(x))
                    }
                    _ => return Err("Invalid type".to_string()),
                }
            }
        })
    }

    /// The nodes `step` selects from each node of `input`, in document
    /// order, predicates applied per input node.
    fn step(&mut self, input: &[NodeId], step: &Step) -> Result<Vec<NodeId>, String> {
        let mut out: Vec<NodeId> = Vec::new();
        for &n in input {
            let candidates: Vec<NodeId> = self
                .axis(n, step.axis)
                .into_iter()
                .filter(|&c| self.matches(c, &step.test, step.axis))
                .collect();
            let mut set = candidates;
            for p in &step.predicates {
                set = self.filter(set, p, step.axis.is_reverse())?;
            }
            out.extend(set);
        }
        Ok(self.sort_dedup(out))
    }

    /// Keep the nodes of `set` (in axis order) whose predicate holds; a
    /// numeric predicate is a position test.
    fn filter(
        &mut self,
        set: Vec<NodeId>,
        pred: &Expr,
        reverse: bool,
    ) -> Result<Vec<NodeId>, String> {
        let size = set.len();
        let mut out = Vec::new();
        let ordered: Vec<NodeId> = if reverse {
            set.iter().rev().copied().collect()
        } else {
            set.clone()
        };
        for (i, &n) in ordered.iter().enumerate() {
            let ctx = Context {
                node: n,
                position: i + 1,
                size,
                in_predicate: true,
            };
            let v = self.eval(pred, &ctx)?;
            let keep = match v {
                XValue::Number(p) => p == (i + 1) as f64,
                other => self.to_bool(&other),
            };
            if keep {
                out.push(n);
            }
        }
        Ok(out)
    }

    fn axis(&self, n: NodeId, axis: Axis) -> Vec<NodeId> {
        let d = self.doc;
        let is_attr = d.node(n).kind == NodeKind::Attribute;
        match axis {
            Axis::SelfAxis => vec![n],
            Axis::Child => d.children(n),
            Axis::Parent => d.node(n).parent.into_iter().collect(),
            Axis::Attribute => d.node(n).attrs.clone(),
            Axis::Namespace => Vec::new(),
            Axis::Descendant => d.descendants(n).into_iter().skip(1).collect(),
            Axis::DescendantOrSelf => d.descendants(n),
            Axis::Ancestor => {
                let mut out = Vec::new();
                let mut cur = d.node(n).parent;
                while let Some(p) = cur {
                    out.push(p);
                    cur = d.node(p).parent;
                }
                out.reverse();
                out
            }
            Axis::AncestorOrSelf => {
                let mut out = vec![n];
                let mut cur = d.node(n).parent;
                while let Some(p) = cur {
                    out.push(p);
                    cur = d.node(p).parent;
                }
                out.reverse();
                out
            }
            Axis::FollowingSibling => {
                if is_attr {
                    return Vec::new();
                }
                let mut out = Vec::new();
                let mut cur = d.node(n).next;
                while let Some(s) = cur {
                    out.push(s);
                    cur = d.node(s).next;
                }
                out
            }
            Axis::PrecedingSibling => {
                if is_attr {
                    return Vec::new();
                }
                let mut out = Vec::new();
                let mut cur = d.node(n).prev;
                while let Some(s) = cur {
                    out.push(s);
                    cur = d.node(s).prev;
                }
                out.reverse();
                out
            }
            Axis::Following => {
                // Everything after `n` in document order that is not a
                // descendant (attributes excluded).
                let all = d.descendants(crate::tree::DOCUMENT);
                let start = if is_attr {
                    d.node(n).parent.unwrap_or(n)
                } else {
                    n
                };
                let pos = all.iter().position(|&x| x == start).unwrap_or(all.len());
                all[pos + 1..]
                    .iter()
                    .copied()
                    .filter(|&x| !d.is_ancestor(start, x))
                    .collect()
            }
            Axis::Preceding => {
                let all = d.descendants(crate::tree::DOCUMENT);
                let start = if is_attr {
                    d.node(n).parent.unwrap_or(n)
                } else {
                    n
                };
                let pos = all.iter().position(|&x| x == start).unwrap_or(0);
                all[..pos]
                    .iter()
                    .copied()
                    .filter(|&x| !d.is_ancestor(x, start))
                    .collect()
            }
        }
    }

    fn matches(&self, n: NodeId, test: &NodeTest, axis: Axis) -> bool {
        let node = self.doc.node(n);
        let principal = match axis {
            Axis::Attribute => NodeKind::Attribute,
            _ => NodeKind::Element,
        };
        match test {
            NodeTest::Node => true,
            NodeTest::Text => matches!(node.kind, NodeKind::Text | NodeKind::CData),
            NodeTest::Comment => node.kind == NodeKind::Comment,
            NodeTest::Pi(target) => {
                node.kind == NodeKind::Pi && target.as_ref().is_none_or(|t| node.name == *t)
            }
            NodeTest::Any => node.kind == principal,
            NodeTest::AnyInNs(prefix) => {
                node.kind == principal
                    && self
                        .host
                        .namespace(prefix)
                        .is_some_and(|uri| node.ns.as_deref() == Some(uri.as_slice()))
            }
            NodeTest::Name(prefix, local) => {
                if node.kind != principal || node.name != *local {
                    return false;
                }
                match prefix {
                    Some(p) => self
                        .host
                        .namespace(p)
                        .is_some_and(|uri| node.ns.as_deref() == Some(uri.as_slice())),
                    None => node.ns.is_none(),
                }
            }
        }
    }

    // ---- conversions ------------------------------------------------------

    pub fn string_value(&self, n: NodeId) -> Vec<u8> {
        self.doc.text_content(n).unwrap_or_default()
    }

    pub fn to_string(&self, v: &XValue) -> Vec<u8> {
        match v {
            XValue::NodeSet(set) => set
                .first()
                .map(|&n| self.string_value(n))
                .unwrap_or_default(),
            XValue::Bool(b) => {
                if *b {
                    b"true".to_vec()
                } else {
                    b"false".to_vec()
                }
            }
            XValue::Number(n) => number_to_string(*n).into_bytes(),
            XValue::Str(s) => s.clone(),
        }
    }

    pub fn to_number(&self, v: &XValue) -> f64 {
        match v {
            XValue::Number(n) => *n,
            XValue::Bool(b) => f64::from(u8::from(*b)),
            XValue::Str(s) => string_to_number(s),
            XValue::NodeSet(_) => string_to_number(&self.to_string(v)),
        }
    }

    pub fn to_bool(&self, v: &XValue) -> bool {
        match v {
            XValue::NodeSet(set) => !set.is_empty(),
            XValue::Bool(b) => *b,
            XValue::Number(n) => *n != 0.0 && !n.is_nan(),
            XValue::Str(s) => !s.is_empty(),
        }
    }

    fn compare_eq(&self, l: &XValue, r: &XValue, equal: bool) -> bool {
        match (l, r) {
            (XValue::NodeSet(a), XValue::NodeSet(b)) => {
                let bs: Vec<Vec<u8>> = b.iter().map(|&n| self.string_value(n)).collect();
                a.iter().any(|&x| {
                    let sx = self.string_value(x);
                    bs.iter().any(|sy| (sx == *sy) == equal)
                })
            }
            (XValue::NodeSet(a), other) | (other, XValue::NodeSet(a)) => a.iter().any(|&n| {
                let sv = self.string_value(n);
                let r = match other {
                    XValue::Number(x) => string_to_number(&sv) == *x,
                    XValue::Bool(b) => !a.is_empty() == *b,
                    XValue::Str(s) => sv == *s,
                    XValue::NodeSet(_) => unreachable!(),
                };
                r == equal
            }),
            (XValue::Bool(_), _) | (_, XValue::Bool(_)) => {
                (self.to_bool(l) == self.to_bool(r)) == equal
            }
            (XValue::Number(_), _) | (_, XValue::Number(_)) => {
                (self.to_number(l) == self.to_number(r)) == equal
            }
            (XValue::Str(a), XValue::Str(b)) => (a == b) == equal,
        }
    }

    fn compare_rel(&self, l: &XValue, r: &XValue, op: &str) -> bool {
        let cmp = |x: f64, y: f64| match op {
            "<" => x < y,
            ">" => x > y,
            "<=" => x <= y,
            _ => x >= y,
        };
        match (l, r) {
            (XValue::NodeSet(a), XValue::NodeSet(b)) => a.iter().any(|&x| {
                let nx = string_to_number(&self.string_value(x));
                b.iter()
                    .any(|&y| cmp(nx, string_to_number(&self.string_value(y))))
            }),
            (XValue::NodeSet(a), other) => {
                let y = self.to_number(other);
                a.iter()
                    .any(|&x| cmp(string_to_number(&self.string_value(x)), y))
            }
            (other, XValue::NodeSet(b)) => {
                let x = self.to_number(other);
                b.iter()
                    .any(|&y| cmp(x, string_to_number(&self.string_value(y))))
            }
            _ => cmp(self.to_number(l), self.to_number(r)),
        }
    }

    // ---- functions --------------------------------------------------------

    fn call(&mut self, name: &[u8], args: &[Expr], ctx: &Context) -> Result<XValue, String> {
        let mut vals = Vec::with_capacity(args.len());
        for a in args {
            vals.push(self.eval(a, ctx)?);
        }
        let s = |v: &XValue, me: &Self| me.to_string(v);
        Ok(match name {
            b"last" => {
                if !ctx.in_predicate {
                    return Err("Invalid context size".to_string());
                }
                XValue::Number(ctx.size as f64)
            }
            b"position" => {
                if !ctx.in_predicate {
                    return Err("Invalid context position".to_string());
                }
                XValue::Number(ctx.position as f64)
            }
            b"count" => match vals.first() {
                Some(XValue::NodeSet(set)) => XValue::Number(set.len() as f64),
                _ => return Err("Invalid type".to_string()),
            },
            b"id" => {
                // Without DTD-declared IDs nothing has one (php's answer).
                let wanted = match vals.first() {
                    Some(v) => s(v, self),
                    None => return Err("Invalid number of arguments".to_string()),
                };
                let ids: Vec<Vec<u8>> = wanted
                    .split(|b| b.is_ascii_whitespace())
                    .filter(|w| !w.is_empty())
                    .map(<[u8]>::to_vec)
                    .collect();
                let d = self.doc;
                let found: Vec<NodeId> = d
                    .descendants(crate::tree::DOCUMENT)
                    .into_iter()
                    .filter(|&n| {
                        let node = d.node(n);
                        node.kind == NodeKind::Element
                            && node.attrs.iter().any(|&a| {
                                let an = d.node(a);
                                (an.is_id
                                    || (an.name == b"id" && an.prefix.as_deref() == Some(b"xml")))
                                    && ids.contains(&d.attr_value(a))
                            })
                    })
                    .collect();
                XValue::NodeSet(found)
            }
            b"local-name" | b"name" | b"namespace-uri" => {
                let node = match vals.first() {
                    Some(XValue::NodeSet(set)) => set.first().copied(),
                    Some(_) => return Err("Invalid type".to_string()),
                    None => Some(ctx.node),
                };
                let Some(n) = node else {
                    return Ok(XValue::Str(Vec::new()));
                };
                let nd = self.doc.node(n);
                XValue::Str(match name {
                    b"local-name" => match nd.kind {
                        NodeKind::Element | NodeKind::Attribute => nd.name.clone(),
                        NodeKind::Pi => nd.name.clone(),
                        _ => Vec::new(),
                    },
                    b"name" => match nd.kind {
                        NodeKind::Element | NodeKind::Attribute => nd.qualified_name(),
                        NodeKind::Pi => nd.name.clone(),
                        _ => Vec::new(),
                    },
                    _ => nd.ns.clone().unwrap_or_default(),
                })
            }
            b"string" => XValue::Str(match vals.first() {
                Some(v) => s(v, self),
                None => self.string_value(ctx.node),
            }),
            b"concat" => {
                let mut out = Vec::new();
                for v in &vals {
                    out.extend_from_slice(&s(v, self));
                }
                XValue::Str(out)
            }
            b"starts-with" => {
                let (a, b) = two(&vals)?;
                XValue::Bool(s(a, self).starts_with(&s(b, self)))
            }
            b"contains" => {
                let (a, b) = two(&vals)?;
                let (x, y) = (s(a, self), s(b, self));
                XValue::Bool(y.is_empty() || x.windows(y.len()).any(|w| w == y.as_slice()))
            }
            b"substring-before" => {
                let (a, b) = two(&vals)?;
                let (x, y) = (s(a, self), s(b, self));
                XValue::Str(match find_sub(&x, &y) {
                    Some(i) => x[..i].to_vec(),
                    None => Vec::new(),
                })
            }
            b"substring-after" => {
                let (a, b) = two(&vals)?;
                let (x, y) = (s(a, self), s(b, self));
                XValue::Str(match find_sub(&x, &y) {
                    Some(i) => x[i + y.len()..].to_vec(),
                    None => Vec::new(),
                })
            }
            b"substring" => {
                let Some(first) = vals.first() else {
                    return Err("Invalid number of arguments".to_string());
                };
                let text = String::from_utf8_lossy(&s(first, self)).into_owned();
                let chars: Vec<char> = text.chars().collect();
                let start = vals.get(1).map(|v| self.to_number(v)).unwrap_or(f64::NAN);
                let start_r = xround(start);
                let end_r = match vals.get(2) {
                    Some(len) => start_r + xround(self.to_number(len)),
                    None => f64::INFINITY,
                };
                if start_r.is_nan() || end_r.is_nan() {
                    return Ok(XValue::Str(Vec::new()));
                }
                let out: String = chars
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| {
                        let p = (*i + 1) as f64;
                        p >= start_r && p < end_r
                    })
                    .map(|(_, c)| *c)
                    .collect();
                XValue::Str(out.into_bytes())
            }
            b"string-length" => {
                let text = match vals.first() {
                    Some(v) => s(v, self),
                    None => self.string_value(ctx.node),
                };
                XValue::Number(String::from_utf8_lossy(&text).chars().count() as f64)
            }
            b"normalize-space" => {
                let text = match vals.first() {
                    Some(v) => s(v, self),
                    None => self.string_value(ctx.node),
                };
                let t = String::from_utf8_lossy(&text);
                let words: Vec<&str> = t
                    .split(|c: char| c == ' ' || c == '\t' || c == '\n' || c == '\r')
                    .filter(|w| !w.is_empty())
                    .collect();
                XValue::Str(words.join(" ").into_bytes())
            }
            b"translate" => {
                if vals.len() < 3 {
                    return Err("Invalid number of arguments".to_string());
                }
                let text = String::from_utf8_lossy(&s(&vals[0], self)).into_owned();
                let from: Vec<char> = String::from_utf8_lossy(&s(&vals[1], self))
                    .chars()
                    .collect();
                let to: Vec<char> = String::from_utf8_lossy(&s(&vals[2], self))
                    .chars()
                    .collect();
                let out: String = text
                    .chars()
                    .filter_map(|c| match from.iter().position(|&f| f == c) {
                        Some(i) => to.get(i).copied(),
                        None => Some(c),
                    })
                    .collect();
                XValue::Str(out.into_bytes())
            }
            b"boolean" => match vals.first() {
                Some(v) => XValue::Bool(self.to_bool(v)),
                None => return Err("Invalid number of arguments".to_string()),
            },
            b"not" => match vals.first() {
                Some(v) => XValue::Bool(!self.to_bool(v)),
                None => return Err("Invalid number of arguments".to_string()),
            },
            b"true" => XValue::Bool(true),
            b"false" => XValue::Bool(false),
            b"lang" => {
                let want = s(vals.first().ok_or("Invalid number of arguments")?, self)
                    .to_ascii_lowercase();
                let d = self.doc;
                let mut cur = Some(ctx.node);
                let mut lang: Option<Vec<u8>> = None;
                while let Some(n) = cur {
                    if let Some(a) = d.find_attr(n, b"xml:lang") {
                        lang = Some(d.attr_value(a).to_ascii_lowercase());
                        break;
                    }
                    cur = d.node(n).parent;
                }
                XValue::Bool(
                    lang.is_some_and(|l| {
                        l == want || l.starts_with(&[want.as_slice(), b"-"].concat())
                    }),
                )
            }
            b"number" => XValue::Number(match vals.first() {
                Some(v) => self.to_number(v),
                None => string_to_number(&self.string_value(ctx.node)),
            }),
            b"sum" => match vals.first() {
                Some(XValue::NodeSet(set)) => XValue::Number(
                    set.iter()
                        .map(|&n| string_to_number(&self.string_value(n)))
                        .sum(),
                ),
                _ => return Err("Invalid type".to_string()),
            },
            b"floor" => XValue::Number(
                self.to_number(vals.first().ok_or("Invalid number of arguments")?)
                    .floor(),
            ),
            b"ceiling" => XValue::Number(
                self.to_number(vals.first().ok_or("Invalid number of arguments")?)
                    .ceil(),
            ),
            b"round" => XValue::Number(xround(
                self.to_number(vals.first().ok_or("Invalid number of arguments")?),
            )),
            _ => {
                // A prefixed name resolves through the host (`php:function`).
                let (ns, local) = match name.iter().position(|&b| b == b':') {
                    Some(i) => {
                        let prefix = &name[..i];
                        let Some(uri) = self.host.namespace(prefix) else {
                            return Err(format!(
                                "xmlXPathCompOpEval: function {} bound to undefined prefix {}",
                                String::from_utf8_lossy(&name[i + 1..]),
                                String::from_utf8_lossy(prefix)
                            ));
                        };
                        (Some(uri), name[i + 1..].to_vec())
                    }
                    None => (None, name.to_vec()),
                };
                let doc = self.doc;
                return self.host.call(doc, ns.as_deref(), &local, vals);
            }
        })
    }
}

fn two(vals: &[XValue]) -> Result<(&XValue, &XValue), String> {
    match (vals.first(), vals.get(1)) {
        (Some(a), Some(b)) => Ok((a, b)),
        _ => Err("Invalid number of arguments".to_string()),
    }
}

fn find_sub(h: &[u8], n: &[u8]) -> Option<usize> {
    if n.is_empty() {
        return Some(0);
    }
    h.windows(n.len()).position(|w| w == n)
}

/// XPath `round()`: half away from... no — half towards +∞ (`round(-2.5)`
/// is `-2`).
fn xround(n: f64) -> f64 {
    if n.is_nan() || n.is_infinite() {
        return n;
    }
    (n + 0.5).floor()
}
