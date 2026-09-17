//! `cargo xtask gen` — generate the per-extension descriptor tables from the
//! PHP oracle manifest (`manifest/php-8.5.0`, produced by
//! `tools/manifest/dump.php`) as `rphp_ext_api` statics:
//!
//! ```text
//! cargo xtask gen [--manifest <dir>] [--ext <name>]... [--out <dir>] [--create] [--check]
//! ```
//!
//! For every extension (default: all in the manifest) it writes into
//! `crates/ext/rphp-ext-<name>/src/generated/`:
//!
//! - `arginfo.rs` — one `pub static <NAME>: FnSig` per function and one
//!   `<CLASS>__<METHOD>` per method, plus `FUNCTIONS`, `METHODS`, `ALL`;
//! - `consts.rs` — `pub static CONSTANTS: &[ConstDef]` (environment-specific
//!   values such as `PHP_BINARY` or `STDIN` become `ConstValue::Runtime`);
//! - `ini.rs` — `pub static INI: &[IniDef]` with the compiled-in defaults;
//! - `classes.rs` — `pub static <CLASS>: ClassSig` skeletons plus `CLASSES`;
//! - `mod.rs` — the module list (`#[rustfmt::skip]`) and `INFO: ExtInfo`.
//!
//! An extension whose crate directory does not exist is skipped with a note
//! unless `--out <dir>` or `--create` is given (`--out` with several
//! extensions writes `<dir>/<ext>/`). `--check` regenerates into a temporary
//! directory and fails when the files on disk differ. Output is fully
//! deterministic: sorted by name, declaration order kept where PHP exposes it.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use rphp_ext_api::TypeMask;
use serde::Deserialize;
use serde_json::Value as Json;

use crate::XtaskResult;

/// Constants whose value depends on the process or build environment and is
/// therefore computed by the engine at startup rather than baked in.
const RUNTIME_CONSTANTS: &[&str] = &[
    "PHP_BINARY",
    "PHP_OS",
    "PHP_OS_FAMILY",
    "PHP_SAPI",
    "DIRECTORY_SEPARATOR",
    "PATH_SEPARATOR",
    "DEFAULT_INCLUDE_PATH",
    "PEAR_INSTALL_DIR",
    "PEAR_EXTENSION_DIR",
    "PHP_EXTENSION_DIR",
    "PHP_PREFIX",
    "PHP_BINDIR",
    "PHP_LIBDIR",
    "PHP_DATADIR",
    "PHP_SYSCONFDIR",
    "PHP_LOCALSTATEDIR",
    "PHP_CONFIG_FILE_PATH",
    "PHP_CONFIG_FILE_SCAN_DIR",
    "PHP_SHLIB_SUFFIX",
    "PHP_FD_SETSIZE",
    "PHP_MAXPATHLEN",
    "PHP_MANDIR",
    "STDIN",
    "STDOUT",
    "STDERR",
];

const GENERATED_FILES: &[&str] = &["mod.rs", "arginfo.rs", "consts.rs", "ini.rs", "classes.rs"];

// ---------------------------------------------------------------------------
// manifest schema (see tools/manifest/dump.php)

#[derive(Deserialize)]
struct MSig {
    name: String,
    deprecated: bool,
    returns_ref: bool,
    required: u32,
    #[serde(rename = "return")]
    ret: Option<MRet>,
    params: Vec<MParam>,
}

#[derive(Deserialize)]
struct MRet {
    #[serde(rename = "type")]
    ty: String,
}

#[derive(Deserialize)]
struct MParam {
    name: String,
    #[serde(rename = "type")]
    ty: Option<String>,
    nullable: bool,
    by_ref: bool,
    prefer_ref: bool,
    variadic: bool,
    default_expr: Option<String>,
    default: Option<Json>,
}

#[derive(Deserialize)]
struct MFn {
    #[serde(flatten)]
    sig: MSig,
    extension: String,
}

#[derive(Deserialize)]
struct MMethod {
    #[serde(flatten)]
    sig: MSig,
    visibility: String,
    #[serde(rename = "static")]
    is_static: bool,
    #[serde(rename = "abstract")]
    is_abstract: bool,
    #[serde(rename = "final")]
    is_final: bool,
}

#[derive(Deserialize)]
struct MClassConst {
    name: String,
    value: Json,
    visibility: String,
    #[serde(rename = "final")]
    is_final: bool,
}

#[derive(Deserialize)]
struct MProp {
    name: String,
    #[serde(rename = "static")]
    is_static: bool,
    visibility: String,
    readonly: bool,
    #[serde(rename = "type")]
    ty: Option<String>,
    has_default: bool,
    default: Json,
}

#[derive(Deserialize)]
struct MCase {
    name: String,
    value: Json,
}

#[derive(Deserialize)]
struct MEnum {
    backing_type: Option<String>,
    cases: Vec<MCase>,
}

#[derive(Deserialize)]
struct MClass {
    name: String,
    extension: String,
    kind: String,
    #[serde(rename = "abstract")]
    is_abstract: bool,
    #[serde(rename = "final")]
    is_final: bool,
    readonly: bool,
    parent: Option<String>,
    interfaces: Vec<String>,
    constants: Vec<MClassConst>,
    properties: Vec<MProp>,
    methods: Vec<MMethod>,
    #[serde(rename = "enum")]
    enum_info: Option<MEnum>,
}

#[derive(Deserialize)]
struct MConst {
    value: Json,
    #[serde(rename = "type")]
    ty: String,
    deprecated: bool,
}

#[derive(Deserialize)]
struct MIni {
    global_value: Json,
    access: u8,
    extension: Option<String>,
}

#[derive(Deserialize)]
struct MExt {
    version: String,
    dependencies: BTreeMap<String, String>,
}

/// The loaded manifest directory.
struct Manifest {
    functions: BTreeMap<String, MFn>,
    classes: BTreeMap<String, MClass>,
    constants: BTreeMap<String, BTreeMap<String, MConst>>,
    ini: BTreeMap<String, MIni>,
    extensions: BTreeMap<String, MExt>,
}

impl Manifest {
    fn load(dir: &Path) -> Result<Manifest, Box<dyn std::error::Error>> {
        fn read<T: serde::de::DeserializeOwned>(
            dir: &Path,
            file: &str,
        ) -> Result<T, Box<dyn std::error::Error>> {
            let path = dir.join(file);
            let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()).into())
        }
        Ok(Manifest {
            functions: read(dir, "functions.json")?,
            classes: read(dir, "classes.json")?,
            constants: read(dir, "constants.json")?,
            ini: read(dir, "ini.json")?,
            extensions: read(dir, "extensions.json")?,
        })
    }

    /// The manifest's spelling of an extension name, matched case-insensitively.
    fn extension_name(&self, name: &str) -> Option<&str> {
        self.extensions
            .keys()
            .find(|k| k.eq_ignore_ascii_case(name))
            .map(String::as_str)
    }
}

// ---------------------------------------------------------------------------
// type strings

/// A parsed declaration type: builtin mask plus the class part as spelled.
#[derive(Debug, PartialEq)]
struct Ty {
    mask: TypeMask,
    class: Option<String>,
}

/// Split a type string at top-level `|` (a DNF group `(A&B)` stays whole).
fn split_union(s: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    for (i, ch) in s.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            '|' if depth == 0 => {
                parts.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&s[start..]);
    parts
}

/// Map a reflection type string (`?int`, `array|string|null`,
/// `Traversable|array`, `RoundingMode|int`, `(A&B)|null`) to a [`Ty`].
fn parse_type(s: Option<&str>) -> Ty {
    let mut mask = TypeMask::EMPTY;
    let mut classes: Vec<&str> = Vec::new();
    let Some(s) = s else {
        return Ty { mask, class: None };
    };
    let s = s.trim();
    let s = match s.strip_prefix('?') {
        Some(rest) => {
            mask |= TypeMask::NULL;
            rest
        }
        None => s,
    };
    for part in split_union(s) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if part.starts_with('(') || part.contains('&') {
            classes.push(part);
            continue;
        }
        match TypeMask::from_keyword(part) {
            Some(bit) => mask |= bit,
            None => classes.push(part),
        }
    }
    let class = if classes.is_empty() {
        None
    } else {
        Some(classes.join("|"))
    };
    Ty { mask, class }
}

// ---------------------------------------------------------------------------
// Rust literal helpers

/// A Rust string literal.
fn rs_str(s: &str) -> String {
    format!("{s:?}")
}

fn rs_opt_str(s: Option<&str>) -> String {
    match s {
        Some(s) => format!("Some({})", rs_str(s)),
        None => "None".to_string(),
    }
}

fn rs_i64(i: i64) -> String {
    if i == i64::MIN {
        "i64::MIN".to_string()
    } else {
        i.to_string()
    }
}

fn rs_f64(f: f64) -> String {
    if f.is_nan() {
        "f64::NAN".to_string()
    } else if f == f64::INFINITY {
        "f64::INFINITY".to_string()
    } else if f == f64::NEG_INFINITY {
        "f64::NEG_INFINITY".to_string()
    } else {
        format!("{f:?}_f64")
    }
}

/// `TypeMask::INT.union(TypeMask::NULL)` — a `const` expression for a mask.
fn rs_mask(mask: TypeMask) -> String {
    const ORDER: &[(&str, TypeMask)] = &[
        ("MIXED", TypeMask::MIXED),
        ("STATIC", TypeMask::STATIC),
        ("CALLABLE", TypeMask::CALLABLE),
        ("OBJECT", TypeMask::OBJECT),
        ("ITERABLE", TypeMask::ITERABLE),
        ("ARRAY", TypeMask::ARRAY),
        ("STRING", TypeMask::STRING),
        ("INT", TypeMask::INT),
        ("FLOAT", TypeMask::FLOAT),
        ("BOOL", TypeMask::BOOL),
        ("FALSE", TypeMask::FALSE),
        ("TRUE", TypeMask::TRUE),
        ("VOID", TypeMask::VOID),
        ("NEVER", TypeMask::NEVER),
        ("RESOURCE", TypeMask::RESOURCE),
        ("NULL", TypeMask::NULL),
    ];
    let mut rest = mask;
    let mut out = String::new();
    for (name, bit) in ORDER {
        if rest.contains(*bit) && !bit.is_empty() {
            if out.is_empty() {
                out.push_str("TypeMask::");
                out.push_str(name);
            } else {
                let _ = write!(out, ".union(TypeMask::{name})");
            }
            rest = rest - *bit;
        }
    }
    if !rest.is_empty() {
        let _ = write!(
            out,
            ".union(TypeMask::from_bits_retain({:#x}))",
            rest.bits()
        );
    }
    if out.is_empty() {
        out.push_str("TypeMask::EMPTY");
    }
    out
}

fn rs_vis(v: &str) -> &'static str {
    match v {
        "private" => "Vis::Private",
        "protected" => "Vis::Protected",
        _ => "Vis::Public",
    }
}

/// An uppercase Rust identifier for a PHP name (`strlen` → `STRLEN`,
/// `Random\Randomizer` → `RANDOM__RANDOMIZER`). The namespace separator
/// becomes `__` so `Dom\import_simplexml` and `dom_import_simplexml` (both
/// exist since 8.4) stay distinct.
fn ident(name: &str) -> String {
    let mut s = String::with_capacity(name.len() + 4);
    for c in name.chars() {
        match c {
            '\\' => s.push_str("__"),
            c if c.is_ascii_alphanumeric() => s.push(c.to_ascii_uppercase()),
            _ => s.push('_'),
        }
    }
    if s.starts_with(|c: char| c.is_ascii_digit()) {
        s.insert(0, '_');
    }
    s
}

/// The crate directory suffix for an extension (`Zend OPcache` → `zend_opcache`).
fn ext_dir_name(ext: &str) -> String {
    ext.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect()
}

/// Whether a stub default's text is a plain literal (so the evaluated JSON
/// value is used) rather than a constant expression kept as text.
fn is_literal_expr(expr: &str) -> bool {
    let e = expr.trim();
    if e.is_empty() {
        return false;
    }
    let lower = e.to_ascii_lowercase();
    if matches!(lower.as_str(), "null" | "true" | "false" | "[]") {
        return true;
    }
    if e.starts_with('"') || e.starts_with('\'') {
        return true;
    }
    let body = e.strip_prefix('-').unwrap_or(e);
    body.starts_with(|c: char| c.is_ascii_digit())
        && body.chars().all(|c| {
            c.is_ascii_hexdigit() || matches!(c, '.' | 'x' | 'X' | 'o' | 'O' | '_' | '+' | '-')
        })
}

/// `DefaultVal` for a JSON default value (already known not to be a tagged
/// `{"const"|"expr"|"bytes"}` object).
fn default_from_json(v: &Json, fallback_expr: Option<&str>) -> Result<String, String> {
    Ok(match v {
        Json::Null => "DefaultVal::Null".to_string(),
        Json::Bool(b) => format!("DefaultVal::Bool({b})"),
        Json::Number(n) => match (n.as_i64(), n.as_f64()) {
            (Some(i), _) => format!("DefaultVal::Int({})", rs_i64(i)),
            (None, Some(f)) => format!("DefaultVal::Float({})", rs_f64(f)),
            _ => return Err(format!("unrepresentable number {n}")),
        },
        Json::String(s) => format!("DefaultVal::Str({})", rs_str(s)),
        Json::Array(a) if a.is_empty() => "DefaultVal::EmptyArray".to_string(),
        other => match fallback_expr {
            Some(e) => format!("DefaultVal::Const({})", rs_str(e)),
            None => return Err(format!("unrepresentable default {other}")),
        },
    })
}

/// The `DefaultVal` expression for a parameter, if it has a default.
fn param_default(p: &MParam) -> Result<Option<String>, String> {
    let Some(v) = &p.default else {
        return Ok(None);
    };
    let expr = p.default_expr.as_deref();
    if let Json::Object(o) = v {
        if let Some(Json::String(c)) = o.get("const") {
            return Ok(Some(format!("DefaultVal::Const({})", rs_str(c))));
        }
        if let Some(Json::String(e)) = o.get("expr") {
            return Ok(Some(format!("DefaultVal::Const({})", rs_str(e))));
        }
        return match expr {
            Some(e) => Ok(Some(format!("DefaultVal::Const({})", rs_str(e)))),
            None => Err(format!(
                "parameter ${}: unrepresentable default {v}",
                p.name
            )),
        };
    }
    if let Some(e) = expr {
        if !is_literal_expr(e) {
            return Ok(Some(format!("DefaultVal::Const({})", rs_str(e))));
        }
    }
    default_from_json(v, expr)
        .map(Some)
        .map_err(|e| format!("parameter ${}: {e}", p.name))
}

/// A `DefaultVal` for a property default (no stub text available).
fn prop_default(v: &Json) -> String {
    match v {
        Json::Object(o) => {
            let text = o
                .get("enum")
                .or_else(|| o.get("expr"))
                .or_else(|| o.get("const"))
                .and_then(Json::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| v.to_string());
            format!("DefaultVal::Const({})", rs_str(&text))
        }
        _ => default_from_json(v, Some(&v.to_string()))
            .unwrap_or_else(|_| format!("DefaultVal::Const({})", rs_str(&v.to_string()))),
    }
}

/// A `ConstValue` for a JSON constant value (global, class constant or enum
/// case); anything not a scalar becomes `ConstValue::Runtime`.
fn const_value(v: &Json) -> String {
    match v {
        Json::Null => "ConstValue::Null".to_string(),
        Json::Bool(b) => format!("ConstValue::Bool({b})"),
        Json::Number(n) => match (n.as_i64(), n.as_f64()) {
            (Some(i), _) => format!("ConstValue::Int({})", rs_i64(i)),
            (None, Some(f)) => format!("ConstValue::Float({})", rs_f64(f)),
            _ => "ConstValue::Runtime".to_string(),
        },
        Json::String(s) => format!("ConstValue::Str({})", rs_str(s)),
        Json::Object(o) => match o.get("expr").and_then(Json::as_str) {
            Some("NAN") => "ConstValue::Float(f64::NAN)".to_string(),
            Some("INF") => "ConstValue::Float(f64::INFINITY)".to_string(),
            Some("-INF") => "ConstValue::Float(f64::NEG_INFINITY)".to_string(),
            _ => "ConstValue::Runtime".to_string(),
        },
        Json::Array(_) => "ConstValue::Runtime".to_string(),
    }
}

fn rs_ini_access(bits: u8) -> String {
    if bits & 7 == 7 {
        return "IniAccess::ALL".to_string();
    }
    let mut parts = Vec::new();
    if bits & 1 != 0 {
        parts.push("IniAccess::USER");
    }
    if bits & 2 != 0 {
        parts.push("IniAccess::PERDIR");
    }
    if bits & 4 != 0 {
        parts.push("IniAccess::SYSTEM");
    }
    match parts.split_first() {
        None => "IniAccess::EMPTY".to_string(),
        Some((first, rest)) => rest
            .iter()
            .fold(first.to_string(), |acc, p| format!("{acc}.union({p})")),
    }
}

// ---------------------------------------------------------------------------
// per-extension generation

/// Everything the manifest holds for one extension.
struct ExtData<'a> {
    name: &'a str,
    info: Option<&'a MExt>,
    functions: Vec<&'a MFn>,
    classes: Vec<&'a MClass>,
    constants: Vec<(&'a str, &'a MConst)>,
    ini: Vec<(&'a str, &'a MIni)>,
}

impl<'a> ExtData<'a> {
    fn collect(m: &'a Manifest, name: &'a str) -> ExtData<'a> {
        let functions = m
            .functions
            .values()
            .filter(|f| f.extension.eq_ignore_ascii_case(name))
            .collect();
        let classes = m
            .classes
            .values()
            .filter(|c| c.extension.eq_ignore_ascii_case(name))
            .collect();
        let constants = m
            .constants
            .iter()
            .filter(|(cat, _)| cat.eq_ignore_ascii_case(name))
            .flat_map(|(_, list)| list.iter().map(|(k, v)| (k.as_str(), v)))
            .collect();
        let ini = m
            .ini
            .iter()
            .filter(|(_, i)| {
                i.extension
                    .as_deref()
                    .is_some_and(|e| e.eq_ignore_ascii_case(name))
            })
            .map(|(k, v)| (k.as_str(), v))
            .collect();
        ExtData {
            name,
            info: m.extensions.get(name),
            functions,
            classes,
            constants,
            ini,
        }
    }

    fn method_count(&self) -> usize {
        self.classes.iter().map(|c| c.methods.len()).sum()
    }
}

/// Tracks generated identifiers so two PHP names never map to one Rust name.
struct Idents {
    used: BTreeMap<String, String>,
}

impl Idents {
    fn new(reserved: &[&str]) -> Idents {
        let used = reserved
            .iter()
            .map(|r| (r.to_string(), format!("reserved `{r}`")))
            .collect();
        Idents { used }
    }

    fn claim(&mut self, id: String, source: &str) -> Result<String, String> {
        if let Some(prev) = self.used.get(&id) {
            return Err(format!(
                "identifier `{id}` for `{source}` collides with {prev}"
            ));
        }
        self.used.insert(id.clone(), format!("`{source}`"));
        Ok(id)
    }
}

fn header(out: &mut String, label: &str, doc: &str, allow: &str) {
    let _ = writeln!(
        out,
        "// @generated by `cargo xtask gen` from {label} — do not edit."
    );
    let _ = writeln!(out, "//! {doc}");
    let _ = writeln!(out, "#![allow({allow})]");
    out.push('\n');
}

/// The PHP spelling of a signature for the doc comment.
fn php_signature(display_name: &str, sig: &MSig) -> String {
    let mut s = format!("{display_name}(");
    for (i, p) in sig.params.iter().enumerate() {
        if i > 0 {
            s.push_str(", ");
        }
        if let Some(t) = &p.ty {
            s.push_str(t);
            s.push(' ');
        }
        if p.by_ref {
            s.push('&');
        }
        if p.variadic {
            s.push_str("...");
        }
        s.push('$');
        s.push_str(&p.name);
        if let Some(e) = &p.default_expr {
            s.push_str(" = ");
            s.push_str(e);
        } else if let Some(d) = &p.default {
            let _ = write!(s, " = {d}");
        }
    }
    s.push(')');
    if let Some(r) = &sig.ret {
        s.push_str(": ");
        s.push_str(&r.ty);
    }
    s.replace(['\n', '\r'], " ")
}

fn emit_fn_sig(
    out: &mut String,
    id: &str,
    name: &str,
    display_name: &str,
    sig: &MSig,
) -> Result<(), String> {
    let _ = writeln!(out, "/// `{}`", php_signature(display_name, sig));
    let _ = writeln!(out, "pub static {id}: FnSig = FnSig {{");
    let _ = writeln!(out, "    name: {},", rs_str(name));
    if sig.params.is_empty() {
        out.push_str("    params: &[],\n");
    } else {
        out.push_str("    params: &[\n");
        for p in &sig.params {
            let ty = parse_type(p.ty.as_deref());
            let default = param_default(p).map_err(|e| format!("{display_name}: {e}"))?;
            let _ = writeln!(
                out,
                "        ParamInfo {{ name: {}, ty: {}, class: {}, by_ref: {}, prefer_ref: {}, variadic: {}, nullable: {}, default: {} }},",
                rs_str(&p.name),
                rs_mask(ty.mask),
                rs_opt_str(ty.class.as_deref()),
                p.by_ref,
                p.prefer_ref,
                p.variadic,
                p.nullable,
                default.map_or_else(|| "None".to_string(), |d| format!("Some({d})")),
            );
        }
        out.push_str("    ],\n");
    }
    let required = u8::try_from(sig.required)
        .map_err(|_| format!("{display_name}: {} required parameters", sig.required))?;
    let ret = parse_type(sig.ret.as_ref().map(|r| r.ty.as_str()));
    let _ = writeln!(out, "    required: {required},");
    let _ = writeln!(out, "    ret: {},", rs_mask(ret.mask));
    let _ = writeln!(out, "    ret_class: {},", rs_opt_str(ret.class.as_deref()));
    let _ = writeln!(out, "    deprecated: {},", sig.deprecated);
    let _ = writeln!(out, "    returns_ref: {},", sig.returns_ref);
    out.push_str("};\n\n");
    Ok(())
}

fn emit_static_list(out: &mut String, doc: &str, name: &str, ty: &str, items: &[String]) {
    let _ = writeln!(out, "/// {doc}");
    if items.is_empty() {
        let _ = writeln!(out, "pub static {name}: &[&{ty}] = &[];");
    } else {
        let _ = writeln!(out, "pub static {name}: &[&{ty}] = &[");
        for it in items {
            let _ = writeln!(out, "    &{it},");
        }
        out.push_str("];\n");
    }
}

/// Generated `arginfo.rs`; returns the file plus the method identifier map
/// (`Class::method` → static name) that `classes.rs` links against.
fn gen_arginfo(
    ext: &ExtData<'_>,
    label: &str,
) -> Result<(String, BTreeMap<String, String>), String> {
    let mut out = String::new();
    header(
        &mut out,
        label,
        &format!(
            "Function and method signatures of ext/{} ({} functions, {} methods).",
            ext.name,
            ext.functions.len(),
            ext.method_count()
        ),
        "clippy::all, dead_code, unused_imports",
    );
    out.push_str("use rphp_ext_api::{DefaultVal, FnSig, ParamInfo, TypeMask};\n\n");
    let mut idents = Idents::new(&["ALL", "FUNCTIONS", "METHODS"]);
    let mut functions = Vec::new();
    for f in &ext.functions {
        let id = idents.claim(ident(&f.sig.name), &f.sig.name)?;
        emit_fn_sig(
            &mut out,
            &id,
            &f.sig.name.to_ascii_lowercase(),
            &f.sig.name,
            &f.sig,
        )?;
        functions.push(id);
    }
    let mut methods = Vec::new();
    let mut method_ids = BTreeMap::new();
    for c in &ext.classes {
        for m in &c.methods {
            let display = format!("{}::{}", c.name, m.sig.name);
            let id = idents.claim(
                format!("{}__{}", ident(&c.name), ident(&m.sig.name)),
                &display,
            )?;
            let name = format!(
                "{}::{}",
                c.name.to_ascii_lowercase(),
                m.sig.name.to_ascii_lowercase()
            );
            emit_fn_sig(&mut out, &id, &name, &display, &m.sig)?;
            method_ids.insert(display, id.clone());
            methods.push(id);
        }
    }
    emit_static_list(
        &mut out,
        "All functions of the extension, sorted by name.",
        "FUNCTIONS",
        "FnSig",
        &functions,
    );
    out.push('\n');
    emit_static_list(
        &mut out,
        "All methods of the extension's classes (`class::method`), by class then declaration order.",
        "METHODS",
        "FnSig",
        &methods,
    );
    out.push('\n');
    let all: Vec<String> = functions.iter().chain(methods.iter()).cloned().collect();
    emit_static_list(
        &mut out,
        "Every signature: functions, then methods.",
        "ALL",
        "FnSig",
        &all,
    );
    Ok((out, method_ids))
}

fn gen_consts(ext: &ExtData<'_>, label: &str) -> String {
    let mut out = String::new();
    header(
        &mut out,
        label,
        &format!(
            "Constants of ext/{} (`get_defined_constants(true)`), sorted by name.",
            ext.name
        ),
        "clippy::all, dead_code, unused_imports",
    );
    out.push_str("use rphp_ext_api::{ConstDef, ConstValue};\n\n");
    out.push_str("/// Value constants verbatim from the oracle; resource/object-valued and\n");
    out.push_str(
        "/// environment-specific ones are `ConstValue::Runtime` (computed at startup).\n",
    );
    if ext.constants.is_empty() {
        out.push_str("pub static CONSTANTS: &[ConstDef] = &[];\n");
        return out;
    }
    out.push_str("pub static CONSTANTS: &[ConstDef] = &[\n");
    for (name, c) in &ext.constants {
        let value = if RUNTIME_CONSTANTS.contains(name)
            || matches!(c.ty.as_str(), "resource" | "object" | "array")
        {
            "ConstValue::Runtime".to_string()
        } else {
            const_value(&c.value)
        };
        let _ = writeln!(
            out,
            "    ConstDef {{ name: {}, value: {value}, deprecated: {} }},",
            rs_str(name),
            c.deprecated
        );
    }
    out.push_str("];\n");
    out
}

fn gen_ini(ext: &ExtData<'_>, label: &str) -> String {
    let mut out = String::new();
    header(
        &mut out,
        label,
        &format!(
            "Ini directives of ext/{} with their compiled-in defaults (`php -n`).",
            ext.name
        ),
        "clippy::all, dead_code, unused_imports",
    );
    out.push_str("use rphp_ext_api::{IniAccess, IniDef};\n\n");
    out.push_str("/// Directives sorted by name.\n");
    if ext.ini.is_empty() {
        out.push_str("pub static INI: &[IniDef] = &[];\n");
        return out;
    }
    out.push_str("pub static INI: &[IniDef] = &[\n");
    for (name, i) in &ext.ini {
        let default = match &i.global_value {
            Json::String(s) => rs_opt_str(Some(s)),
            Json::Null => "None".to_string(),
            other => rs_opt_str(Some(&other.to_string())),
        };
        let _ = writeln!(
            out,
            "    IniDef {{ name: {}, default: {default}, access: {} }},",
            rs_str(name),
            rs_ini_access(i.access)
        );
    }
    out.push_str("];\n");
    out
}

fn gen_classes(
    ext: &ExtData<'_>,
    label: &str,
    method_ids: &BTreeMap<String, String>,
) -> Result<String, String> {
    let mut out = String::new();
    header(
        &mut out,
        label,
        &format!(
            "Class skeletons of ext/{} ({} classes).",
            ext.name,
            ext.classes.len()
        ),
        "clippy::all, dead_code, unused_imports",
    );
    out.push_str("use rphp_ext_api::{ClassConstSig, ClassKind, ClassSig, ClassSigFlags, ConstValue, DefaultVal, EnumCaseSig, MethodSig, PropSig, TypeMask, Vis};\n\n");
    out.push_str("use super::arginfo;\n\n");
    let mut idents = Idents::new(&["CLASSES"]);
    let mut list = Vec::new();
    for c in &ext.classes {
        let id = idents.claim(ident(&c.name), &c.name)?;
        let kind = match c.kind.as_str() {
            "interface" => "ClassKind::Interface",
            "trait" => "ClassKind::Trait",
            "enum" => "ClassKind::Enum",
            _ => "ClassKind::Class",
        };
        let mut flags = Vec::new();
        if c.is_abstract {
            flags.push("ClassSigFlags::ABSTRACT");
        }
        if c.is_final {
            flags.push("ClassSigFlags::FINAL");
        }
        if c.readonly {
            flags.push("ClassSigFlags::READONLY");
        }
        let flags = match flags.split_first() {
            None => "ClassSigFlags::EMPTY".to_string(),
            Some((first, rest)) => rest
                .iter()
                .fold(first.to_string(), |acc, f| format!("{acc}.union({f})")),
        };
        // doc line: `final class Foo extends Bar implements Baz`
        let mut doc = String::new();
        if c.is_abstract {
            doc.push_str("abstract ");
        }
        if c.is_final {
            doc.push_str("final ");
        }
        if c.readonly {
            doc.push_str("readonly ");
        }
        doc.push_str(&c.kind);
        doc.push(' ');
        doc.push_str(&c.name);
        if let Some(e) = &c.enum_info {
            if let Some(b) = &e.backing_type {
                let _ = write!(doc, ": {b}");
            }
        }
        if let Some(p) = &c.parent {
            let _ = write!(doc, " extends {p}");
        }
        if !c.interfaces.is_empty() {
            let kw = if c.kind == "interface" {
                "extends"
            } else {
                "implements"
            };
            let _ = write!(doc, " {kw} {}", c.interfaces.join(", "));
        }
        let _ = writeln!(out, "/// `{doc}`");
        let _ = writeln!(out, "pub static {id}: ClassSig = ClassSig {{");
        let _ = writeln!(out, "    name: {},", rs_str(&c.name));
        let _ = writeln!(out, "    kind: {kind},");
        let _ = writeln!(out, "    flags: {flags},");
        let _ = writeln!(out, "    parent: {},", rs_opt_str(c.parent.as_deref()));
        let ifaces: Vec<String> = c.interfaces.iter().map(|i| rs_str(i)).collect();
        let _ = writeln!(out, "    interfaces: &[{}],", ifaces.join(", "));
        if c.constants.is_empty() {
            out.push_str("    consts: &[],\n");
        } else {
            out.push_str("    consts: &[\n");
            for k in &c.constants {
                let _ = writeln!(
                    out,
                    "        ClassConstSig {{ name: {}, value: {}, vis: {}, is_final: {} }},",
                    rs_str(&k.name),
                    const_value(&k.value),
                    rs_vis(&k.visibility),
                    k.is_final
                );
            }
            out.push_str("    ],\n");
        }
        if c.properties.is_empty() {
            out.push_str("    props: &[],\n");
        } else {
            out.push_str("    props: &[\n");
            for p in &c.properties {
                let ty = parse_type(p.ty.as_deref());
                let default = if p.has_default {
                    format!("Some({})", prop_default(&p.default))
                } else {
                    "None".to_string()
                };
                let _ = writeln!(
                    out,
                    "        PropSig {{ name: {}, ty: {}, class: {}, vis: {}, is_static: {}, readonly: {}, default: {default} }},",
                    rs_str(&p.name),
                    rs_mask(ty.mask),
                    rs_opt_str(ty.class.as_deref()),
                    rs_vis(&p.visibility),
                    p.is_static,
                    p.readonly
                );
            }
            out.push_str("    ],\n");
        }
        if c.methods.is_empty() {
            out.push_str("    methods: &[],\n");
        } else {
            out.push_str("    methods: &[\n");
            for m in &c.methods {
                let key = format!("{}::{}", c.name, m.sig.name);
                let sig_id = method_ids
                    .get(&key)
                    .ok_or_else(|| format!("no arginfo for {key}"))?;
                let _ = writeln!(
                    out,
                    "        MethodSig {{ sig: &arginfo::{sig_id}, vis: {}, is_static: {}, is_abstract: {}, is_final: {} }},",
                    rs_vis(&m.visibility),
                    m.is_static,
                    m.is_abstract || c.kind == "interface",
                    m.is_final
                );
            }
            out.push_str("    ],\n");
        }
        let backing = c
            .enum_info
            .as_ref()
            .and_then(|e| e.backing_type.as_deref())
            .map_or(TypeMask::EMPTY, |b| {
                TypeMask::from_keyword(b).unwrap_or(TypeMask::EMPTY)
            });
        let _ = writeln!(out, "    backing: {},", rs_mask(backing));
        match c.enum_info.as_ref().filter(|e| !e.cases.is_empty()) {
            None => out.push_str("    cases: &[],\n"),
            Some(e) => {
                out.push_str("    cases: &[\n");
                for case in &e.cases {
                    let _ = writeln!(
                        out,
                        "        EnumCaseSig {{ name: {}, value: {} }},",
                        rs_str(&case.name),
                        const_value(&case.value)
                    );
                }
                out.push_str("    ],\n");
            }
        }
        out.push_str("};\n\n");
        list.push(id);
    }
    emit_static_list(
        &mut out,
        "All classes of the extension, sorted by name.",
        "CLASSES",
        "ClassSig",
        &list,
    );
    Ok(out)
}

fn gen_mod(ext: &ExtData<'_>, label: &str) -> String {
    let mut out = String::new();
    header(
        &mut out,
        label,
        &format!("Generated descriptor tables of ext/{}: signatures, constants, ini defaults, class skeletons.", ext.name),
        "dead_code, unused_imports",
    );
    out.push_str("use rphp_ext_api::ExtInfo;\n\n");
    for m in ["arginfo", "classes", "consts", "ini"] {
        let _ = writeln!(out, "#[rustfmt::skip]\npub mod {m};");
    }
    out.push('\n');
    let version = ext.info.map_or("", |i| i.version.as_str());
    let deps: Vec<String> = ext
        .info
        .map(|i| {
            i.dependencies
                .iter()
                .filter(|(_, kind)| kind.as_str() == "Required")
                .map(|(n, _)| rs_str(n))
                .collect()
        })
        .unwrap_or_default();
    out.push_str(
        "/// `ReflectionExtension` metadata (name, `phpversion()`, required extensions).\n",
    );
    out.push_str("#[rustfmt::skip]\n");
    out.push_str("pub static INFO: ExtInfo = ExtInfo {\n");
    let _ = writeln!(out, "    name: {},", rs_str(ext.name));
    let _ = writeln!(out, "    version: {},", rs_str(version));
    let _ = writeln!(out, "    deps: &[{}],", deps.join(", "));
    out.push_str("};\n");
    out
}

/// Generate all files for one extension: file name → contents.
fn generate(ext: &ExtData<'_>, label: &str) -> Result<BTreeMap<&'static str, String>, String> {
    let (arginfo, method_ids) = gen_arginfo(ext, label)?;
    let mut files = BTreeMap::new();
    files.insert("mod.rs", gen_mod(ext, label));
    files.insert("arginfo.rs", arginfo);
    files.insert("consts.rs", gen_consts(ext, label));
    files.insert("ini.rs", gen_ini(ext, label));
    files.insert("classes.rs", gen_classes(ext, label, &method_ids)?);
    Ok(files)
}

// ---------------------------------------------------------------------------
// driver

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives one level below the workspace root")
        .to_path_buf()
}

fn resolve(root: &Path, p: &Path) -> PathBuf {
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        root.join(p)
    }
}

/// Where an extension's generated files live.
enum Dest {
    Dir(PathBuf),
    /// Crate directory missing and neither `--out` nor `--create` given.
    Skipped(PathBuf),
}

struct Options {
    manifest: PathBuf,
    exts: Vec<String>,
    out: Option<PathBuf>,
    create: bool,
    check: bool,
}

fn parse_args(args: &[String]) -> Result<Options, Box<dyn std::error::Error>> {
    let mut p = pico_args::Arguments::from_vec(args.iter().map(OsString::from).collect());
    let manifest: PathBuf = p
        .opt_value_from_str("--manifest")?
        .unwrap_or_else(|| PathBuf::from("manifest/php-8.5.0"));
    let exts: Vec<String> = p.values_from_str("--ext")?;
    let out: Option<PathBuf> = p.opt_value_from_str("--out")?;
    let create = p.contains("--create");
    let check = p.contains("--check");
    let rest = p.finish();
    if !rest.is_empty() {
        return Err(format!("unexpected arguments: {rest:?}").into());
    }
    Ok(Options {
        manifest,
        exts,
        out,
        create,
        check,
    })
}

/// `cargo xtask gen` entry point.
pub fn run(args: &[String]) -> XtaskResult {
    let opts = parse_args(args)?;
    let root = workspace_root();
    let manifest_dir = resolve(&root, &opts.manifest);
    let label = opts.manifest.to_string_lossy().replace('\\', "/");
    let manifest = Manifest::load(&manifest_dir)?;

    let ext_names: Vec<String> = if opts.exts.is_empty() {
        manifest.extensions.keys().cloned().collect()
    } else {
        opts.exts
            .iter()
            .map(|e| {
                manifest
                    .extension_name(e)
                    .map(str::to_string)
                    .ok_or_else(|| format!("extension `{e}` is not in {label}/extensions.json"))
            })
            .collect::<Result<_, _>>()?
    };
    let single = ext_names.len() == 1;

    let mut drift: Vec<String> = Vec::new();
    let mut written = 0usize;
    let mut skipped = 0usize;
    for name in &ext_names {
        let ext = ExtData::collect(&manifest, name);
        let dir_name = ext_dir_name(name);
        let dest = match &opts.out {
            Some(out) if single => Dest::Dir(resolve(&root, out)),
            Some(out) => Dest::Dir(resolve(&root, out).join(&dir_name)),
            None => {
                let crate_dir = root.join("crates/ext").join(format!("rphp-ext-{dir_name}"));
                if crate_dir.is_dir() || opts.create {
                    Dest::Dir(crate_dir.join("src/generated"))
                } else {
                    Dest::Skipped(crate_dir)
                }
            }
        };
        let files = generate(&ext, &label).map_err(|e| format!("{name}: {e}"))?;
        let summary = format!(
            "{name:<14} {:>4} fn {:>4} method {:>4} class {:>5} const {:>3} ini",
            ext.functions.len(),
            ext.method_count(),
            ext.classes.len(),
            ext.constants.len(),
            ext.ini.len()
        );
        match dest {
            Dest::Skipped(crate_dir) => {
                skipped += 1;
                println!(
                    "{summary}  -> skipped ({} missing; pass --create or --out)",
                    rel(&root, &crate_dir)
                );
            }
            Dest::Dir(dir) => {
                if opts.check {
                    let mut changed = Vec::new();
                    for (file, content) in &files {
                        let on_disk = fs::read_to_string(dir.join(file)).ok();
                        if on_disk.as_deref() != Some(content.as_str()) {
                            changed.push(file.to_string());
                        }
                    }
                    if let Ok(rd) = fs::read_dir(&dir) {
                        for entry in rd.flatten() {
                            let fname = entry.file_name().to_string_lossy().to_string();
                            if fname.ends_with(".rs") && !GENERATED_FILES.contains(&fname.as_str())
                            {
                                changed.push(format!("{fname} (stale)"));
                            }
                        }
                    }
                    if changed.is_empty() {
                        println!("{summary}  -> up to date ({})", rel(&root, &dir));
                    } else {
                        println!(
                            "{summary}  -> DRIFT in {}: {}",
                            rel(&root, &dir),
                            changed.join(", ")
                        );
                        drift.push(format!("{}: {}", rel(&root, &dir), changed.join(", ")));
                    }
                } else {
                    fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
                    for (file, content) in &files {
                        let path = dir.join(file);
                        if fs::read_to_string(&path).ok().as_deref() != Some(content.as_str()) {
                            fs::write(&path, content)
                                .map_err(|e| format!("{}: {e}", path.display()))?;
                        }
                    }
                    written += 1;
                    println!("{summary}  -> {}", rel(&root, &dir));
                }
            }
        }
    }
    if opts.check {
        if drift.is_empty() {
            println!(
                "gen --check: {} extension(s) up to date, {skipped} skipped",
                ext_names.len() - skipped
            );
            Ok(())
        } else {
            Err(format!(
                "generated files are out of date; run `cargo xtask gen`:\n  {}",
                drift.join("\n  ")
            )
            .into())
        }
    } else {
        println!("gen: {written} extension(s) written, {skipped} skipped");
        Ok(())
    }
}

fn rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .unwrap_or(p)
        .to_string_lossy()
        .replace('\\', "/")
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn manifest() -> Manifest {
        let root = workspace_root();
        Manifest::load(&root.join("manifest/php-8.5.0")).expect("manifest loads")
    }

    #[test]
    fn parses_type_strings() {
        let t = |s: &str| parse_type(Some(s));
        assert_eq!(
            t("?int"),
            Ty {
                mask: TypeMask::INT | TypeMask::NULL,
                class: None
            }
        );
        assert_eq!(
            t("array|string|null"),
            Ty {
                mask: TypeMask::ARRAY | TypeMask::STRING | TypeMask::NULL,
                class: None
            }
        );
        assert_eq!(
            t("Traversable|array"),
            Ty {
                mask: TypeMask::ARRAY,
                class: Some("Traversable".into())
            }
        );
        assert_eq!(
            t("RoundingMode|int"),
            Ty {
                mask: TypeMask::INT,
                class: Some("RoundingMode".into())
            }
        );
        assert_eq!(
            t("?Throwable"),
            Ty {
                mask: TypeMask::NULL,
                class: Some("Throwable".into())
            }
        );
        assert_eq!(
            t("mixed"),
            Ty {
                mask: TypeMask::MIXED,
                class: None
            }
        );
        assert_eq!(
            t("static"),
            Ty {
                mask: TypeMask::STATIC,
                class: None
            }
        );
        assert_eq!(
            t("(A&B)|null"),
            Ty {
                mask: TypeMask::NULL,
                class: Some("(A&B)".into())
            }
        );
        assert_eq!(
            t("Odbc\\Connection|Odbc\\Result"),
            Ty {
                mask: TypeMask::EMPTY,
                class: Some("Odbc\\Connection|Odbc\\Result".into())
            }
        );
        assert_eq!(
            parse_type(None),
            Ty {
                mask: TypeMask::EMPTY,
                class: None
            }
        );
        assert_eq!(
            rs_mask(TypeMask::INT | TypeMask::NULL),
            "TypeMask::INT.union(TypeMask::NULL)"
        );
        assert_eq!(rs_mask(TypeMask::BOOL), "TypeMask::BOOL");
        assert_eq!(
            rs_mask(TypeMask::INT | TypeMask::FALSE),
            "TypeMask::INT.union(TypeMask::FALSE)"
        );
        assert_eq!(rs_mask(TypeMask::EMPTY), "TypeMask::EMPTY");
    }

    #[test]
    fn identifiers_and_literals() {
        assert_eq!(ident("strlen"), "STRLEN");
        assert_eq!(ident("Random\\Randomizer"), "RANDOM__RANDOMIZER");
        assert_ne!(
            ident("Dom\\import_simplexml"),
            ident("dom_import_simplexml")
        );
        assert_eq!(ident("9foo"), "_9FOO");
        assert_eq!(ext_dir_name("Zend OPcache"), "zend_opcache");
        assert_eq!(ext_dir_name("PDO_ODBC"), "pdo_odbc");
        assert!(is_literal_expr("null"));
        assert!(is_literal_expr("-1"));
        assert!(is_literal_expr("1.0"));
        assert!(is_literal_expr("0x1F"));
        assert!(is_literal_expr("\"\""));
        assert!(is_literal_expr("[]"));
        assert!(!is_literal_expr("PHP_INT_MAX"));
        assert!(!is_literal_expr("ENT_QUOTES | ENT_HTML401"));
        assert!(!is_literal_expr("E_ALL"));
        assert_eq!(rs_f64(1.0), "1.0_f64");
        assert_eq!(rs_f64(f64::NAN), "f64::NAN");
        assert_eq!(rs_i64(i64::MIN), "i64::MIN");
        assert_eq!(rs_str("a\"b\\c\n"), "\"a\\\"b\\\\c\\n\"");
        assert_eq!(rs_ini_access(7), "IniAccess::ALL");
        assert_eq!(
            rs_ini_access(6),
            "IniAccess::PERDIR.union(IniAccess::SYSTEM)"
        );
        assert_eq!(rs_ini_access(0), "IniAccess::EMPTY");
    }

    #[test]
    fn generates_ctype_snapshot() {
        let m = manifest();
        let ext = ExtData::collect(&m, "ctype");
        let files = generate(&ext, "manifest/php-8.5.0").unwrap();
        let arginfo = &files["arginfo.rs"];
        assert!(arginfo.starts_with(
            "// @generated by `cargo xtask gen` from manifest/php-8.5.0 — do not edit.\n"
        ));
        assert!(arginfo.contains("/// `ctype_alpha(mixed $text): bool`\n"));
        assert!(arginfo
            .contains("pub static CTYPE_ALPHA: FnSig = FnSig {\n    name: \"ctype_alpha\",\n"));
        assert!(arginfo.contains(
            "        ParamInfo { name: \"text\", ty: TypeMask::MIXED, class: None, by_ref: false, prefer_ref: false, variadic: false, nullable: true, default: None },\n"
        ));
        assert!(
            arginfo.contains("    required: 1,\n    ret: TypeMask::BOOL,\n    ret_class: None,\n")
        );
        assert!(arginfo.contains(
            "pub static FUNCTIONS: &[&FnSig] = &[\n    &CTYPE_ALNUM,\n    &CTYPE_ALPHA,\n"
        ));
        assert!(arginfo.contains("pub static METHODS: &[&FnSig] = &[];\n"));
        assert!(files["consts.rs"].contains("pub static CONSTANTS: &[ConstDef] = &[];\n"));
        assert!(files["ini.rs"].contains("pub static INI: &[IniDef] = &[];\n"));
        assert!(files["classes.rs"].contains("pub static CLASSES: &[&ClassSig] = &[];\n"));
        assert!(files["mod.rs"].contains("pub static INFO: ExtInfo = ExtInfo {\n    name: \"ctype\",\n    version: \"8.5.0\",\n    deps: &[],\n};\n"));
        assert!(files["mod.rs"].contains("#[rustfmt::skip]\npub mod arginfo;\n"));
    }

    #[test]
    fn generates_core_details() {
        let m = manifest();
        let files = generate(&ExtData::collect(&m, "Core"), "manifest/php-8.5.0").unwrap();
        let consts = &files["consts.rs"];
        assert!(consts.contains(
            "ConstDef { name: \"PHP_BINARY\", value: ConstValue::Runtime, deprecated: false },"
        ));
        assert!(consts.contains(
            "ConstDef { name: \"STDIN\", value: ConstValue::Runtime, deprecated: false },"
        ));
        assert!(consts.contains("ConstDef { name: \"PHP_INT_MAX\", value: ConstValue::Int(9223372036854775807), deprecated: false },"));
        assert!(consts.contains("ConstDef { name: \"PHP_INT_MIN\", value: ConstValue::Int(i64::MIN), deprecated: false },"));
        assert!(consts.contains(
            "ConstDef { name: \"PHP_EOL\", value: ConstValue::Str(\"\\n\"), deprecated: false },"
        ));
        assert!(consts.contains(
            "ConstDef { name: \"E_ALL\", value: ConstValue::Int(30719), deprecated: false },"
        ));
        let ini = &files["ini.rs"];
        assert!(ini.contains(
            "IniDef { name: \"display_errors\", default: Some(\"1\"), access: IniAccess::ALL },"
        ));
        assert!(ini.contains("IniDef { name: \"allow_url_fopen\", default: Some(\"1\"), access: IniAccess::SYSTEM },"));
        let classes = &files["classes.rs"];
        assert!(classes.contains("/// `class Exception implements Throwable`\npub static EXCEPTION: ClassSig = ClassSig {"));
        assert!(classes.contains("PropSig { name: \"previous\", ty: TypeMask::NULL, class: Some(\"Throwable\"), vis: Vis::Private, is_static: false, readonly: false, default: Some(DefaultVal::Null) },"));
        assert!(classes.contains("MethodSig { sig: &arginfo::EXCEPTION__GETMESSAGE, vis: Vis::Public, is_static: false, is_abstract: false, is_final: true },"));
        assert!(classes.contains("MethodSig { sig: &arginfo::THROWABLE__GETMESSAGE, vis: Vis::Public, is_static: false, is_abstract: true, is_final: false },"));
        let arginfo = &files["arginfo.rs"];
        assert!(arginfo.contains("pub static EXCEPTION__GETMESSAGE: FnSig = FnSig {\n    name: \"exception::getmessage\",\n"));
        // unevaluated constant-expression defaults keep their stub text
        let standard = generate(&ExtData::collect(&m, "standard"), "manifest/php-8.5.0").unwrap();
        assert!(standard["arginfo.rs"].contains(
            "default: Some(DefaultVal::Const(\"ENT_QUOTES | ENT_SUBSTITUTE | ENT_HTML401\"))"
        ));
        assert!(standard["arginfo.rs"].contains("name: \"limit\", ty: TypeMask::INT, class: None, by_ref: false, prefer_ref: false, variadic: false, nullable: false, default: Some(DefaultVal::Const(\"PHP_INT_MAX\")) },"));
        assert!(standard["arginfo.rs"].contains("name: \"mode\", ty: TypeMask::INT, class: Some(\"RoundingMode\"), by_ref: false, prefer_ref: false, variadic: false, nullable: false, default: Some(DefaultVal::Const(\"RoundingMode::HalfAwayFromZero\")) },"));
        assert!(standard["consts.rs"].contains(
            "ConstDef { name: \"NAN\", value: ConstValue::Float(f64::NAN), deprecated: false },"
        ));
        assert!(standard["consts.rs"].contains("ConstDef { name: \"M_PI\", value: ConstValue::Float(3.141592653589793_f64), deprecated: false },"));
        let date = generate(&ExtData::collect(&m, "date"), "manifest/php-8.5.0").unwrap();
        assert!(date["consts.rs"].contains("ConstDef { name: \"SUNFUNCS_RET_STRING\", value: ConstValue::Int(1), deprecated: true },"));
        let json = generate(&ExtData::collect(&m, "json"), "manifest/php-8.5.0").unwrap();
        assert!(json["classes.rs"].contains("/// `class JsonException extends Exception`\npub static JSONEXCEPTION: ClassSig = ClassSig {\n    name: \"JsonException\",\n    kind: ClassKind::Class,\n    flags: ClassSigFlags::EMPTY,\n    parent: Some(\"Exception\"),\n    interfaces: &[],\n"));
        assert!(json["classes.rs"]
            .contains("/// `interface JsonSerializable`\npub static JSONSERIALIZABLE: ClassSig"));
        let reflection =
            generate(&ExtData::collect(&m, "Reflection"), "manifest/php-8.5.0").unwrap();
        let classes = &reflection["classes.rs"];
        assert!(classes.contains("/// `enum PropertyHookType: string implements BackedEnum`\npub static PROPERTYHOOKTYPE: ClassSig"));
        assert!(classes.contains("    kind: ClassKind::Enum,\n"));
        assert!(classes.contains("    backing: TypeMask::STRING,\n    cases: &[\n        EnumCaseSig { name: \"Get\", value: ConstValue::Str(\"get\") },\n        EnumCaseSig { name: \"Set\", value: ConstValue::Str(\"set\") },\n    ],\n"));
        let pure = &standard["classes.rs"];
        assert!(pure.contains("/// `enum RoundingMode implements UnitEnum`"));
        assert!(pure.contains("    backing: TypeMask::EMPTY,\n    cases: &[\n        EnumCaseSig { name: \"HalfAwayFromZero\", value: ConstValue::Null },"));
    }

    #[test]
    fn every_extension_generates_deterministically() {
        let m = manifest();
        for name in m.extensions.keys() {
            let ext = ExtData::collect(&m, name);
            let a = generate(&ext, "manifest/php-8.5.0").unwrap_or_else(|e| panic!("{name}: {e}"));
            let b = generate(&ext, "manifest/php-8.5.0").unwrap();
            assert_eq!(a, b, "{name} is not deterministic");
        }
    }

    /// Writes the generated `ctype` and `json` tables into a throwaway crate
    /// that depends on `rphp-ext-api` and runs `cargo check` on it.
    #[test]
    fn generated_code_compiles() {
        let Ok(cargo) = std::env::var("CARGO") else {
            eprintln!("skipping: CARGO not set");
            return;
        };
        let root = workspace_root();
        let m = manifest();
        let tmp = tempfile::tempdir().unwrap();
        let crate_dir = tmp.path().join("gen-check");
        let src = crate_dir.join("src");
        let mut lib = String::from("#![forbid(unsafe_code)]\n");
        // alphabetical so rustfmt's `reorder_modules` leaves lib.rs alone
        for name in ["Core", "ctype", "json"] {
            let ext = ExtData::collect(&m, name);
            let dir_name = ext_dir_name(name);
            let dir = src.join(&dir_name).join("generated");
            fs::create_dir_all(&dir).unwrap();
            for (file, content) in generate(&ext, "manifest/php-8.5.0").unwrap() {
                fs::write(dir.join(file), content).unwrap();
            }
            fs::write(src.join(&dir_name).join("mod.rs"), "pub mod generated;\n").unwrap();
            let _ = writeln!(lib, "pub mod {dir_name};");
        }
        lib.push_str(
            "\n/// Touch the tables so nothing is optimised away and the types line up.\n\
             pub fn smoke() -> usize {\n    ctype::generated::arginfo::ALL.len()\n        + json::generated::classes::CLASSES.len()\n        + core::generated::consts::CONSTANTS.len()\n        + core::generated::ini::INI.len()\n        + core::generated::INFO.deps.len()\n}\n",
        );
        fs::write(src.join("lib.rs"), lib).unwrap();
        fs::write(
            crate_dir.join("Cargo.toml"),
            format!(
                "[package]\nname = \"gen-check\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n[dependencies]\nrphp-ext-api = {{ path = {:?} }}\n\n[workspace]\n",
                root.join("crates/rphp-ext-api").to_string_lossy()
            ),
        )
        .unwrap();
        if let Ok(tc) = fs::read(root.join("rust-toolchain.toml")) {
            fs::write(crate_dir.join("rust-toolchain.toml"), tc).unwrap();
        }
        let status = Command::new(&cargo)
            .args(["check", "--quiet", "--manifest-path"])
            .arg(crate_dir.join("Cargo.toml"))
            .arg("--target-dir")
            .arg(tmp.path().join("target"))
            .env_remove("RUSTFLAGS")
            .env_remove("CARGO_TARGET_DIR")
            .status()
            .expect("run cargo check");
        assert!(status.success(), "generated code does not compile");
        // `cargo fmt` must be a no-op: the generated modules are `#[rustfmt::skip]`.
        let fmt = Command::new(&cargo)
            .args(["fmt", "--check", "--manifest-path"])
            .arg(crate_dir.join("Cargo.toml"))
            .status();
        if let Ok(fmt) = fmt {
            assert!(fmt.success(), "cargo fmt would reformat the generated code");
        }
    }
}
