//! `Transliterator` and the `transliterator_*` functions over ICU4X's
//! rule-based transliterator (CLDR's transforms, the same rules ICU 78
//! compiles): php's IDs are parsed the way ICU parses them (`id.rs`),
//! each piece is resolved through ICU's registry — the CLDR IDs and
//! aliases dumped from php (`table.rs`), the script/locale fallbacks of
//! `TransliteratorSpec`, the code-based transliterators (`custom.rs`) —
//! and the whole chain is compiled into one ICU4X transliterator with
//! the filters as written. `createFromRules()` hands the rules to ICU4X's
//! rule compiler.
//!
//! Differences from ICU catalogued in COVERAGE.md: a script run inside
//! `Any-<script>` sees only the text before it as context, `Any-FCD` is
//! the identity and `Any-FCC` is NFC, a few CLDR transforms ICU4X bakes
//! from other data (`Latin-Hebrew`, `ug-ug_FONIPA`), and a rule-syntax
//! error's code and offset are reconstructed rule by rule (ICU4X's
//! compiler reports neither).

mod custom;
mod id;
mod table;

use std::rc::Rc;

use icu::locale::{Locale, LocaleExpander};
use icu::properties::props::Script;
use icu::properties::{PropertyNamesLong, PropertyParser};
use icu_experimental::transliterate::provider::TransliteratorRulesV1;
use icu_experimental::transliterate::{CustomTransliterator, RuleCollection, Transliterator as IcuTransliterator};
use icu_provider::prelude::*;
use rphp_runtime::{value_name, Ctx, Interp, NativeResult, Registry, Unwind};
use rphp_value::{Array, Object, Payload, Value};

use crate::shape::{register_class, register_functions, FnImpl, MethodImpl};
use crate::state::{self, IntlError, U_ILLEGAL_ARGUMENT_ERROR, U_INVALID_CHAR_FOUND, U_INVALID_ID};
use crate::{generated, str_arg};

/// `U_MALFORMED_RULE`.
const U_MALFORMED_RULE: i64 = 65537;

/// Where a rule-based transliterator's rules live.
#[derive(Clone, Copy, Debug)]
pub enum Rbt {
    /// ICU4X's baked data, by marker (`und-latn-t-und-cyrl`).
    Baked(&'static str),
    /// A CLDR rules file compiled at runtime (ICU4X ships no data for
    /// it), and whether it runs backward.
    Source(&'static str, bool),
}

/// The CLDR rules sources ICU4X has no baked data for.
fn rules_source(file: &str) -> Option<&'static str> {
    Some(match file {
        "Han-Latin.txt" => include_str!("rules/Han-Latin.txt"),
        "Pinyin-NumericPinyin.txt" => include_str!("rules/Pinyin-NumericPinyin.txt"),
        "Thai-Latin.txt" => include_str!("rules/Thai-Latin.txt"),
        "el-Title.txt" => include_str!("rules/el-Title.txt"),
        "nl-Title.txt" => include_str!("rules/nl-Title.txt"),
        _ => return None,
    })
}

/// What a basic ID resolves to.
#[derive(Clone, Debug)]
enum Step {
    /// One of ICU4X's built-ins, spelled as its rules spell it.
    Native(&'static str),
    Rbt(Rbt),
    Custom(Kind),
}

/// The code-based transliterators.
#[derive(Clone, Debug)]
enum Kind {
    Title,
    HexTo(custom::HexForm),
    HexFrom(&'static [custom::Unescape]),
    NameTo,
    NameFrom,
    Identity,
    Any(String, Option<Script>),
}

impl Kind {
    fn instantiate(&self) -> Box<dyn CustomTransliterator> {
        match self {
            Kind::Title => Box::new(custom::Title),
            Kind::HexTo(f) => Box::new(custom::HexTo(*f)),
            Kind::HexFrom(f) => Box::new(custom::HexFrom(f)),
            Kind::NameTo => Box::new(custom::NameTo),
            Kind::NameFrom => Box::new(custom::NameFrom),
            Kind::Identity => Box::new(custom::Identity),
            Kind::Any(t, s) => Box::new(custom::AnyScript::new(t.clone(), *s)),
        }
    }
}

/// A compiled transliterator.
#[derive(Debug)]
pub struct Compiled(IcuTransliterator);

impl Compiled {
    pub fn run(&self, s: String) -> String {
        self.0.transliterate(s)
    }
}

// ---- the registry --------------------------------------------------------------------

/// The targets ICU's `AnyTransliterator` registers (`Any-Latin`, `Any-ru`,
/// …): every `Any-X` ID php lists that is not one of the code-based ones.
pub(crate) fn any_targets() -> Vec<&'static str> {
    const NOT_SCRIPTS: &[&str] = &[
        "null", "remove", "lower", "upper", "title", "nfc", "nfd", "nfkc", "nfkd", "fcc", "fcd", "hex", "name", "accents",
        "publishing", "any",
    ];
    table::LIST_IDS
        .iter()
        .filter_map(|id| {
            let (src, rest) = id.split_once('-')?;
            if !src.eq_ignore_ascii_case("any") {
                return None;
            }
            let target = rest.split('/').next().unwrap_or(rest);
            (!NOT_SCRIPTS.contains(&target.to_ascii_lowercase().as_str())).then_some(target)
        })
        .collect()
}

/// `uscript_getCode()`'s first code for a script name, script code or
/// locale (`Cyrillic`, `Cyrl`, `ru`, `uz_Cyrl`, `ja` → Katakana).
pub(crate) fn script_code(spec: &str) -> Option<Script> {
    let parser = PropertyParser::<Script>::new();
    let by_name = |s: &str| -> Option<Script> {
        match s.to_ascii_lowercase().as_str() {
            "hans" | "hant" | "hani" => Some(Script::Han),
            "jpan" => Some(Script::Katakana),
            "kore" => Some(Script::Hangul),
            _ => parser.get_loose(s),
        }
    };
    let plain = !spec.contains(['-', '_']);
    if plain {
        if let Some(s) = by_name(spec) {
            return Some(s);
        }
    }
    let tag = spec.replace('_', "-");
    let mut parts = tag.split('-');
    let lang = parts.next().unwrap_or("").to_ascii_lowercase();
    match lang.as_str() {
        "ja" => return Some(Script::Katakana),
        "ko" => return Some(Script::Hangul),
        _ => {}
    }
    let explicit = parts.next().filter(|p| p.len() == 4 && p.chars().all(|c| c.is_ascii_alphabetic()));
    if let Some(s) = explicit.and_then(by_name) {
        return Some(s);
    }
    let mut langid: icu::locale::LanguageIdentifier = tag.parse().ok()?;
    if langid.language.is_unknown() && langid.script.is_none() && langid.region.is_none() && !tag.starts_with("und-") {
        return None;
    }
    langid.variants.clear();
    LocaleExpander::new_extended().maximize(&mut langid);
    // `und_FONIPA` and friends: ICU's likely subtags make `und` Latin.
    let script = langid.script.or_else(|| langid.language.is_unknown().then(|| "Latn".parse().ok()).flatten())?;
    by_name(script.as_str())
}

/// `TransliteratorSpec`'s top: a spec naming a script (or a locale with a
/// script) becomes the script's long name.
fn spec_top(spec: &str) -> String {
    match script_code(spec).and_then(|s| PropertyNamesLong::<Script>::new().get(s)) {
        Some(name) => name.to_string(),
        None => spec.to_string(),
    }
}

fn stv(source: &str, target: &str, variant: &str) -> String {
    let mut k = format!("{source}-{target}");
    if !variant.is_empty() {
        k.push('/');
        k.push_str(variant);
    }
    k.to_ascii_lowercase()
}

fn registry_lookup(key: &str) -> Option<Rbt> {
    table::REGISTRY.binary_search_by(|(k, _)| (*k).cmp(key)).ok().map(|i| table::REGISTRY[i].1)
}

/// The code-based transliterators ICU registers by hand.
fn special(source: &str, target: &str, variant: &str) -> Option<Step> {
    let (s, t, v) = (source.to_ascii_lowercase(), target.to_ascii_lowercase(), variant.to_ascii_lowercase());
    if s == "any" {
        let native = |n| v.is_empty().then_some(Step::Native(n));
        return match t.as_str() {
            "null" => native("Any-Null"),
            "remove" => native("Any-Remove"),
            "lower" => native("Any-Lower"),
            "upper" => native("Any-Upper"),
            "nfc" | "fcc" => native("Any-NFC"),
            "nfd" => native("Any-NFD"),
            "nfkc" => native("Any-NFKC"),
            "nfkd" => native("Any-NFKD"),
            "fcd" | "breakinternal" if v.is_empty() => Some(Step::Custom(Kind::Identity)),
            "title" if v.is_empty() => Some(Step::Custom(Kind::Title)),
            "hex" => custom::hex_form(&v).map(|f| Step::Custom(Kind::HexTo(f))),
            "name" if v.is_empty() => Some(Step::Custom(Kind::NameTo)),
            _ => None,
        };
    }
    if t == "any" {
        return match s.as_str() {
            "hex" => custom::unescape_forms(&v).map(|f| Step::Custom(Kind::HexFrom(f))),
            "name" if v.is_empty() => Some(Step::Custom(Kind::NameFrom)),
            _ => None,
        };
    }
    None
}

/// One registry probe: the code-based ones, the rule-based ones, the
/// `AnyTransliterator` instances.
fn probe(source: &str, target: &str, variant: &str) -> Option<Step> {
    if let Some(s) = special(source, target, variant) {
        return Some(s);
    }
    let key = stv(source, target, variant);
    if let Some(r) = registry_lookup(&key) {
        return Some(Step::Rbt(r));
    }
    if source.eq_ignore_ascii_case("any") && table::LIST_IDS.iter().any(|id| id.eq_ignore_ascii_case(&key)) {
        let mut t = target.to_string();
        if !variant.is_empty() {
            t.push('/');
            t.push_str(variant);
        }
        return Some(Step::Custom(Kind::Any(t, script_code(target))));
    }
    None
}

/// `TransliteratorRegistry::find`: the ID as given, then its specs'
/// canonical forms, then (for rule data) without the variant.
fn resolve(basic: &str) -> Option<Step> {
    let (source, target, variant) = id::split_basic(basic);
    if let Some(s) = probe(&source, &target, &variant) {
        return Some(s);
    }
    let (src, trg) = (spec_top(&source), spec_top(&target));
    // ICU's translit bundle for Greek names UNGEGN the variant.
    if [&source, &target].iter().any(|s| s.eq_ignore_ascii_case("el")) {
        if let Some(s) = probe(&src, &trg, "UNGEGN") {
            return Some(s);
        }
    }
    // A spec ICU's translit tree has a bundle for stays a locale, so the
    // variant search misses and the script fallback runs without it
    // (measured: `Grek-sr_Latn/BGN` is plain Greek-Latin, `Grek-ru_Latn/BGN`
    // Greek-Latin/BGN).
    const BUNDLES: &[&str] = &["byn_latn", "en", "hi_latn", "kok_latn", "ky_latn", "shi_latn", "sr_latn", "vai_latn"];
    let is_locale = |spec: &str, _: &str| BUNDLES.contains(&spec.to_ascii_lowercase().replace('-', "_").as_str());
    let keep_variant = !is_locale(&source, &src) && !is_locale(&target, &trg);
    if let Some(s) = probe(&src, &trg, if keep_variant { &variant } else { "" }) {
        return Some(s);
    }
    // A target of `Any` takes the source's first target, Latin.
    if trg.eq_ignore_ascii_case("any") && !src.eq_ignore_ascii_case("any") {
        if let Some(s) = probe(&src, "Latin", &variant) {
            return Some(s);
        }
    }
    if !variant.is_empty() {
        return registry_lookup(&stv(&src, &trg, "")).map(Step::Rbt);
    }
    None
}

// ---- compiling -----------------------------------------------------------------------

/// The rule collection first, then ICU4X's baked data (internal
/// dependencies, `x-thai-thaisemi`, keep ICU4X's eight-letter subtags).
struct Provider<'a, P: ?Sized>(&'a P);

impl<P: DataProvider<TransliteratorRulesV1> + ?Sized> DataProvider<TransliteratorRulesV1> for Provider<'_, P> {
    fn load(&self, req: DataRequest) -> Result<DataResponse<TransliteratorRulesV1>, DataError> {
        match self.0.load(req) {
            Err(e) if e.kind == DataErrorKind::IdentifierNotFound || e.kind == DataErrorKind::MarkerNotFound => {
                let attrs = req.id.marker_attributes.as_str();
                let key = match attrs.strip_prefix("x-") {
                    Some(rest) => {
                        let parts: Vec<String> = rest.split('-').map(|p| p.chars().take(8).collect()).collect();
                        format!("und-x-{}", parts.join("-"))
                    }
                    None => attrs.to_string(),
                };
                // The baked Han data reaches for its own `Han-Spacedhan`;
                // ours is the one ICU 78 runs.
                let spaced = key == "und-x-han-spacedha";
                let key = if spaced { "und-x-spcdhan" } else { key.as_str() };
                let attrs = DataMarkerAttributes::try_from_str(key).map_err(|_| e)?;
                let req = DataRequest { id: DataIdentifierBorrowed::for_marker_attributes(attrs), ..Default::default() };
                if spaced {
                    return self.0.load(req);
                }
                icu_experimental::provider::Baked.load(req)
            }
            r => r,
        }
    }
}

/// A filter pattern with the pseudo-properties ICU's `UnicodeSet` knows
/// and ICU4X's parser does not (`[:ASCII:]`, `[:Any:]`, `[:Assigned:]`)
/// spelled out.
fn icu_set(pattern: &str) -> String {
    let mut s = pattern.to_string();
    for (name, set, neg) in [
        ("ASCII", "\\u0000-\\u007F", false),
        ("Any", "\\u0000-\\U0010FFFF", false),
        ("Assigned", "[:Cn:]", true),
    ] {
        for (from, negate) in [
            (format!("[:{name}:]"), false),
            (format!("[:^{name}:]"), true),
            (format!("\\p{{{name}}}"), false),
            (format!("\\P{{{name}}}"), true),
        ] {
            let caret = if negate != neg { "^" } else { "" };
            s = s.replace(&from, &format!("[{caret}{set}]"));
        }
    }
    s
}

/// One chain: the global filter and the pieces with their filters.
fn assemble(global: Option<&str>, steps: &[(Option<String>, Step)]) -> Option<Compiled> {
    let mut rules = String::new();
    if let Some(g) = global {
        rules.push_str(&format!(":: {} ;\n", icu_set(g)));
    }
    let mut coll = RuleCollection::default();
    let mut customs: Vec<Kind> = Vec::new();
    for (i, (filter, step)) in steps.iter().enumerate() {
        let name = match step {
            Step::Native(n) => n.to_string(),
            other => {
                let loc: Locale = match other {
                    Step::Rbt(Rbt::Baked(m)) => m.parse().ok()?,
                    Step::Rbt(Rbt::Source(file, rev)) => {
                        let loc: Locale = format!("und-x-s{i}").parse().ok()?;
                        coll.register_source(&loc, rules_source(file)?.to_string(), [], *rev, true);
                        loc
                    }
                    Step::Custom(k) => {
                        customs.push(k.clone());
                        format!("und-x-c{}", customs.len() - 1).parse().ok()?
                    }
                    Step::Native(_) => unreachable!(),
                };
                let alias = format!("Rphp-Step{i}");
                coll.register_aliases(&loc, [alias.as_str()]);
                alias
            }
        };
        rules.push_str(&format!(":: {} {name} ;\n", filter.as_deref().map(icu_set).unwrap_or_default()));
    }
    if steps.is_empty() {
        rules.push_str(":: Any-Null ;\n");
    }
    compile(coll, rules, customs, false)
}

/// Compile `rules` (as the top-level transliterator) in `coll`.
fn compile(mut coll: RuleCollection, rules: String, customs: Vec<Kind>, reverse: bool) -> Option<Compiled> {
    // What CLDR's rules reference by name and ICU implements in code.
    coll.register_aliases(&"und-x-title".parse().ok()?, ["Any-Title"]);
    coll.register_aliases(&"und-x-null".parse().ok()?, ["Any-BreakInternal"]);
    coll.register_source(
        &"und-x-spcdhan".parse().ok()?,
        include_str!("rules/Han-Spacedhan.txt").to_string(),
        ["Han-Spacedhan"],
        false,
        false,
    );
    let top: Locale = "und-x-top".parse().ok()?;
    coll.register_source(&top, rules, [], reverse, true);
    let provider = coll.as_provider();
    let lookup = |loc: &Locale| -> Option<Result<Box<dyn CustomTransliterator>, DataError>> {
        let s = loc.to_string();
        let tag = s.strip_prefix("und-x-")?;
        let t: Box<dyn CustomTransliterator> = match tag {
            "title" => Box::new(custom::Title),
            "null" => Box::new(custom::Identity),
            _ => customs.get(tag.strip_prefix('c')?.parse::<usize>().ok()?)?.instantiate(),
        };
        Some(Ok(t))
    };
    IcuTransliterator::try_new_with_override_unstable(
        &Provider(&provider),
        &icu::normalizer::provider::Baked,
        &icu::casemap::provider::Baked,
        &top,
        lookup,
    )
    .ok()
    .map(Compiled)
}

/// `Transliterator::createInstance(id, dir)`: the canonical ID and the
/// compiled chain, `None` for ICU's `U_INVALID_ID`.
pub(crate) fn build_id(id: &str, reverse: bool) -> Option<(String, Compiled)> {
    let c = id::parse_compound(id, reverse)?;
    let mut steps = Vec::new();
    for s in &c.list {
        if s.basic.is_empty() {
            continue;
        }
        steps.push((s.filter.clone(), resolve(&s.basic)?));
    }
    let t = assemble(c.global_filter.as_deref(), &steps)?;
    Some((c.canon, t))
}

// ---- the class -----------------------------------------------------------------------

/// The object's state: the chain (shared by clones; it is immutable) and
/// its own last error.
pub struct TranslitState {
    t: Rc<Compiled>,
    pub err: IntlError,
}

fn with_state<R>(o: &Object, f: impl FnOnce(&mut TranslitState) -> R) -> Option<R> {
    o.with_payload::<TranslitState, _>(f)
}

fn this(o: Option<&Object>) -> Result<&Object, Unwind> {
    o.ok_or_else(|| Unwind::error("Non-static method called statically"))
}

fn payload_clone(_: &mut Interp, src: &Object, dst: &Object) -> Result<(), Unwind> {
    if let Some(copy) = with_state(src, |s| TranslitState { t: s.t.clone(), err: s.err.clone() }) {
        dst.set_payload(Payload::Native(Box::new(copy)));
    }
    Ok(())
}

fn new_object(ctx: &mut Ctx, id: &str, t: Compiled) -> NativeResult {
    let cid = ctx.lookup_class_or_error(b"Transliterator")?;
    let obj = ctx.instantiate(cid);
    obj.set_payload(Payload::Native(Box::new(TranslitState { t: Rc::new(t), err: IntlError::default() })));
    obj.set(b"id", Value::string(id.as_bytes()));
    Ok(Value::Object(obj))
}

fn direction_arg(args: &[Value], i: usize) -> Result<bool, Unwind> {
    match args.get(i).map_or(0, Value::to_int) {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(Unwind::value_error(String::new())),
    }
}

fn direction_error(ctx: &mut Ctx, n: usize) -> Unwind {
    let who = ctx.active_function_name();
    Unwind::value_error(format!(
        "{who}(): Argument #{n} ($direction) must be either Transliterator::FORWARD or Transliterator::REVERSE"
    ))
}

/// `create_transliterator()`: the object, or `None` after setting the
/// global error.
fn open(ctx: &mut Ctx, id: &[u8], reverse: bool) -> Result<Option<Value>, Unwind> {
    let who = ctx.active_function_name();
    let Ok(text) = std::str::from_utf8(id) else {
        state::set_global(ctx, &who, U_INVALID_CHAR_FOUND, "String conversion of id to UTF-16 failed")?;
        return Ok(None);
    };
    match build_id(text, reverse) {
        Some((canon, t)) => Ok(Some(new_object(ctx, &canon, t)?)),
        None => {
            state::set_global(ctx, &who, U_INVALID_ID, &format!("unable to open ICU transliterator with id \"{text}\""))?;
            Ok(None)
        }
    }
}

fn construct(_: &mut Ctx, _: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Err(Unwind::exception("Exception", "An object of this type cannot be created with the new operator."))
}

fn create(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    state::reset_global(ctx);
    let reverse = direction_arg(args, 1).map_err(|_| direction_error(ctx, 2))?;
    Ok(open(ctx, &str_arg(args, 0), reverse)?.unwrap_or(Value::Null))
}

fn create_from_rules(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    state::reset_global(ctx);
    let reverse = direction_arg(args, 1).map_err(|_| direction_error(ctx, 2))?;
    let who = ctx.active_function_name();
    let raw = str_arg(args, 0);
    let Ok(rules) = std::str::from_utf8(&raw) else {
        state::set_global(ctx, &who, U_INVALID_CHAR_FOUND, "String conversion of rules to UTF-16 failed")?;
        return Ok(Value::Null);
    };
    match compile_rules(rules, reverse) {
        Some(t) => new_object(ctx, "RulesTransPHP", t),
        None => {
            let (code, offset) = rules_error(rules, reverse);
            let chars: Vec<char> = rules.chars().collect();
            let pre: String = chars[offset.saturating_sub(15)..offset].iter().collect();
            let post: String = chars[offset..].iter().take(15).collect();
            let msg = format!(
                "unable to create ICU transliterator from rules (parse error at offset {offset}, after \"{pre}\", before or at \"{post}\")"
            );
            state::set_global(ctx, &who, code, &msg)?;
            Ok(Value::Null)
        }
    }
}

/// `createFromRules()`'s compile: the rules may reference every
/// registered ID; ICU's last rule needs no `;`.
fn compile_rules(rules: &str, reverse: bool) -> Option<Compiled> {
    let mut coll = RuleCollection::default();
    for (i, (key, rbt)) in table::REGISTRY.iter().enumerate() {
        match rbt {
            Rbt::Baked(m) => coll.register_aliases(&m.parse().ok()?, [*key]),
            Rbt::Source(file, rev) => {
                let loc: Locale = format!("und-x-r{i}").parse().ok()?;
                coll.register_source(&loc, rules_source(file)?.to_string(), [*key], *rev, true);
            }
        }
    }
    let mut text = rules.to_string();
    if !split_rules(rules).last().is_none_or(|(_, r)| r.trim().is_empty() || r.trim_end().ends_with(';')) {
        text.push(';');
    }
    compile(coll, text, Vec::new(), reverse)
}

/// The rules of a rule text: each rule's char offset and its text (with
/// the `;`), split at the `;`s outside sets, quotes and escapes.
fn split_rules(rules: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let (mut start, mut depth, mut quoted, mut escaped) = (0usize, 0usize, false, false);
    let mut cur = String::new();
    for (i, c) in rules.chars().enumerate() {
        cur.push(c);
        if escaped {
            escaped = false;
            continue;
        }
        match c {
            '\\' => escaped = true,
            '\'' => quoted = !quoted,
            '[' if !quoted => depth += 1,
            ']' if !quoted => depth = depth.saturating_sub(1),
            ';' if !quoted && depth == 0 => {
                out.push((start, std::mem::take(&mut cur)));
                start = i + 1;
            }
            _ => {}
        }
    }
    if !cur.is_empty() {
        out.push((start, cur));
    }
    out
}

/// The code and offset ICU reports for rules ICU4X rejects: the first rule
/// whose addition breaks the compile, classified the way ICU's parser
/// would (an unclosed set, an undefined variable, else a malformed rule).
fn rules_error(rules: &str, reverse: bool) -> (i64, usize) {
    const U_MALFORMED_SET: i64 = 65538;
    const U_UNDEFINED_VARIABLE: i64 = 65554;
    let parts = split_rules(rules);
    let mut prefix = String::new();
    for (at, rule) in &parts {
        let before = prefix.clone();
        prefix.push_str(rule);
        if rule.trim().is_empty() || rule.trim() == ";" {
            continue;
        }
        if compile_rules(&prefix, reverse).is_some() {
            continue;
        }
        let lead = rule.chars().take_while(|c| c.is_whitespace()).count();
        let offset = at + lead;
        let body = rule.trim();
        let opens = body.matches('[').count();
        let closes = body.matches(']').count();
        if opens > closes {
            return (U_MALFORMED_SET, offset);
        }
        let defined = |name: &str| before.contains(&format!("${name}")) && before.contains('=');
        let mut rest = body;
        while let Some(p) = rest.find('$') {
            let name: String = rest[p + 1..].chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
            let is_def = body.starts_with('$') && p == 0 && body[1 + name.len()..].trim_start().starts_with('=');
            if !name.is_empty() && !is_def && !defined(&name) {
                return (U_UNDEFINED_VARIABLE, offset);
            }
            rest = &rest[p + 1..];
        }
        return (U_MALFORMED_RULE, offset);
    }
    (U_MALFORMED_RULE, parts.last().map_or(0, |(a, _)| *a))
}

fn create_inverse(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    state::reset_global(ctx);
    let o = this(o)?;
    with_state(o, |s| s.err.reset());
    let id = o.get(b"id").map(|v| v.to_php_bytes().to_vec()).unwrap_or_default();
    let who = ctx.active_function_name();
    let text = String::from_utf8_lossy(&id).into_owned();
    match build_id(&text, true) {
        Some((canon, t)) => new_object(ctx, &canon, t),
        None => {
            state::set_global(ctx, &who, U_INVALID_ID, "could not create inverse ICU transliterator")?;
            Ok(Value::Null)
        }
    }
}

fn list_ids(ctx: &mut Ctx, _: Option<&Object>, _: &mut [Value]) -> NativeResult {
    state::reset_global(ctx);
    let mut a = Array::new();
    for id in table::LIST_IDS {
        a.push(Value::string(id.as_bytes()));
    }
    Ok(Value::Array(a))
}

/// The UTF-16 length of `s` and the byte offset of UTF-16 index `u`
/// (`None` inside a surrogate pair).
fn utf16_to_byte(s: &str, u: usize) -> Option<usize> {
    let mut n = 0usize;
    for (b, c) in s.char_indices() {
        if n == u {
            return Some(b);
        }
        n += c.len_utf16();
        if n > u {
            return None;
        }
    }
    (n == u).then_some(s.len())
}

/// `transliterate()` on an object: `args` are `$string, $start, $end`;
/// `first` is the position of `$string` in the caller's argument list.
fn run(ctx: &mut Ctx, o: &Object, args: &[Value], first: usize) -> NativeResult {
    let start = args.get(1).map_or(0, Value::to_int);
    let end = args.get(2).map_or(-1, Value::to_int);
    let who = ctx.active_function_name();
    if end < -1 {
        return Err(Unwind::value_error(format!("{who}(): Argument #{} ($end) must be greater than or equal to -1", first + 2)));
    }
    if start < 0 {
        return Err(Unwind::value_error(format!("{who}(): Argument #{} ($start) must be greater than or equal to 0", first + 1)));
    }
    if end != -1 && start > end {
        return Err(Unwind::value_error(format!(
            "{who}(): Argument #{} ($start) must be less than or equal to argument #{} ($end)",
            first + 1,
            first + 2
        )));
    }
    with_state(o, |s| s.err.reset());
    let raw = str_arg(args, 0);
    let Ok(text) = std::str::from_utf8(&raw) else {
        let mut err = with_state(o, |s| s.err.clone()).unwrap_or_default();
        state::set_both(ctx, &mut err, &who, U_INVALID_CHAR_FOUND, "String conversion of string to UTF-16 failed")?;
        with_state(o, |s| s.err = err);
        return Ok(Value::Bool(false));
    };
    let units: usize = text.chars().map(char::len_utf16).sum();
    if start as usize > units || (end != -1 && end as usize > units) {
        let msg = format!(
            "Neither \"start\" nor the \"end\" arguments can exceed the number of UTF-16 code units (in this case, {units})"
        );
        let mut err = with_state(o, |s| s.err.clone()).unwrap_or_default();
        state::set_both(ctx, &mut err, &who, U_ILLEGAL_ARGUMENT_ERROR, &msg)?;
        with_state(o, |s| s.err = err);
        return Ok(Value::Bool(false));
    }
    let Some(t) = with_state(o, |s| s.t.clone()) else {
        return Err(Unwind::error("Found unconstructed transliterator"));
    };
    let end = if end == -1 { units } else { end as usize };
    // ICU's context is the range itself: transliterate the slice.
    let (Some(a), Some(b)) = (utf16_to_byte(text, start as usize), utf16_to_byte(text, end)) else {
        return Ok(Value::string(raw.as_slice()));
    };
    let mut out = String::with_capacity(text.len());
    out.push_str(&text[..a]);
    out.push_str(&t.run(text[a..b].to_string()));
    out.push_str(&text[b..]);
    Ok(Value::string(out.as_bytes()))
}

fn transliterate(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    state::reset_global(ctx);
    let o = this(o)?;
    run(ctx, o, args, 1)
}

fn get_error_code(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    state::reset_global(ctx);
    let o = this(o)?;
    Ok(Value::Int(with_state(o, |s| s.err.code).unwrap_or(0)))
}

fn get_error_message(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    state::reset_global(ctx);
    let o = this(o)?;
    let m = with_state(o, |s| s.err.message()).unwrap_or_else(|| state::error_name(0).to_string());
    Ok(Value::string(m.as_bytes()))
}

// ---- the functions -------------------------------------------------------------------

fn translit_arg(ctx: &mut Ctx, args: &[Value]) -> Result<Object, Unwind> {
    match args.first().map(|v| v.deref().into_owned()) {
        Some(Value::Object(o)) if o.with_payload::<TranslitState, _>(|_| ()).is_some() => Ok(o),
        other => {
            let who = ctx.active_function_name();
            let given = other.as_ref().map_or_else(|| "null".to_string(), value_name);
            Err(Unwind::type_error(format!("{who}(): Argument #1 ($transliterator) must be of type Transliterator, {given} given")))
        }
    }
}

fn f_create(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    create(ctx, None, args)
}

fn f_create_from_rules(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    create_from_rules(ctx, None, args)
}

fn f_create_inverse(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let o = translit_arg(ctx, args)?;
    create_inverse(ctx, Some(&o), &mut [])
}

fn f_list_ids(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    list_ids(ctx, None, args)
}

fn f_get_error_code(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let o = translit_arg(ctx, args)?;
    get_error_code(ctx, Some(&o), &mut [])
}

fn f_get_error_message(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let o = translit_arg(ctx, args)?;
    get_error_message(ctx, Some(&o), &mut [])
}

/// `transliterator_transliterate(Transliterator|string $transliterator,
/// string $string, int $start = 0, int $end = -1)`.
fn f_transliterate(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    state::reset_global(ctx);
    let first = args.first().map(|v| v.deref().into_owned()).unwrap_or(Value::Null);
    let o = match first {
        Value::Object(o) if o.with_payload::<TranslitState, _>(|_| ()).is_some() => o,
        Value::Object(_) | Value::Array(_) | Value::Null => {
            let who = ctx.active_function_name();
            return Err(Unwind::type_error(format!(
                "{who}(): Argument #1 ($transliterator) must be of type Transliterator|string, {} given",
                value_name(&first)
            )));
        }
        v => {
            let id = v.to_php_bytes().to_vec();
            match open(ctx, &id, false)? {
                Some(Value::Object(o)) => o,
                _ => {
                    let who = ctx.active_function_name();
                    let message = state::global(ctx).message();
                    let shown = String::from_utf8_lossy(&id).into_owned();
                    ctx.warn(&format!("{who}(): Could not create transliterator with ID \"{shown}\" ({message})"))?;
                    return Ok(Value::Bool(false));
                }
            }
        }
    };
    run(ctx, &o, &args[1..], 2)
}

static METHODS: &[MethodImpl] = &[
    ("__construct", construct),
    ("create", create),
    ("createFromRules", create_from_rules),
    ("createInverse", create_inverse),
    ("listIDs", list_ids),
    ("transliterate", transliterate),
    ("getErrorCode", get_error_code),
    ("getErrorMessage", get_error_message),
];

static FUNCTIONS: &[FnImpl] = &[
    ("transliterator_create", f_create),
    ("transliterator_create_from_rules", f_create_from_rules),
    ("transliterator_create_inverse", f_create_inverse),
    ("transliterator_list_ids", f_list_ids),
    ("transliterator_transliterate", f_transliterate),
    ("transliterator_get_error_code", f_get_error_code),
    ("transliterator_get_error_message", f_get_error_message),
];

pub fn register(r: &mut Registry) {
    register_class(r, &generated::classes::TRANSLITERATOR, METHODS, |b| {
        b.readonly_prop("id", "string").payload_clone(payload_clone)
    });
    register_functions(r, generated::arginfo::FUNCTIONS, FUNCTIONS);
}

