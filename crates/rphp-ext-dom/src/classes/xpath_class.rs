//! `DOMXPath`: the php surface over the XPath engine — registered
//! namespaces, the context node's own namespaces, `php:function()` /
//! `php:functionString()` and php 8.4's `registerPhpFunctionNS()`.

use std::collections::HashMap;

use rphp_runtime::{nm, Ctx, Interp, NativeProps, NativeResult, Registry, Unwind};
use rphp_value::{Array, Object, Payload, Value};

use super::{node_arg, str_arg, this, wrap};
use crate::libxml;
use crate::parser::XmlError;
use crate::tree::{Doc, DocData, NodeKind, DOCUMENT};
use crate::xpath::{self, Context, Eval, Host, XValue};

const PHP_NS: &[u8] = b"http://php.net/xpath";

#[derive(Clone)]
pub struct XPathState {
    pub doc: Doc,
    pub document: Object,
    pub namespaces: Vec<(Vec<u8>, Vec<u8>)>,
    /// `registerPhpFunctions()`: `None` = not enabled, `Some(empty)` =
    /// every function, else the allowed names.
    pub php_functions: Option<Vec<Vec<u8>>>,
    /// `registerPhpFunctionNS()`: (namespace, name) → callable.
    pub ns_functions: Vec<(Vec<u8>, Vec<u8>, Value)>,
    pub register_node_ns: bool,
}

fn state(o: &Object) -> Result<XPathState, Unwind> {
    o.with_payload::<XPathState, _>(|s| s.clone())
        .ok_or_else(|| Unwind::error("Invalid XPath object"))
}

fn update(o: &Object, f: impl FnOnce(&mut XPathState)) {
    o.with_payload::<XPathState, _>(f);
}

fn construct(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let Some(docref) = node_arg(&args[0]) else {
        return Err(Unwind::type_error(
            "DOMXPath::__construct(): Argument #1 ($document) must be of type DOMDocument",
        ));
    };
    let Value::Object(document) = args[0].deref().into_owned() else {
        unreachable!()
    };
    o.set_payload(Payload::Native(Box::new(XPathState {
        doc: docref.doc.clone(),
        document,
        namespaces: Vec::new(),
        php_functions: None,
        ns_functions: Vec::new(),
        register_node_ns: args.get(1).is_none_or(Value::to_bool),
    })));
    Ok(Value::Null)
}

fn get_prop(_: &mut Interp, o: &Object, name: &[u8]) -> Option<Result<Value, Unwind>> {
    let st = state(o).ok()?;
    match name {
        b"document" => Some(Ok(Value::Object(st.document))),
        b"registerNodeNamespaces" => Some(Ok(Value::Bool(st.register_node_ns))),
        _ => None,
    }
}

fn set_prop(it: &mut Interp, o: &Object, name: &[u8], v: Value) -> Option<Result<(), Unwind>> {
    match name {
        b"registerNodeNamespaces" => {
            update(o, |s| s.register_node_ns = v.to_bool());
            Some(Ok(()))
        }
        b"document" => Some(Err(Unwind::error(format!(
            "Cannot modify readonly property {}::$document",
            it.class_name_of(o)
        )))),
        _ => None,
    }
}

fn register_namespace(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let prefix = str_arg(args, 0);
    let uri = str_arg(args, 1);
    update(this(o)?, |s| {
        s.namespaces.retain(|(p, _)| *p != prefix);
        s.namespaces.push((prefix, uri));
    });
    Ok(Value::Bool(true))
}

fn register_php_functions(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let restrict: Option<Vec<Vec<u8>>> = match args.first().map(|v| v.deref().into_owned()) {
        None | Some(Value::Null) => Some(Vec::new()),
        Some(Value::Array(a)) => Some(a.values().map(|v| v.deref().to_php_bytes()).collect()),
        Some(Value::Str(s)) => Some(vec![s.as_bytes().to_vec()]),
        Some(other) => {
            return Err(Unwind::type_error(format!(
                "DOMXPath::registerPhpFunctions(): Argument #1 ($restrict) must be of type array|string|null, {} given",
                other.type_name()
            )))
        }
    };
    let _ = ctx;
    update(this(o)?, |s| match (&mut s.php_functions, restrict) {
        (Some(existing), Some(new)) if !existing.is_empty() && !new.is_empty() => {
            existing.extend(new)
        }
        (slot, new) => *slot = new,
    });
    Ok(Value::Null)
}

fn register_php_function_ns(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let ns = str_arg(args, 0);
    let name = str_arg(args, 1);
    let callable = args[2].deref().into_owned();
    if ns == PHP_NS {
        return Err(Unwind::value_error(
            "DOMXPath::registerPhpFunctionNS(): Argument #1 ($namespaceURI) must not be \"http://php.net/xpath\" because it is reserved for PHP",
        ));
    }
    update(this(o)?, |s| {
        s.ns_functions.retain(|(n, f, _)| !(*n == ns && *f == name));
        s.ns_functions.push((ns, name, callable));
    });
    Ok(Value::Null)
}

/// `DOMXPath::quote()`: the shortest XPath string literal for `s`.
fn quote(_: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let s = str_arg(args, 0);
    let mut out = Vec::new();
    if !s.contains(&b'\'') {
        out.push(b'\'');
        out.extend_from_slice(&s);
        out.push(b'\'');
    } else if !s.contains(&b'"') {
        out.push(b'"');
        out.extend_from_slice(&s);
        out.push(b'"');
    } else {
        // concat('a', "'", 'b')
        out.extend_from_slice(b"concat(");
        let mut first = true;
        for part in s.split(|&b| b == b'\'') {
            if !first {
                out.extend_from_slice(b", \"'\", ");
            }
            first = false;
            out.push(b'\'');
            out.extend_from_slice(part);
            out.push(b'\'');
        }
        out.push(b')');
    }
    Ok(Value::string(&out))
}

// ---- evaluation ------------------------------------------------------------

struct PhpHost<'a, 'b> {
    ctx: &'a mut Ctx<'b>,
    namespaces: HashMap<Vec<u8>, Vec<u8>>,
    php_functions: Option<Vec<Vec<u8>>>,
    ns_functions: Vec<(Vec<u8>, Vec<u8>, Value)>,
    doc: Doc,
    /// An exception a callback raised, to surface after the evaluation.
    unwind: Option<Unwind>,
}

impl Host for PhpHost<'_, '_> {
    fn namespace(&self, prefix: &[u8]) -> Option<Vec<u8>> {
        self.namespaces.get(prefix).cloned()
    }

    fn call(
        &mut self,
        doc: &DocData,
        ns: Option<&[u8]>,
        name: &[u8],
        args: Vec<XValue>,
    ) -> Result<XValue, String> {
        let _ = doc;
        if ns == Some(PHP_NS) {
            let as_string = match name {
                b"function" => false,
                b"functionString" => true,
                _ => {
                    return Err(format!(
                        "xmlXPathCompOpEval: function {} not found",
                        String::from_utf8_lossy(name)
                    ))
                }
            };
            let Some(allowed) = &self.php_functions else {
                return Err("Unregistered function".to_string());
            };
            let mut it = args.into_iter();
            let Some(XValue::Str(fname)) = it.next() else {
                return Err("Handler name must be a string".to_string());
            };
            if !allowed.is_empty() && !allowed.contains(&fname) {
                self.unwind = Some(Unwind::error(format!(
                    "No callback handler \"{}\" registered",
                    String::from_utf8_lossy(&fname)
                )));
                return Err("callback".to_string());
            }
            let callee = Value::string(&fname);
            if !self.ctx.is_callable(&callee) {
                self.unwind = Some(Unwind::error(format!(
                    "Invalid callback {}, function \"{}\" not found or invalid function name",
                    String::from_utf8_lossy(&fname),
                    String::from_utf8_lossy(&fname)
                )));
                return Err("callback".to_string());
            }
            let mut php_args = Vec::new();
            for a in it {
                match self.to_php(a, as_string) {
                    Ok(v) => php_args.push(v),
                    Err(u) => {
                        self.unwind = Some(u);
                        return Err("callback".to_string());
                    }
                }
            }
            return self.invoke(&callee, php_args);
        }
        let found = self
            .ns_functions
            .iter()
            .find(|(n, f, _)| Some(n.as_slice()) == ns && f == name)
            .map(|(_, _, c)| c.clone());
        let Some(callable) = found else {
            return Err(format!(
                "xmlXPathCompOpEval: function {} not found",
                String::from_utf8_lossy(name)
            ));
        };
        let mut php_args = Vec::new();
        for a in args {
            match self.to_php(a, false) {
                Ok(v) => php_args.push(v),
                Err(u) => {
                    self.unwind = Some(u);
                    return Err("callback".to_string());
                }
            }
        }
        self.invoke(&callable, php_args)
    }
}

impl PhpHost<'_, '_> {
    fn to_php(&mut self, v: XValue, as_string: bool) -> Result<Value, Unwind> {
        Ok(match v {
            XValue::Bool(b) => Value::Bool(b),
            XValue::Number(n) => Value::Float(n),
            XValue::Str(s) => Value::string(&s),
            XValue::NodeSet(set) => {
                if as_string {
                    let d = self.doc.borrow();
                    Value::string(
                        &set.first()
                            .map(|&n| d.text_content(n).unwrap_or_default())
                            .unwrap_or_default(),
                    )
                } else {
                    let mut arr = Array::new();
                    for n in set {
                        arr.push(Value::Object(wrap(self.ctx, &self.doc, n)?));
                    }
                    Value::Array(arr)
                }
            }
        })
    }

    fn invoke(&mut self, callee: &Value, args: Vec<Value>) -> Result<XValue, String> {
        match self.ctx.call_value(callee, &args) {
            Ok(v) => match v {
                Value::Bool(b) => Ok(XValue::Bool(b)),
                Value::Object(o) => {
                    match o.with_payload::<crate::tree::NodeRef, _>(|r| r.clone()) {
                        Some(r) => Ok(XValue::NodeSet(vec![r.id])),
                        None => {
                            self.unwind = Some(Unwind::type_error(
                            "Only objects that are instances of DOM nodes can be converted to an XPath expression",
                        ));
                            Err("callback".to_string())
                        }
                    }
                }
                Value::Array(_) => {
                    if let Err(u) = self.ctx.warn("Array to string conversion") {
                        self.unwind = Some(u);
                        return Err("callback".to_string());
                    }
                    Ok(XValue::Str(b"Array".to_vec()))
                }
                other => Ok(XValue::Str(other.to_php_bytes())),
            },
            Err(u) => {
                self.unwind = Some(u);
                Err("callback".to_string())
            }
        }
    }
}

/// Run `expr` against `context` (the document when null): the result, or
/// the warnings php prints and `false`.
fn run(ctx: &mut Ctx, o: &Object, who: &str, args: &[Value], as_list: bool) -> NativeResult {
    let st = state(o)?;
    let expr = str_arg(args, 0);
    // php's default context node is the document element.
    let default_context = st.doc.borrow().document_element().unwrap_or(DOCUMENT);
    let context = match args.get(1).map(|v| v.deref().into_owned()) {
        None | Some(Value::Null) => default_context,
        Some(v) => match node_arg(&v) {
            Some(n) if std::rc::Rc::ptr_eq(&n.doc, &st.doc) => n.id,
            Some(_) => {
                return Err(Unwind::error("Node From Wrong Document"));
            }
            None => {
                return Err(Unwind::type_error(format!(
                    "{who}: Argument #2 ($contextNode) must be of type ?DOMNode"
                )))
            }
        },
    };
    let register_node_ns = args.get(2).map_or(st.register_node_ns, Value::to_bool);
    let compiled = match xpath::compile(&expr) {
        Ok(c) => c,
        Err(_) => {
            libxml_error(ctx, 1207, "Invalid expression");
            ctx.warn(&format!("{who}: Invalid expression"))?;
            return Ok(Value::Bool(false));
        }
    };
    let mut namespaces: HashMap<Vec<u8>, Vec<u8>> = HashMap::new();
    if register_node_ns {
        // The context node's in-scope declarations, prefixed ones only.
        let d = st.doc.borrow();
        let mut cur = Some(context);
        while let Some(n) = cur {
            if d.node(n).kind == NodeKind::Element {
                for dcl in &d.node(n).ns_decls {
                    if let Some(p) = &dcl.prefix {
                        namespaces
                            .entry(p.clone())
                            .or_insert_with(|| dcl.uri.clone());
                    }
                }
            }
            cur = d.node(n).parent;
        }
    }
    for (p, u) in &st.namespaces {
        namespaces.insert(p.clone(), u.clone());
    }
    let doc_rc = st.doc.clone();
    let (result, unwind) = {
        let d = doc_rc.borrow();
        // The document is borrowed for the evaluation; a callback that
        // mutated it would hit the borrow (cataloged).
        let mut host = PhpHost {
            ctx,
            namespaces,
            php_functions: st.php_functions.clone(),
            ns_functions: st.ns_functions.clone(),
            doc: doc_rc.clone(),
            unwind: None,
        };
        let cx = Context {
            node: context,
            position: 0,
            size: 0,
            in_predicate: false,
        };
        let result = match compiled.check_prefixes(&host) {
            Err(e) => Err(e),
            Ok(()) => {
                let mut eval = Eval::new(&d, &mut host);
                eval.evaluate(&compiled, &cx)
            }
        };
        (result, host.unwind.take())
    };
    if let Some(u) = unwind {
        return Err(u);
    }
    let value = match result {
        Ok(v) => v,
        Err(msg) => {
            if msg.contains("bound to undefined prefix")
                || msg.starts_with("xmlXPathCompOpEval: function")
            {
                ctx.warn(&format!("{who}: {msg}"))?;
                ctx.warn(&format!("{who}: Unregistered function"))?;
            } else if msg == "Unregistered function" {
                ctx.warn(&format!("{who}: Unregistered function"))?;
            } else {
                if msg == "Undefined namespace prefix" {
                    libxml_error(ctx, 1219, &msg);
                }
                ctx.warn(&format!("{who}: {msg}"))?;
            }
            return Ok(Value::Bool(false));
        }
    };
    match value {
        XValue::NodeSet(set) => Ok(Value::Object(super::lists::fixed_list(ctx, &doc_rc, set)?)),
        _ if as_list => Ok(Value::Object(super::lists::fixed_list(
            ctx,
            &doc_rc,
            Vec::new(),
        )?)),
        XValue::Bool(b) => Ok(Value::Bool(b)),
        XValue::Number(n) => Ok(Value::Float(n)),
        XValue::Str(s) => Ok(Value::string(&s)),
    }
}

/// An XPath error php records on libxml's error list even without
/// `libxml_use_internal_errors()` (line and column 0).
fn libxml_error(ctx: &mut Ctx, code: i64, message: &str) {
    libxml::state(ctx).errors.push(XmlError {
        level: 2,
        code,
        line: 0,
        column: 0,
        message: format!("{message}\n"),
    });
}

fn query(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    run(ctx, this(o)?, "DOMXPath::query()", args, true)
}

fn evaluate(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    run(ctx, this(o)?, "DOMXPath::evaluate()", args, false)
}

pub fn register(r: &mut Registry) {
    r.class("DOMXPath")
        .native_props(NativeProps {
            names: &["document", "registerNodeNamespaces"],
            get: get_prop,
            set: set_prop,
            isset: None,
            unset: None,
            list: None,
            debug: None,
            cast: None,
        })
        .method("__construct", nm!(1, Some(2), construct))
        .method("registerNamespace", nm!(2, Some(2), register_namespace))
        .method(
            "registerPhpFunctions",
            nm!(0, Some(1), register_php_functions),
        )
        .method(
            "registerPhpFunctionNS",
            nm!(3, Some(3), register_php_function_ns),
        )
        .method("query", nm!(1, Some(3), query))
        .method("evaluate", nm!(1, Some(3), evaluate))
        .method("quote", super::static_method(1, Some(1), quote))
        .finish();
}

/// php 8.4's `Dom\XPath`, the same engine over a modern document.
pub fn register_modern(r: &mut Registry) {
    r.class("Dom\\XPath")
        .flags(rphp_runtime::ClassFlags::FINAL)
        .native_props(NativeProps {
            names: &["document", "registerNodeNamespaces"],
            get: get_prop,
            set: set_prop,
            isset: None,
            unset: None,
            list: None,
            debug: None,
            cast: None,
        })
        .method("__construct", nm!(1, Some(2), construct))
        .method("registerNamespace", nm!(2, Some(2), register_namespace))
        .method(
            "registerPhpFunctions",
            nm!(0, Some(1), register_php_functions),
        )
        .method(
            "registerPhpFunctionNS",
            nm!(3, Some(3), register_php_function_ns),
        )
        .method("query", nm!(1, Some(3), query))
        .method("evaluate", nm!(1, Some(3), evaluate))
        .method("quote", super::static_method(1, Some(1), quote))
        .finish();
}
