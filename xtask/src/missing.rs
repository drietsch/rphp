//! `cargo xtask missing` — which internal PHP surface does a code tree need
//! that rphp does not implement yet?
//!
//! Every `*.php` file under the given roots is tokenized in parallel with
//! [`rphp_tokenizer::tokenize`] (PHP-identical tokens) and walked by a small
//! token-level heuristic scanner ([`scan_source`]) that extracts
//!
//! * **function calls** — a `T_STRING`/`T_NAME_FULLY_QUALIFIED` followed by
//!   `(` whose previous significant token is not `->`/`?->`/`::`/`function`/
//!   `new`/`const`/`use`/`namespace`/`as`/`insteadof`, and that is not an
//!   attribute name (`#[Foo(...)]`); `\name` is normalized to `name`;
//!   `use function a\b as c;` aliases are honoured; relative qualified calls
//!   (`Foo\bar()`) are skipped because they cannot be resolved statically;
//! * **class references** — names after `new`/`instanceof`/`extends`/
//!   `implements`/`catch (`/`use` (imports and trait uses), before `::`
//!   (except `self`/`parent`), attribute names, every
//!   `T_NAME_FULLY_QUALIFIED` that is neither a call nor a manifest constant,
//!   and names in **type positions**: a type chain (`?A|B&C`) that starts after
//!   `(`, `,`, `?`, a visibility modifier, `readonly`, `static`, `var`, or the
//!   `:` of a return type and ends at a variable (`$x`, `&$x`, `...$x`) or, for
//!   return types, at `{`, `;` or `=>`. Names are resolved with PHP's rules:
//!   the current `namespace`, `use` imports and aliases, no global fallback for
//!   classes. Unresolvable `namespace\Foo` (`T_NAME_RELATIVE`) is skipped;
//! * **constants** — a bare `T_STRING` (or `\NAME`) that is not followed by `(`
//!   or `::`, not a member access, not a declaration, and **is a name in the
//!   manifest's constants list** (so the heuristic cannot misfire on user
//!   identifiers), plus the string argument of `constant('X')`/`defined('X')`;
//! * **guards** — `function_exists('x')`, `class_exists`/`interface_exists`/
//!   `trait_exists`/`enum_exists('X')` (also `X::class`), `extension_loaded('x')`
//!   and `defined('X')`. A symbol whose every using file also guards it (or
//!   guards its whole extension) is reported as *optional* rather than
//!   blocking;
//! * **polyfills** — global `function name(`, `class Name`, `define('NAME'`
//!   declarations of internal names (Symfony's polyfill packages) are tagged
//!   `polyfill`: rphp does not strictly need them for the tree to run.
//!
//! Names are then intersected with the PHP 8.5.0 oracle manifest
//! (`manifest/php-8.5.0/{functions,classes,constants}.json`, which tags each
//! symbol with its extension) and the registry of what rphp implements is
//! subtracted. Functions come from the live `rphp-stdlib` registry (see
//! [`implemented_functions`]); until native classes and constants exist those
//! two registries are empty and the output says so. `--registry <json>` swaps
//! in an explicit `{ "functions": [...], "classes": [...], "constants": [...] }`
//! dump instead.
//!
//! Known limits of the token heuristics (all documented, all "best effort"):
//! class names inside strings (`autoload_classmap.php`, `'App\\Foo'`) are not
//! seen; unqualified function calls in a namespace are assumed to fall back to
//! the global function even when the namespace defines a same-named function;
//! DNF types `(A&B)|null` only see the first member; enum `case E_ALL;`
//! declarations are excluded but `switch` `case E_ALL:` counts as a use.
//!
//! A machine-readable copy of every run lands in `target/missing-report.json`.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ffi::OsString;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use rayon::prelude::*;
use rphp_tokenizer::{ids, is_ignorable, tokenize, Options, RawToken};
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::XtaskResult;

const HELP: &str = "\
cargo xtask missing — internal PHP functions/classes/constants a code tree uses that rphp lacks

USAGE:
    cargo xtask missing --dir <path> [--dir <path>...] [options]

OPTIONS:
    --dir <path>           root to scan (repeatable; files or directories)
    --include-tests        do not skip /Tests/, /tests/ and /Resources/skeleton/
    --manifest <dir>       oracle manifest (default: manifest/php-8.5.0)
    --registry <json>      what rphp implements, as {\"functions\":[..],\"classes\":[..],\"constants\":[..]}
                           (default: the live rphp-stdlib registry for functions; no classes/constants yet)
    --top <N>              rows to print (default: 60; 0 = all)
    --ext <name>           only symbols of this extension (case-insensitive)
    --report <text|json|md> output format (default: text); md is a COVERAGE.md-ready table
    --by-file              list the files using each symbol
    --optional             also show symbols that only appear behind function_exists()/
                           class_exists()/extension_loaded()/defined() guards
    -h, --help             this help

The JSON report is always written to target/missing-report.json.
";

// ---------------------------------------------------------------------------
// Registry access
// ---------------------------------------------------------------------------

/// The set of internal function names (canonical manifest spelling) that the
/// live `rphp-stdlib` registry implements, out of `candidates`.
///
/// This is the **only** place the tool touches the registry. Today
/// `rphp_stdlib::table()` is private, so the registry is probed one manifest
/// name at a time through `rphp_stdlib::resolve` (case-insensitive, exactly
/// what the compiler does). When `rphp_stdlib::all_functions()` lands, this
/// body becomes a one-liner over that iterator and `candidates` is ignored.
fn implemented_functions<'a>(candidates: impl IntoIterator<Item = &'a str>) -> BTreeSet<String> {
    candidates
        .into_iter()
        .filter(|name| rphp_stdlib::resolve(name.as_bytes()).is_some())
        .map(str::to_owned)
        .collect()
}

/// What rphp implements, keyed the same way as the manifest lookups
/// (functions and classes lower-cased, constants exact).
#[derive(Debug, Default)]
pub struct Registry {
    /// Human-readable provenance for the report header.
    pub label: String,
    functions: BTreeSet<String>,
    classes: BTreeSet<String>,
    constants: BTreeSet<String>,
    /// True when the class/constant sets are empty only because no native
    /// registry exists yet (so the report can label them honestly).
    classes_constants_unavailable: bool,
}

#[derive(Deserialize)]
struct RegistryFile {
    #[serde(default)]
    functions: Vec<String>,
    #[serde(default)]
    classes: Vec<String>,
    #[serde(default)]
    constants: Vec<String>,
}

impl Registry {
    /// The live registry: functions from `rphp-stdlib`, no classes/constants.
    pub fn live(manifest: &Manifest) -> Registry {
        let functions = implemented_functions(manifest.functions.values().map(|s| s.name.as_str()))
            .into_iter()
            .map(|n| n.to_ascii_lowercase())
            .collect();
        Registry {
            label: "live rphp-stdlib registry (functions); classes/constants: no native registry yet, every internal class/constant used is reported missing".into(),
            functions,
            classes: BTreeSet::new(),
            constants: BTreeSet::new(),
            classes_constants_unavailable: true,
        }
    }

    /// Load an explicit `{ "functions": [...], "classes": [...], "constants": [...] }` file.
    pub fn from_json(path: &Path) -> Result<Registry, Box<dyn std::error::Error>> {
        let bytes = fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let file: RegistryFile =
            serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Registry {
            label: path.display().to_string(),
            functions: file
                .functions
                .iter()
                .map(|n| n.trim_start_matches('\\').to_ascii_lowercase())
                .collect(),
            classes: file
                .classes
                .iter()
                .map(|n| n.trim_start_matches('\\').to_ascii_lowercase())
                .collect(),
            constants: file
                .constants
                .iter()
                .map(|n| n.trim_start_matches('\\').to_string())
                .collect(),
            classes_constants_unavailable: false,
        })
    }

    fn implements(&self, kind: Kind, canonical: &str) -> bool {
        match kind {
            Kind::Function => self.functions.contains(&canonical.to_ascii_lowercase()),
            Kind::Class => self.classes.contains(&canonical.to_ascii_lowercase()),
            Kind::Constant => self.constants.contains(canonical),
        }
    }
}

// ---------------------------------------------------------------------------
// Manifest
// ---------------------------------------------------------------------------

/// One internal symbol as the manifest knows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManifestSymbol {
    /// Canonical spelling (`preg_match`, `ArrayObject`, `PHP_EOL`).
    pub name: String,
    /// Owning extension (`pcre`, `SPL`, `Core`, …).
    pub extension: String,
}

/// The PHP 8.5.0 oracle manifest, indexed for lookups: functions and classes
/// by lower-cased name (PHP resolves both case-insensitively), constants by
/// exact name.
#[derive(Debug, Default)]
pub struct Manifest {
    /// Directory the manifest was loaded from.
    pub dir: PathBuf,
    functions: HashMap<String, ManifestSymbol>,
    classes: HashMap<String, ManifestSymbol>,
    constants: HashMap<String, ManifestSymbol>,
}

#[derive(Deserialize)]
struct FnEntry {
    extension: String,
}

#[derive(Deserialize)]
struct ClassEntry {
    extension: String,
}

impl Manifest {
    /// Load `functions.json`, `classes.json` and `constants.json` from `dir`.
    pub fn load(dir: &Path) -> Result<Manifest, Box<dyn std::error::Error>> {
        let read = |file: &str| -> Result<Vec<u8>, Box<dyn std::error::Error>> {
            let p = dir.join(file);
            fs::read(&p).map_err(|e| format!("{}: {e}", p.display()).into())
        };
        let functions: BTreeMap<String, FnEntry> = serde_json::from_slice(&read("functions.json")?)?;
        let classes: BTreeMap<String, ClassEntry> = serde_json::from_slice(&read("classes.json")?)?;
        let constants: BTreeMap<String, BTreeMap<String, serde::de::IgnoredAny>> =
            serde_json::from_slice(&read("constants.json")?)?;
        let mut m = Manifest {
            dir: dir.to_path_buf(),
            ..Manifest::default()
        };
        for (name, e) in functions {
            m.functions.insert(
                name.to_ascii_lowercase(),
                ManifestSymbol {
                    name,
                    extension: e.extension,
                },
            );
        }
        for (name, e) in classes {
            m.classes.insert(
                name.to_ascii_lowercase(),
                ManifestSymbol {
                    name,
                    extension: e.extension,
                },
            );
        }
        for (ext, consts) in constants {
            for name in consts.into_keys() {
                m.constants.insert(
                    name.clone(),
                    ManifestSymbol {
                        name,
                        extension: ext.clone(),
                    },
                );
            }
        }
        Ok(m)
    }

    /// Look up a function by (case-insensitive) name, without leading `\`.
    pub fn function(&self, name: &str) -> Option<&ManifestSymbol> {
        self.functions.get(&name.to_ascii_lowercase())
    }

    /// Look up a class/interface/trait/enum by (case-insensitive) name.
    pub fn class(&self, name: &str) -> Option<&ManifestSymbol> {
        self.classes.get(&name.to_ascii_lowercase())
    }

    /// Look up a constant by exact name.
    pub fn constant(&self, name: &str) -> Option<&ManifestSymbol> {
        self.constants.get(name)
    }

    /// Whether `name` is an internal constant (the scanner's filter).
    pub fn is_constant(&self, name: &str) -> bool {
        self.constants.contains_key(name)
    }

    /// Number of internal functions, classes, constants.
    pub fn counts(&self) -> (usize, usize, usize) {
        (self.functions.len(), self.classes.len(), self.constants.len())
    }
}

// ---------------------------------------------------------------------------
// Token scanner
// ---------------------------------------------------------------------------

/// Everything the scanner extracted from one file.
///
/// Function and class names are lower-cased, fully resolved (namespace and
/// `use` imports applied) and carry no leading `\`; constants are exact.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileScan {
    /// Function call sites by resolved name.
    pub functions: BTreeMap<String, u32>,
    /// Class references by resolved name.
    pub classes: BTreeMap<String, u32>,
    /// Internal-constant uses by exact name (only names the manifest knows).
    pub constants: BTreeMap<String, u32>,
    /// Existence guards seen in the file.
    pub guards: Guards,
    /// Global declarations of internal names (polyfills).
    pub defines: Defines,
}

/// Names the file checks for before using them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Guards {
    /// `function_exists('x')` (lower-cased).
    pub functions: BTreeSet<String>,
    /// `class_exists`/`interface_exists`/`trait_exists`/`enum_exists('X')` (lower-cased).
    pub classes: BTreeSet<String>,
    /// `defined('X')` (exact).
    pub constants: BTreeSet<String>,
    /// `extension_loaded('x')` (lower-cased).
    pub extensions: BTreeSet<String>,
}

/// Global-namespace declarations the file makes (polyfill detection).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Defines {
    /// `function name(` at the global namespace (lower-cased).
    pub functions: BTreeSet<String>,
    /// `class|interface|trait|enum Name` at the global namespace (lower-cased).
    pub classes: BTreeSet<String>,
    /// `define('NAME', …)` (exact).
    pub constants: BTreeSet<String>,
}

/// Words that can appear where a class name is expected but never name one.
const RESERVED_TYPE_WORDS: &[&str] = &[
    "self", "parent", "static", "array", "callable", "int", "float", "bool", "string",
    "iterable", "object", "mixed", "void", "null", "never", "false", "true",
];

fn is_reserved_type_word(name: &str) -> bool {
    RESERVED_TYPE_WORDS
        .iter()
        .any(|w| w.eq_ignore_ascii_case(name))
}

const fn ch(c: u8) -> u16 {
    c as u16
}

fn is_name(id: u16) -> bool {
    matches!(
        id,
        ids::T_STRING | ids::T_NAME_QUALIFIED | ids::T_NAME_FULLY_QUALIFIED
    )
}

/// Tokens allowed inside a type chain besides names.
fn is_type_keyword(id: u16) -> bool {
    matches!(id, ids::T_STATIC | ids::T_ARRAY | ids::T_CALLABLE)
}

fn is_type_separator(id: u16) -> bool {
    id == ch(b'|') || id == ids::T_AMPERSAND_NOT_FOLLOWED_BY_VAR_OR_VARARG
}

fn is_modifier(id: u16) -> bool {
    matches!(
        id,
        ids::T_PUBLIC
            | ids::T_PROTECTED
            | ids::T_PRIVATE
            | ids::T_PUBLIC_SET
            | ids::T_PROTECTED_SET
            | ids::T_PRIVATE_SET
            | ids::T_READONLY
            | ids::T_STATIC
            | ids::T_VAR
    )
}

/// The value of a `T_CONSTANT_ENCAPSED_STRING` token when it is a plain
/// symbol name (`'Foo\\Bar'`, `"strlen"`, `b'x'`), with the leading `\`
/// dropped. `None` for anything with interpolation-grade escapes.
fn string_literal_name(text: &str) -> Option<String> {
    let body = text.trim_start_matches(['b', 'B']);
    let quote = body.chars().next()?;
    if !(quote == '\'' || quote == '"') || body.len() < 2 || !body.ends_with(quote) {
        return None;
    }
    let inner = &body[1..body.len() - 1];
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('\\') => out.push('\\'),
                Some(q) if q == quote => out.push(q),
                Some(other) if quote == '\'' => {
                    out.push('\\');
                    out.push(other);
                }
                _ => return None,
            }
        } else {
            out.push(c);
        }
    }
    let out = out.trim_start_matches('\\').to_string();
    if out.is_empty() || out.contains(|c: char| c.is_whitespace() || c == '$') {
        return None;
    }
    Some(out)
}

/// Which list construct the walker is inside.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ListMode {
    None,
    /// `catch (A | B $e)` — until `)` or the variable.
    Catch,
    /// `extends A implements B, C` — until `{`.
    ClassHeader,
}

/// Resolution scope: current namespace and `use` imports.
#[derive(Debug, Default)]
struct Scope {
    /// `"Foo\Bar\"` or `""` for the global namespace.
    ns_prefix: String,
    /// Namespace declared with braces (`namespace X { … }`), which shifts the
    /// depth at which `use` statements are imports.
    braced: bool,
    /// Import alias (lower-cased) → fully-qualified class name.
    classes: HashMap<String, String>,
    /// Import alias (lower-cased) → fully-qualified function name.
    functions: HashMap<String, String>,
    /// Import alias (exact) → fully-qualified constant name.
    constants: HashMap<String, String>,
}

impl Scope {
    fn reset(&mut self, ns: &str, braced: bool) {
        self.ns_prefix = if ns.is_empty() {
            String::new()
        } else {
            format!("{ns}\\")
        };
        self.braced = braced;
        self.classes.clear();
        self.functions.clear();
        self.constants.clear();
    }

    fn import_depth(&self) -> i32 {
        if self.braced {
            1
        } else {
            0
        }
    }

    /// Resolve a class name token to its fully-qualified spelling.
    fn resolve_class(&self, id: u16, raw: &str) -> Option<String> {
        match id {
            ids::T_NAME_FULLY_QUALIFIED => Some(raw[1..].to_string()),
            ids::T_NAME_QUALIFIED => {
                let (first, rest) = raw.split_once('\\')?;
                match self.classes.get(&first.to_ascii_lowercase()) {
                    Some(target) => Some(format!("{target}\\{rest}")),
                    None => Some(format!("{}{raw}", self.ns_prefix)),
                }
            }
            ids::T_STRING => {
                if is_reserved_type_word(raw) {
                    return None;
                }
                match self.classes.get(&raw.to_ascii_lowercase()) {
                    Some(target) => Some(target.clone()),
                    None => Some(format!("{}{raw}", self.ns_prefix)),
                }
            }
            _ => None,
        }
    }

    /// Resolve a function call name: fully-qualified as written, imports
    /// honoured, otherwise the global candidate (unqualified calls fall back to
    /// the global function at runtime).
    fn resolve_function(&self, id: u16, raw: &str) -> Option<String> {
        match id {
            ids::T_NAME_FULLY_QUALIFIED => Some(raw[1..].to_string()),
            ids::T_STRING => Some(
                self.functions
                    .get(&raw.to_ascii_lowercase())
                    .cloned()
                    .unwrap_or_else(|| raw.to_string()),
            ),
            _ => None,
        }
    }

    fn resolve_constant(&self, id: u16, raw: &str) -> Option<String> {
        match id {
            ids::T_NAME_FULLY_QUALIFIED => Some(raw[1..].to_string()),
            ids::T_STRING => Some(
                self.constants
                    .get(raw)
                    .cloned()
                    .unwrap_or_else(|| raw.to_string()),
            ),
            _ => None,
        }
    }
}

struct Walker<'a> {
    src: &'a [u8],
    /// Significant tokens only (whitespace, comments and open tags dropped).
    sig: Vec<RawToken>,
    is_constant: &'a dyn Fn(&str) -> bool,
    out: FileScan,
    scope: Scope,
    depth: i32,
    /// `Some(bracket depth)` while inside a `#[...]` attribute group.
    attr: Option<i32>,
    list: ListMode,
}

impl<'a> Walker<'a> {
    fn id(&self, i: isize) -> u16 {
        if i < 0 || i as usize >= self.sig.len() {
            0
        } else {
            self.sig[i as usize].id
        }
    }

    fn text(&self, i: usize) -> String {
        String::from_utf8_lossy(self.sig[i].text(self.src)).into_owned()
    }

    fn bump(map: &mut BTreeMap<String, u32>, key: String) {
        *map.entry(key).or_insert(0) += 1;
    }

    fn class_ref(&mut self, i: usize) {
        let raw = self.text(i);
        if let Some(fq) = self.scope.resolve_class(self.sig[i].id, &raw) {
            Self::bump(&mut self.out.classes, fq.to_ascii_lowercase());
        }
    }

    /// Parse a `use …;` statement starting at the `use` keyword (index `i`);
    /// returns the index of the token that ends it (`;`, or `{` for a trait
    /// adaptation block) so the caller resumes there.
    fn parse_use(&mut self, i: usize) -> usize {
        #[derive(Clone, Copy, PartialEq, Eq)]
        enum UseKind {
            Class,
            Function,
            Const,
        }
        let is_import = self.depth == self.scope.import_depth();
        let mut j = i + 1;
        let mut kind = match self.id(j as isize) {
            ids::T_FUNCTION => {
                j += 1;
                UseKind::Function
            }
            ids::T_CONST => {
                j += 1;
                UseKind::Const
            }
            _ => UseKind::Class,
        };
        let outer_kind = kind;
        let mut prefix: Option<String> = None;
        let mut in_group = false;
        loop {
            let id = self.id(j as isize);
            if !is_name(id) {
                // `}` closes a group; `;` ends; `{` (trait adaptations) ends.
                if in_group && id == ch(b'}') {
                    in_group = false;
                    prefix = None;
                    j += 1;
                    continue;
                }
                if id == ch(b',') {
                    j += 1;
                    kind = outer_kind;
                    continue;
                }
                if in_group && (id == ids::T_FUNCTION || id == ids::T_CONST) {
                    kind = if id == ids::T_FUNCTION {
                        UseKind::Function
                    } else {
                        UseKind::Const
                    };
                    j += 1;
                    continue;
                }
                return j.min(self.sig.len());
            }
            let raw = self.text(j);
            let name = raw.trim_start_matches('\\').to_string();
            // `Prefix\{ … }` group: the name is a namespace prefix.
            if self.id(j as isize + 1) == ids::T_NS_SEPARATOR && self.id(j as isize + 2) == ch(b'{') {
                prefix = Some(name);
                in_group = true;
                j += 3;
                continue;
            }
            let full = match &prefix {
                Some(p) => format!("{p}\\{name}"),
                None => name,
            };
            let mut alias = full.rsplit('\\').next().unwrap_or(&full).to_string();
            j += 1;
            if self.id(j as isize) == ids::T_AS && is_name(self.id(j as isize + 1)) {
                alias = self.text(j + 1);
                j += 2;
            }
            match kind {
                UseKind::Class => {
                    // An import (or trait use) is a reference to that class.
                    Self::bump(&mut self.out.classes, full.to_ascii_lowercase());
                    if is_import {
                        self.scope.classes.insert(alias.to_ascii_lowercase(), full);
                    }
                }
                UseKind::Function => {
                    if is_import {
                        self.scope.functions.insert(alias.to_ascii_lowercase(), full);
                    }
                }
                UseKind::Const => {
                    if is_import {
                        self.scope.constants.insert(alias, full);
                    }
                }
            }
        }
    }

    /// Record the guard/definition implied by a call to `fname` at index `i`
    /// (the name token; `i + 1` is `(`).
    fn note_special_call(&mut self, fname: &str, i: usize) {
        let arg_id = self.id(i as isize + 2);
        let literal = if arg_id == ids::T_CONSTANT_ENCAPSED_STRING {
            string_literal_name(&self.text(i + 2))
        } else if is_name(arg_id)
            && self.id(i as isize + 3) == ids::T_PAAMAYIM_NEKUDOTAYIM
            && self.id(i as isize + 4) == ids::T_CLASS
        {
            // class_exists(Foo::class)
            let raw = self.text(i + 2);
            self.scope.resolve_class(arg_id, &raw)
        } else {
            None
        };
        let Some(name) = literal else { return };
        match fname {
            "function_exists" => {
                self.out.guards.functions.insert(name.to_ascii_lowercase());
            }
            "class_exists" | "interface_exists" | "trait_exists" | "enum_exists" => {
                self.out.guards.classes.insert(name.to_ascii_lowercase());
            }
            "extension_loaded" => {
                self.out.guards.extensions.insert(name.to_ascii_lowercase());
            }
            "defined" => {
                if (self.is_constant)(&name) {
                    Self::bump(&mut self.out.constants, name.clone());
                }
                self.out.guards.constants.insert(name);
            }
            "constant" => {
                if (self.is_constant)(&name) {
                    Self::bump(&mut self.out.constants, name);
                }
            }
            "define" => {
                // define() always creates a global constant, whatever the namespace.
                self.out.defines.constants.insert(name);
            }
            _ => {}
        }
    }

    /// If a type chain starts at (or runs through) index `i`, return the
    /// indices of its name tokens and the index just past the chain.
    fn type_chain(&self, i: usize) -> Option<(Vec<usize>, usize)> {
        // Walk back to the head of the chain (`array|Foo` has a keyword head).
        let mut head = i as isize;
        while head >= 2
            && is_type_separator(self.id(head - 1))
            && (is_name(self.id(head - 2)) || is_type_keyword(self.id(head - 2)))
        {
            head -= 2;
        }
        let mut before = head - 1;
        if self.id(before) == ch(b'?') {
            // nullable `?Foo`; a ternary `?` is rejected below because what
            // precedes it is an expression, not `(`, `,`, `:` or a modifier.
            before -= 1;
        }
        let prev = self.id(before);
        let return_ctx = prev == ch(b':') && self.id(before - 1) == ch(b')');
        let start_ok = return_ctx || prev == ch(b'(') || prev == ch(b',') || is_modifier(prev);
        if !start_ok {
            return None;
        }
        let mut names = Vec::new();
        let mut j = head;
        loop {
            let id = self.id(j);
            if is_name(id) {
                names.push(j as usize);
            } else if !is_type_keyword(id) {
                return None;
            }
            j += 1;
            if is_type_separator(self.id(j)) && (is_name(self.id(j + 1)) || is_type_keyword(self.id(j + 1))) {
                j += 1;
                continue;
            }
            break;
        }
        let end = self.id(j);
        let ok = end == ids::T_VARIABLE
            || end == ids::T_ELLIPSIS
            || end == ids::T_AMPERSAND_FOLLOWED_BY_VAR_OR_VARARG
            || (return_ctx && (end == ch(b'{') || end == ch(b';') || end == ids::T_DOUBLE_ARROW));
        if ok && names.iter().any(|&n| n >= i) {
            Some((names, j as usize))
        } else {
            None
        }
    }

    fn run(mut self) -> FileScan {
        let n = self.sig.len();
        let mut i = 0usize;
        while i < n {
            let id = self.sig[i].id;
            let prev = self.id(i as isize - 1);
            let next = self.id(i as isize + 1);

            // --- bookkeeping that applies to every token ------------------
            match id {
                ids::T_CURLY_OPEN | ids::T_DOLLAR_OPEN_CURLY_BRACES => self.depth += 1,
                _ if id == ch(b'{') => {
                    self.depth += 1;
                    if self.list == ListMode::ClassHeader {
                        self.list = ListMode::None;
                    }
                }
                _ if id == ch(b'}') => self.depth -= 1,
                _ => {}
            }
            if let Some(d) = self.attr.as_mut() {
                if id == ch(b'(') || id == ch(b'[') {
                    *d += 1;
                } else if id == ch(b')') || id == ch(b']') {
                    *d -= 1;
                    if *d < 0 {
                        self.attr = None;
                    }
                }
            }
            match id {
                ids::T_ATTRIBUTE => {
                    self.attr = Some(0);
                    i += 1;
                    continue;
                }
                ids::T_CATCH => {
                    self.list = ListMode::Catch;
                    i += 1;
                    continue;
                }
                ids::T_EXTENDS | ids::T_IMPLEMENTS => {
                    self.list = ListMode::ClassHeader;
                    i += 1;
                    continue;
                }
                ids::T_NAMESPACE => {
                    if is_name(next) {
                        let ns = self.text(i + 1);
                        let braced = self.id(i as isize + 2) == ch(b'{');
                        self.scope.reset(&ns, braced);
                        i += 2;
                    } else if next == ch(b'{') {
                        self.scope.reset("", true);
                        i += 1;
                    } else {
                        i += 1;
                    }
                    continue;
                }
                ids::T_USE => {
                    if next == ch(b'(') {
                        // closure `use ($x)`
                        i += 1;
                        continue;
                    }
                    i = self.parse_use(i);
                    continue;
                }
                _ => {}
            }
            if (self.list == ListMode::Catch && (id == ch(b')') || id == ids::T_VARIABLE))
                || id == ch(b';')
            {
                self.list = ListMode::None;
            }
            if !is_name(id) {
                i += 1;
                continue;
            }

            // --- a name token ---------------------------------------------
            let raw = self.text(i);
            let bare = raw.trim_start_matches('\\');

            // Attribute name: `#[Foo(...)]`, `#[A, B]`.
            if self.attr == Some(0) && (prev == ids::T_ATTRIBUTE || prev == ch(b',')) {
                self.class_ref(i);
                i += 1;
                continue;
            }
            // Class lists.
            if self.list != ListMode::None {
                self.class_ref(i);
                i += 1;
                continue;
            }
            // `new Foo`, `$x instanceof Foo`.
            if prev == ids::T_NEW || prev == ids::T_INSTANCEOF {
                self.class_ref(i);
                i += 1;
                continue;
            }
            // Declarations and other non-uses.
            match prev {
                ids::T_FUNCTION => {
                    if id == ids::T_STRING && self.scope.ns_prefix.is_empty() {
                        self.out.defines.functions.insert(bare.to_ascii_lowercase());
                    }
                    i += 1;
                    continue;
                }
                ids::T_CLASS | ids::T_INTERFACE | ids::T_TRAIT | ids::T_ENUM => {
                    if id == ids::T_STRING && self.scope.ns_prefix.is_empty() {
                        self.out.defines.classes.insert(bare.to_ascii_lowercase());
                    }
                    i += 1;
                    continue;
                }
                ids::T_CONST | ids::T_GOTO | ids::T_AS | ids::T_INSTEADOF => {
                    i += 1;
                    continue;
                }
                ids::T_OBJECT_OPERATOR | ids::T_NULLSAFE_OBJECT_OPERATOR | ids::T_PAAMAYIM_NEKUDOTAYIM => {
                    i += 1;
                    continue;
                }
                _ => {}
            }
            // `function &foo(`
            if (prev == ids::T_AMPERSAND_FOLLOWED_BY_VAR_OR_VARARG
                || prev == ids::T_AMPERSAND_NOT_FOLLOWED_BY_VAR_OR_VARARG)
                && self.id(i as isize - 2) == ids::T_FUNCTION
            {
                if id == ids::T_STRING && self.scope.ns_prefix.is_empty() {
                    self.out.defines.functions.insert(bare.to_ascii_lowercase());
                }
                i += 1;
                continue;
            }
            // Enum case declaration `case Foo;` / `case Foo = …;`.
            if prev == ids::T_CASE && (next == ch(b';') || next == ch(b'=')) {
                i += 1;
                continue;
            }
            // Function call.
            if next == ch(b'(') {
                if let Some(resolved) = self.scope.resolve_function(id, &raw) {
                    let lower = resolved.to_ascii_lowercase();
                    self.note_special_call(&lower, i);
                    Self::bump(&mut self.out.functions, lower);
                }
                i += 1;
                continue;
            }
            // `Foo::…`
            if next == ids::T_PAAMAYIM_NEKUDOTAYIM {
                self.class_ref(i);
                i += 1;
                continue;
            }
            // Internal constant (manifest-filtered).
            if !(next == ids::T_VARIABLE || next == ids::T_ELLIPSIS) {
                if let Some(resolved) = self.scope.resolve_constant(id, &raw) {
                    if (self.is_constant)(&resolved) && !is_reserved_type_word(&resolved) {
                        Self::bump(&mut self.out.constants, resolved);
                        i += 1;
                        continue;
                    }
                }
            }
            // Fully-qualified name anywhere else is a class reference.
            if id == ids::T_NAME_FULLY_QUALIFIED {
                self.class_ref(i);
                i += 1;
                continue;
            }
            // Type positions.
            if let Some((names, end)) = self.type_chain(i) {
                for k in names {
                    if k >= i {
                        self.class_ref(k);
                    }
                }
                i = end.max(i + 1);
                continue;
            }
            i += 1;
        }
        self.out
    }
}

/// Scan one PHP source. `is_constant` tells the scanner which bare words are
/// internal constants (normally [`Manifest::is_constant`]); everything else
/// needs no manifest.
pub fn scan_source(src: &[u8], is_constant: &dyn Fn(&str) -> bool) -> FileScan {
    let sig: Vec<RawToken> = tokenize(src, Options::default())
        .into_iter()
        .filter(|t| !is_ignorable(t.id))
        .collect();
    let walker = Walker {
        src,
        sig,
        is_constant,
        out: FileScan::default(),
        scope: Scope::default(),
        depth: 0,
        attr: None,
        list: ListMode::None,
    };
    walker.run()
}

// ---------------------------------------------------------------------------
// File discovery
// ---------------------------------------------------------------------------

/// Whether a path relative to its scan root is a test/skeleton file that the
/// default scan skips.
pub fn is_skipped_path(rel: &Path) -> bool {
    let comps: Vec<&str> = rel
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect();
    // The file itself is never a skip marker: only its directories are.
    let dirs = &comps[..comps.len().saturating_sub(1)];
    dirs.iter().any(|c| *c == "Tests" || *c == "tests")
        || dirs.windows(2).any(|w| w == ["Resources", "skeleton"])
}

fn is_php_file(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("php"))
}

/// One file to scan: its absolute path and the label used in reports
/// (`<root basename>/<relative path>`).
#[derive(Clone, Debug)]
struct Candidate {
    path: PathBuf,
    label: String,
}

fn discover(roots: &[PathBuf], include_tests: bool) -> Result<(Vec<Candidate>, usize), Box<dyn std::error::Error>> {
    let mut out = Vec::new();
    let mut skipped = 0usize;
    for root in roots {
        if !root.exists() {
            return Err(format!("--dir {}: no such file or directory", root.display()).into());
        }
        let root_name = root
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| root.display().to_string());
        if root.is_file() {
            out.push(Candidate {
                path: root.clone(),
                label: root_name,
            });
            continue;
        }
        for entry in WalkDir::new(root).sort_by_file_name().into_iter().filter_map(Result::ok) {
            if !entry.file_type().is_file() || !is_php_file(entry.path()) {
                continue;
            }
            let rel = entry.path().strip_prefix(root).unwrap_or(entry.path());
            if !include_tests && is_skipped_path(rel) {
                skipped += 1;
                continue;
            }
            out.push(Candidate {
                path: entry.path().to_path_buf(),
                label: format!("{root_name}/{}", rel.to_string_lossy().replace('\\', "/")),
            });
        }
    }
    Ok((out, skipped))
}

// ---------------------------------------------------------------------------
// Aggregation
// ---------------------------------------------------------------------------

/// Symbol kind.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// Internal function.
    #[default]
    Function,
    /// Internal class, interface, trait or enum.
    Class,
    /// Internal constant.
    Constant,
}

impl Kind {
    fn label(self) -> &'static str {
        match self {
            Kind::Function => "function",
            Kind::Class => "class",
            Kind::Constant => "constant",
        }
    }
    fn plural(self) -> &'static str {
        match self {
            Kind::Function => "functions",
            Kind::Class => "classes",
            Kind::Constant => "constants",
        }
    }
}

/// Aggregated use of one internal symbol across the tree.
#[derive(Clone, Debug, Default, Serialize)]
pub struct SymbolUse {
    /// Canonical name from the manifest.
    pub symbol: String,
    /// Symbol kind.
    pub kind: Kind,
    /// Owning extension.
    pub ext: String,
    /// Total call sites / references.
    pub call_sites: u32,
    /// Number of files using it.
    pub files: usize,
    /// Whether rphp implements it.
    pub implemented: bool,
    /// Every using file also guards it (`function_exists`, …).
    pub optional: bool,
    /// The tree declares it itself (polyfill).
    pub polyfilled: bool,
    /// Per-file counts (label → call sites).
    pub by_file: BTreeMap<String, u32>,
    #[serde(skip)]
    guarded_files: usize,
}

/// Per-kind totals for the summary line.
#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct KindSummary {
    /// Distinct internal symbols used.
    pub used: usize,
    /// … of which rphp implements.
    pub implemented: usize,
    /// … of which rphp lacks (optional ones included).
    pub missing: usize,
    /// … of the missing ones, only used behind guards.
    pub optional: usize,
    /// … of the missing ones, declared by the tree itself.
    pub polyfilled: usize,
}

/// Per-extension totals of missing symbols.
#[derive(Clone, Debug, Default, Serialize)]
pub struct ExtTotals {
    /// Missing symbols (all kinds, optional included).
    pub missing: usize,
    /// … of which optional.
    pub optional: usize,
    /// Call sites of the missing symbols.
    pub call_sites: u32,
}

/// The whole report (also serialized to `target/missing-report.json`).
#[derive(Debug, Default, Serialize)]
pub struct Report {
    /// Scan roots as given.
    pub dirs: Vec<String>,
    /// Files tokenized.
    pub files_scanned: usize,
    /// Files skipped by the tests/skeleton rule.
    pub files_skipped: usize,
    /// Files that could not be read.
    pub files_unreadable: usize,
    /// Manifest directory.
    pub manifest: String,
    /// Registry provenance.
    pub registry: String,
    /// Whether classes/constants were compared against an empty registry.
    pub registry_lacks_classes_constants: bool,
    /// Distinct non-internal (user) function names / class names seen — a
    /// sanity signal for the heuristics.
    pub user_functions: usize,
    /// See `user_functions`.
    pub user_classes: usize,
    /// Totals per kind.
    pub summary: BTreeMap<String, KindSummary>,
    /// Totals per extension (missing only).
    pub by_extension: BTreeMap<String, ExtTotals>,
    /// Every internal symbol used, ranked (missing first, then by call sites).
    pub symbols: Vec<SymbolUse>,
    /// Seconds spent.
    pub seconds: f64,
}

struct Aggregate {
    uses: BTreeMap<(Kind, String), SymbolUse>,
    polyfilled: BTreeSet<(Kind, String)>,
    user_functions: BTreeSet<String>,
    user_classes: BTreeSet<String>,
}

impl Aggregate {
    fn new() -> Self {
        Aggregate {
            uses: BTreeMap::new(),
            polyfilled: BTreeSet::new(),
            user_functions: BTreeSet::new(),
            user_classes: BTreeSet::new(),
        }
    }

    fn add(&mut self, manifest: &Manifest, label: &str, scan: &FileScan) {
        let ext_guarded = |ext: &str| scan.guards.extensions.contains(&ext.to_ascii_lowercase());
        for (name, &count) in &scan.functions {
            match manifest.function(name) {
                Some(sym) => {
                    let guarded = scan.guards.functions.contains(name) || ext_guarded(&sym.extension);
                    self.record(Kind::Function, sym, label, count, guarded);
                }
                None => {
                    self.user_functions.insert(name.clone());
                }
            }
        }
        for (name, &count) in &scan.classes {
            match manifest.class(name) {
                Some(sym) => {
                    let guarded = scan.guards.classes.contains(name) || ext_guarded(&sym.extension);
                    self.record(Kind::Class, sym, label, count, guarded);
                }
                None => {
                    self.user_classes.insert(name.clone());
                }
            }
        }
        for (name, &count) in &scan.constants {
            if let Some(sym) = manifest.constant(name) {
                let guarded = scan.guards.constants.contains(name) || ext_guarded(&sym.extension);
                self.record(Kind::Constant, sym, label, count, guarded);
            }
        }
        for name in &scan.defines.functions {
            if let Some(sym) = manifest.function(name) {
                self.polyfilled.insert((Kind::Function, sym.name.clone()));
            }
        }
        for name in &scan.defines.classes {
            if let Some(sym) = manifest.class(name) {
                self.polyfilled.insert((Kind::Class, sym.name.clone()));
            }
        }
        for name in &scan.defines.constants {
            if let Some(sym) = manifest.constant(name) {
                self.polyfilled.insert((Kind::Constant, sym.name.clone()));
            }
        }
    }

    fn record(&mut self, kind: Kind, sym: &ManifestSymbol, label: &str, count: u32, guarded: bool) {
        let entry = self
            .uses
            .entry((kind, sym.name.clone()))
            .or_insert_with(|| SymbolUse {
                symbol: sym.name.clone(),
                kind,
                ext: sym.extension.clone(),
                ..SymbolUse::default()
            });
        entry.call_sites += count;
        *entry.by_file.entry(label.to_string()).or_insert(0) += count;
        if guarded {
            entry.guarded_files += 1;
        }
    }

    fn finish(self, registry: &Registry) -> (Vec<SymbolUse>, usize, usize) {
        let polyfilled = self.polyfilled;
        let mut symbols: Vec<SymbolUse> = self
            .uses
            .into_iter()
            .map(|((kind, name), mut u)| {
                u.files = u.by_file.len();
                u.optional = u.files > 0 && u.guarded_files == u.files;
                u.polyfilled = polyfilled.contains(&(kind, name.clone()));
                u.implemented = registry.implements(kind, &name);
                u
            })
            .collect();
        symbols.sort_by(|a, b| {
            a.implemented
                .cmp(&b.implemented)
                .then(b.call_sites.cmp(&a.call_sites))
                .then(b.files.cmp(&a.files))
                .then(a.kind.cmp(&b.kind))
                .then(a.symbol.cmp(&b.symbol))
        });
        (symbols, self.user_functions.len(), self.user_classes.len())
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Format {
    Text,
    Json,
    Md,
}

struct Opts {
    dirs: Vec<PathBuf>,
    include_tests: bool,
    manifest: PathBuf,
    registry: Option<PathBuf>,
    top: usize,
    ext: Option<String>,
    format: Format,
    by_file: bool,
    optional: bool,
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// A path as given on the command line, falling back to the repository root
/// when it does not exist relative to the current directory.
fn resolve_repo_path(p: &Path) -> PathBuf {
    if p.exists() || p.is_absolute() {
        p.to_path_buf()
    } else {
        let alt = repo_root().join(p);
        if alt.exists() {
            alt
        } else {
            p.to_path_buf()
        }
    }
}

fn parse_args(args: &[String]) -> Result<Option<Opts>, Box<dyn std::error::Error>> {
    let mut a = pico_args::Arguments::from_vec(args.iter().map(OsString::from).collect());
    if a.contains(["-h", "--help"]) {
        return Ok(None);
    }
    let format = match a.opt_value_from_str::<_, String>("--report")? {
        None => Format::Text,
        Some(s) if s == "text" => Format::Text,
        Some(s) if s == "json" => Format::Json,
        Some(s) if s == "md" => Format::Md,
        Some(other) => return Err(format!("--report must be text, json or md, got `{other}`").into()),
    };
    let dirs: Vec<PathBuf> = a.values_from_str("--dir")?;
    let opts = Opts {
        include_tests: a.contains("--include-tests"),
        manifest: a
            .opt_value_from_str::<_, PathBuf>("--manifest")?
            .unwrap_or_else(|| PathBuf::from("manifest/php-8.5.0")),
        registry: a.opt_value_from_str("--registry")?,
        top: a.opt_value_from_str("--top")?.unwrap_or(60),
        ext: a.opt_value_from_str("--ext")?,
        format,
        by_file: a.contains("--by-file"),
        optional: a.contains("--optional"),
        dirs,
    };
    let rest = a.finish();
    if !rest.is_empty() {
        return Err(format!("unexpected argument(s): {}", rest.iter().map(|s| s.to_string_lossy()).collect::<Vec<_>>().join(" ")).into());
    }
    if opts.dirs.is_empty() {
        return Err("at least one --dir <path> is required".into());
    }
    Ok(Some(opts))
}

/// Rows that the text/md tables show: missing, extension filter, optional filter.
fn visible_rows<'a>(report: &'a Report, opts: &Opts) -> Vec<&'a SymbolUse> {
    report
        .symbols
        .iter()
        .filter(|s| !s.implemented)
        .filter(|s| opts.optional || !s.optional)
        .filter(|s| {
            opts.ext
                .as_ref()
                .is_none_or(|e| e.eq_ignore_ascii_case(&s.ext))
        })
        .collect()
}

fn summary_line(report: &Report) -> String {
    let part = |kind: Kind| {
        let s = report.summary.get(kind.plural()).copied().unwrap_or_default();
        let mut text = format!(
            "internal {} used: {}, implemented: {}, missing: {} (optional: {}",
            kind.plural(),
            s.used,
            s.implemented,
            s.missing,
            s.optional
        );
        if s.polyfilled > 0 {
            let _ = write!(text, ", polyfilled: {}", s.polyfilled);
        }
        text.push(')');
        text
    };
    format!(
        "{}; {}; {}",
        part(Kind::Function),
        part(Kind::Class),
        part(Kind::Constant)
    )
}

fn tags(s: &SymbolUse) -> String {
    let mut t = Vec::new();
    if s.optional {
        t.push("optional");
    }
    if s.polyfilled {
        t.push("polyfill");
    }
    t.join(", ")
}

fn ext_order(rows: &[&SymbolUse]) -> Vec<String> {
    let mut totals: BTreeMap<&str, u32> = BTreeMap::new();
    for r in rows {
        *totals.entry(r.ext.as_str()).or_insert(0) += r.call_sites;
    }
    let mut exts: Vec<(&str, u32)> = totals.into_iter().collect();
    exts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    exts.into_iter().map(|(e, _)| e.to_string()).collect()
}

fn by_file_lines(s: &SymbolUse, indent: &str, out: &mut String) {
    let mut files: Vec<(&String, &u32)> = s.by_file.iter().collect();
    files.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    for (path, n) in files {
        let _ = writeln!(out, "{indent}{path} ({n})");
    }
}

fn render_text(report: &Report, opts: &Opts) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "cargo xtask missing — {} files scanned ({} skipped as tests/skeleton{}), {} root(s), {:.2}s",
        report.files_scanned,
        report.files_skipped,
        if report.files_unreadable > 0 {
            format!(", {} unreadable", report.files_unreadable)
        } else {
            String::new()
        },
        report.dirs.len(),
        report.seconds
    );
    let _ = writeln!(out, "manifest: {}", report.manifest);
    let _ = writeln!(out, "registry: {}", report.registry);
    let _ = writeln!(
        out,
        "heuristic sanity: {} distinct non-internal function names, {} non-internal class names also seen",
        report.user_functions, report.user_classes
    );
    let rows = visible_rows(report, opts);
    let shown: Vec<&SymbolUse> = if opts.top == 0 {
        rows.clone()
    } else {
        rows.iter().copied().take(opts.top).collect()
    };
    let _ = writeln!(out);
    if rows.is_empty() {
        let _ = writeln!(out, "nothing missing{}.", if opts.optional { "" } else { " (use --optional to include guarded symbols)" });
    } else {
        let _ = writeln!(
            out,
            "missing symbols ranked by call sites — showing {} of {}{}{}:",
            shown.len(),
            rows.len(),
            if opts.optional { "" } else { " (optional ones hidden; --optional shows them)" },
            opts.ext.as_ref().map(|e| format!(", extension {e}")).unwrap_or_default()
        );
        let width = shown.iter().map(|s| s.symbol.len()).max().unwrap_or(6).max(6);
        for ext in ext_order(&shown) {
            let all_ext: Vec<&&SymbolUse> = rows.iter().filter(|s| s.ext == ext).collect();
            let ext_sites: u32 = all_ext.iter().map(|s| s.call_sites).sum();
            let ext_rows: Vec<&&SymbolUse> = shown.iter().filter(|s| s.ext == ext).collect();
            let _ = writeln!(
                out,
                "\n== {ext}: {} missing symbol(s), {} call sites; showing {} ==",
                all_ext.len(),
                ext_sites,
                ext_rows.len()
            );
            let _ = writeln!(out, "{:<width$}  {:<8}  {:<12}  {:>10}  {:>5}  tags", "symbol", "kind", "ext", "call-sites", "files");
            for s in ext_rows {
                let _ = writeln!(
                    out,
                    "{:<width$}  {:<8}  {:<12}  {:>10}  {:>5}  {}",
                    s.symbol,
                    s.kind.label(),
                    s.ext,
                    s.call_sites,
                    s.files,
                    tags(s)
                );
                if opts.by_file {
                    by_file_lines(s, "      ", &mut out);
                }
            }
        }
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "{}", summary_line(report));
    out
}

fn render_md(report: &Report, opts: &Opts) -> String {
    let mut out = String::new();
    let rows = visible_rows(report, opts);
    let shown: Vec<&SymbolUse> = if opts.top == 0 {
        rows.clone()
    } else {
        rows.iter().copied().take(opts.top).collect()
    };
    let _ = writeln!(
        out,
        "<!-- generated by `cargo xtask missing{}` on {} files -->",
        opts.dirs.iter().map(|d| format!(" --dir {}", d.display())).collect::<String>(),
        report.files_scanned
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "{}", summary_line(report));
    let _ = writeln!(out);
    let _ = writeln!(out, "| Extension | Missing symbols | Optional | Call sites |");
    let _ = writeln!(out, "|---|---:|---:|---:|");
    let mut exts: Vec<(&String, &ExtTotals)> = report.by_extension.iter().collect();
    exts.sort_by(|a, b| b.1.call_sites.cmp(&a.1.call_sites).then(a.0.cmp(b.0)));
    for (ext, t) in exts {
        if opts.ext.as_ref().is_some_and(|e| !e.eq_ignore_ascii_case(ext)) {
            continue;
        }
        let _ = writeln!(out, "| {ext} | {} | {} | {} |", t.missing, t.optional, t.call_sites);
    }
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "Top {} missing symbols by call sites{}:",
        shown.len(),
        if opts.optional { " (guarded/optional included)" } else { " (guarded/optional hidden)" }
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "| Symbol | Kind | Extension | Call sites | Files | Notes |");
    let _ = writeln!(out, "|---|---|---|---:|---:|---|");
    for s in &shown {
        let mut notes = tags(s);
        if opts.by_file {
            let mut files: Vec<(&String, &u32)> = s.by_file.iter().collect();
            files.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
            let list = files
                .iter()
                .take(5)
                .map(|(p, n)| format!("`{p}` ({n})"))
                .collect::<Vec<_>>()
                .join(", ");
            if !notes.is_empty() {
                notes.push_str("; ");
            }
            notes.push_str(&list);
            if files.len() > 5 {
                let _ = write!(notes, ", … {} more", files.len() - 5);
            }
        }
        let _ = writeln!(
            out,
            "| `{}` | {} | {} | {} | {} | {} |",
            s.symbol,
            s.kind.label(),
            s.ext,
            s.call_sites,
            s.files,
            notes
        );
    }
    out
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Build the report for `opts` (everything but printing).
fn build_report(opts: &Opts) -> Result<Report, Box<dyn std::error::Error>> {
    let started = Instant::now();
    let manifest_dir = resolve_repo_path(&opts.manifest);
    let manifest = Manifest::load(&manifest_dir)?;
    let registry = match &opts.registry {
        Some(p) => Registry::from_json(p)?,
        None => Registry::live(&manifest),
    };
    let (files, skipped) = discover(&opts.dirs, opts.include_tests)?;

    let is_constant = |name: &str| manifest.is_constant(name);
    let scans: Vec<(String, Option<FileScan>)> = files
        .par_iter()
        .map(|c| {
            let scan = fs::read(&c.path)
                .ok()
                .map(|src| scan_source(&src, &is_constant));
            (c.label.clone(), scan)
        })
        .collect();

    let mut agg = Aggregate::new();
    let mut unreadable = 0usize;
    for (label, scan) in &scans {
        match scan {
            Some(s) => agg.add(&manifest, label, s),
            None => unreadable += 1,
        }
    }
    let (symbols, user_functions, user_classes) = agg.finish(&registry);

    let mut summary: BTreeMap<String, KindSummary> = BTreeMap::new();
    for kind in [Kind::Function, Kind::Class, Kind::Constant] {
        summary.insert(kind.plural().to_string(), KindSummary::default());
    }
    let mut by_extension: BTreeMap<String, ExtTotals> = BTreeMap::new();
    for s in &symbols {
        let ks = summary.get_mut(s.kind.plural()).expect("kind present");
        ks.used += 1;
        if s.implemented {
            ks.implemented += 1;
        } else {
            ks.missing += 1;
            if s.optional {
                ks.optional += 1;
            }
            if s.polyfilled {
                ks.polyfilled += 1;
            }
            let e = by_extension.entry(s.ext.clone()).or_default();
            e.missing += 1;
            e.call_sites += s.call_sites;
            if s.optional {
                e.optional += 1;
            }
        }
    }
    let (nf, nc, nk) = manifest.counts();
    Ok(Report {
        dirs: opts.dirs.iter().map(|d| d.display().to_string()).collect(),
        files_scanned: files.len() - unreadable,
        files_skipped: skipped,
        files_unreadable: unreadable,
        manifest: format!("{} ({nf} functions, {nc} classes, {nk} constants)", manifest.dir.display()),
        registry: registry.label.clone(),
        registry_lacks_classes_constants: registry.classes_constants_unavailable,
        user_functions,
        user_classes,
        summary,
        by_extension,
        symbols,
        seconds: started.elapsed().as_secs_f64(),
    })
}

/// `cargo xtask missing …` entry point.
pub fn run(args: &[String]) -> XtaskResult {
    let Some(opts) = parse_args(args)? else {
        print!("{HELP}");
        return Ok(());
    };
    let report = build_report(&opts)?;

    let target = repo_root().join("target");
    fs::create_dir_all(&target)?;
    let json_path = target.join("missing-report.json");
    let json = serde_json::to_string_pretty(&report)?;
    fs::write(&json_path, &json)?;

    match opts.format {
        Format::Json => println!("{json}"),
        Format::Text => {
            print!("{}", render_text(&report, &opts));
            eprintln!("(json copy: {})", json_path.display());
        }
        Format::Md => print!("{}", render_md(&report, &opts)),
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> Manifest {
        Manifest::load(&repo_root().join("manifest/php-8.5.0")).expect("manifest loads")
    }

    fn fixture(name: &str) -> Vec<u8> {
        let p = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/missing")
            .join(name);
        fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
    }

    fn counts(pairs: &[(&str, u32)]) -> BTreeMap<String, u32> {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    fn set(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn manifest_loads_and_tags_extensions() {
        let m = manifest();
        let (f, c, k) = m.counts();
        assert!(f > 1900 && c > 300 && k > 2800, "{f} {c} {k}");
        assert_eq!(m.function("strlen").unwrap().extension, "Core");
        assert_eq!(m.function("STRLEN").unwrap().name, "strlen");
        assert_eq!(m.function("preg_match").unwrap().extension, "pcre");
        assert_eq!(m.class("arrayobject").unwrap().name, "ArrayObject");
        assert_eq!(m.class("ArrayObject").unwrap().extension, "SPL");
        assert_eq!(m.class("dom\\document").unwrap().extension, "dom");
        assert_eq!(m.constant("PHP_EOL").unwrap().extension, "Core");
        assert_eq!(m.constant("JSON_THROW_ON_ERROR").unwrap().extension, "json");
        assert!(m.constant("php_eol").is_none(), "constants are case-sensitive");
    }

    #[test]
    fn live_registry_probe_knows_strlen() {
        let m = manifest();
        let r = Registry::live(&m);
        assert!(r.implements(Kind::Function, "strlen"));
        assert!(!r.implements(Kind::Function, "no_such_function_xyz"));
        assert!(r.classes_constants_unavailable);
    }

    #[test]
    fn calls_fixture_extracts_exact_sets() {
        let m = manifest();
        let scan = scan_source(&fixture("calls.php"), &|n| m.is_constant(n));
        assert_eq!(
            scan.functions,
            counts(&[
                ("strlen", 3),      // strlen($s) + \strlen($s) + strlen(...); $this->strlen / Helper::strlen excluded
                ("mb_strlen", 1),
                ("json_encode", 1), // \json_encode
                ("preg_match", 1),
                ("dom\\import_simplexml", 1), // via `use function Dom\import_simplexml as sx`
                ("is_callable", 1), // strlen(...) first-class callable is still a call site
            ])
        );
        assert_eq!(
            scan.classes,
            counts(&[
                ("arrayaccess", 2),        // import + implements
                ("countable", 3),          // import + int|Countable ...$rest + Countable&\Traversable
                ("iteratoraggregate", 1),  // import (aliased IA)
                ("attribute", 2),          // #[\Attribute(\Attribute::TARGET_CLASS)]
                ("arrayobject", 1),        // extends
                ("jsonserializable", 1),   // implements
                ("fixture\\sometrait", 1), // trait use, resolved in the namespace
                ("closure", 1),            // ?\Closure property type
                ("datetimeimmutable", 1),  // promoted ctor param union
                ("datetime", 1),
                ("stdclass", 2),           // array|\stdClass return type + new \stdClass
                ("valueerror", 1),         // catch
                ("fixture\\runtimeexception", 1), // unimported => current namespace, no global fallback
                ("logicexception", 1),     // new \LogicException
                ("stringable", 1),         // instanceof
                ("fixture\\helper", 1),    // Helper::strlen
                ("splobjectstorage", 1),   // \SplObjectStorage $s
                ("traversable", 2),        // ?\Traversable return + Countable&\Traversable
                ("dom\\document", 2),      // import + new Document
                ("sensitiveparameter", 1), // #[\SensitiveParameter] on a param
            ])
        );
        assert_eq!(
            scan.constants,
            counts(&[
                ("JSON_THROW_ON_ERROR", 1),
                ("JSON_PRETTY_PRINT", 1),
                ("PHP_EOL", 1),
                ("DIRECTORY_SEPARATOR", 1), // \DIRECTORY_SEPARATOR
                ("E_ALL", 2),               // E_ALL . case E_ALL:
                ("PHP_INT_MAX", 1),         // ternary branch
            ])
        );
        assert_eq!(scan.guards, Guards::default());
        // Namespaced declarations are not polyfills.
        assert_eq!(scan.defines, Defines::default());
    }

    #[test]
    fn guards_fixture_extracts_guards_and_polyfills() {
        let m = manifest();
        let scan = scan_source(&fixture("guards.php"), &|n| m.is_constant(n));
        assert_eq!(
            scan.guards,
            Guards {
                functions: set(&["mb_strlen", "iconv_strlen", "opcache_invalidate"]),
                classes: set(&["arrayobject", "stringable"]),
                constants: set(&["PHP_WINDOWS_VERSION_MAJOR"]),
                extensions: set(&["mbstring"]),
            }
        );
        assert_eq!(
            scan.defines,
            Defines {
                functions: set(&["mb_strlen"]),
                classes: set(&["stringable"]),
                constants: set(&["SOME_FLAG", "E_STRICT"]),
            }
        );
        assert_eq!(
            scan.functions,
            counts(&[
                ("function_exists", 3),
                ("extension_loaded", 1),
                ("iconv_strlen", 1),
                ("class_exists", 1),
                ("interface_exists", 1),
                ("defined", 1),
                ("constant", 2),
                ("define", 2),
                ("opcache_invalidate", 1),
                ("strlen", 1),
            ])
        );
        assert_eq!(scan.classes, counts(&[("stringable", 1)]));
        // defined()/constant() string arguments count as constant uses (when
        // the manifest knows them); PHP_WINDOWS_VERSION_MAJOR is not in a macOS
        // manifest and SOME_FLAG is user-defined.
        assert_eq!(scan.constants, counts(&[("PHP_VERSION", 1), ("E_ALL", 1)]));
    }

    #[test]
    fn guards_make_symbols_optional_and_polyfills_are_tagged() {
        let m = manifest();
        let mut agg = Aggregate::new();
        let is_constant = |n: &str| m.is_constant(n);
        agg.add(&m, "fx/calls.php", &scan_source(&fixture("calls.php"), &is_constant));
        agg.add(&m, "fx/guards.php", &scan_source(&fixture("guards.php"), &is_constant));
        let empty = Registry::default();
        let (symbols, _, _) = agg.finish(&empty);
        let find = |kind: Kind, name: &str| {
            symbols
                .iter()
                .find(|s| s.kind == kind && s.symbol == name)
                .unwrap_or_else(|| panic!("{name} missing"))
        };
        // mb_strlen: called unguarded in calls.php, so not optional; but polyfilled.
        let mb = find(Kind::Function, "mb_strlen");
        assert!(!mb.optional && mb.polyfilled && mb.files == 1 && mb.call_sites == 1);
        // opcache_invalidate: only used behind function_exists.
        assert!(find(Kind::Function, "opcache_invalidate").optional);
        // iconv_strlen: guarded by function_exists + extension_loaded('mbstring') in its only file.
        assert!(find(Kind::Function, "iconv_strlen").optional);
        // Stringable: referenced in both files, guarded in only one => blocking; polyfilled.
        let st = find(Kind::Class, "Stringable");
        assert!(!st.optional && st.polyfilled && st.files == 2);
        assert_eq!(find(Kind::Function, "strlen").call_sites, 4);
        assert_eq!(find(Kind::Constant, "E_ALL").ext, "Core");
        assert_eq!(find(Kind::Function, "preg_match").ext, "pcre");
        assert_eq!(find(Kind::Class, "ArrayObject").ext, "SPL");
        assert_eq!(find(Kind::Class, "Dom\\Document").ext, "dom");
    }

    #[test]
    fn skip_rules() {
        assert!(is_skipped_path(Path::new("symfony/console/Tests/Foo.php")));
        assert!(is_skipped_path(Path::new("pkg/tests/Foo.php")));
        assert!(is_skipped_path(Path::new("bundle/Resources/skeleton/Foo.php")));
        assert!(!is_skipped_path(Path::new("symfony/console/Foo.php")));
        assert!(!is_skipped_path(Path::new("Tests.php")), "a file named Tests.php is not a directory");
        assert!(!is_skipped_path(Path::new("Resources/config/skeleton.php")));
        assert!(!is_skipped_path(Path::new("skeleton/Foo.php")));
    }

    #[test]
    fn end_to_end_on_fixture_dir() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/missing");
        let opts = Opts {
            dirs: vec![dir],
            include_tests: false,
            manifest: PathBuf::from("manifest/php-8.5.0"),
            registry: None,
            top: 0,
            ext: None,
            format: Format::Text,
            by_file: true,
            optional: true,
        };
        let report = build_report(&opts).expect("report");
        assert_eq!(report.files_scanned, 2, "Tests/ and Resources/skeleton/ are skipped");
        assert_eq!(report.files_skipped, 2);
        let f = report.summary["functions"];
        assert!(f.used >= 15 && f.implemented >= 1, "{f:?}");
        assert!(report.symbols.iter().any(|s| s.symbol == "strlen" && s.implemented));
        let text = render_text(&report, &opts);
        assert!(text.contains("internal functions used:"));
        assert!(text.contains("== SPL:") || text.contains("== Core:"));
        let md = render_md(&report, &opts);
        assert!(md.contains("| Symbol | Kind | Extension |"));
        // The skipped test file's exclusive symbol never shows up.
        assert!(!report.symbols.iter().any(|s| s.symbol == "array_walk_recursive"));
    }

    #[test]
    fn use_statements_resolve_imports_groups_and_aliases() {
        let src = b"<?php
namespace A;
use Foo\\{Bar, Baz as Qux, function f, const C};
use Dom\\{Document, Element as El};
use function Dom\\import_simplexml;
class X { use \\Some\\T; }
$a = new Bar(); $b = new Qux(); $c = new El(); $d = new Document(); f(); import_simplexml($d);
function g(Unknown $u): Bar|Document {}
";
        let scan = scan_source(src, &|_| false);
        assert_eq!(
            scan.classes,
            counts(&[
                ("foo\\bar", 3),      // import + new + return type
                ("foo\\baz", 2),      // import (aliased Qux) + new Qux
                ("dom\\document", 3), // import + new + return type
                ("dom\\element", 2),  // import (aliased El) + new El
                ("some\\t", 1),       // trait use inside the class body
                ("a\\unknown", 1),    // unimported => current namespace
            ])
        );
        assert_eq!(
            scan.functions,
            counts(&[("foo\\f", 1), ("dom\\import_simplexml", 1)])
        );
        assert!(scan.constants.is_empty());
        // A braced global namespace block and a later namespace reset imports.
        let src2 = b"<?php
namespace { use ArrayObject; $x = new ArrayObject(); function polyfill() {} }
namespace B { $y = new ArrayObject(); }
";
        let scan2 = scan_source(src2, &|_| false);
        assert_eq!(scan2.classes, counts(&[("arrayobject", 2), ("b\\arrayobject", 1)]));
        assert_eq!(scan2.defines.functions, set(&["polyfill"]));
    }

    #[test]
    fn string_literal_names() {
        assert_eq!(string_literal_name("'strlen'").as_deref(), Some("strlen"));
        assert_eq!(string_literal_name("\"\\\\Foo\\\\Bar\"").as_deref(), Some("Foo\\Bar"));
        assert_eq!(string_literal_name("'\\Foo\\Bar'").as_deref(), Some("Foo\\Bar"));
        assert_eq!(string_literal_name("b'x'").as_deref(), Some("x"));
        assert_eq!(string_literal_name("''"), None);
        assert_eq!(string_literal_name("'a b'"), None);
        assert_eq!(string_literal_name("\"$x\""), None);
    }
}
