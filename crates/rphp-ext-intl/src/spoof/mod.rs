//! `Spoofchecker`: UTS #39 the way ICU 78's `uspoof_check2` and
//! `uspoof_areConfusable` implement it — restriction levels over the
//! augmented, resolved script sets (Script_Extensions from ICU4X), mixed
//! decimal-digit systems, repeated nonspacing marks (`INVISIBLE`), the
//! hidden-overlay dot, the allowed-character limit (a locale list's
//! scripts or a `UnicodeSet` pattern), and confusability by skeleton
//! (NFD, the confusables prototypes of Unicode 17's `confusables.txt`,
//! NFD again).

mod data;

use icu::casemap::{CaseMapCloser, ClosureSink};
use icu::collections::codepointinvliststringlist::CodePointInversionListAndStringList;
use icu::normalizer::DecomposingNormalizerBorrowed;
use icu::properties::props::{CanonicalCombiningClass, GeneralCategory, Script, SoftDotted};
use icu::properties::script::ScriptWithExtensions;
use icu::properties::{CodePointMapData, CodePointSetData, PropertyNamesShort};
use rphp_runtime::{Ctx, Interp, NativeResult, Registry, Unwind};
use rphp_value::{Object, Payload, Value};

use crate::shape::{register_class, MethodImpl};
use crate::{generated, str_arg};

const SINGLE_SCRIPT_CONFUSABLE: i64 = 1;
const MIXED_SCRIPT_CONFUSABLE: i64 = 2;
const WHOLE_SCRIPT_CONFUSABLE: i64 = 4;
const CONFUSABLE: i64 = 7;
const RESTRICTION_LEVEL: i64 = 16;
const INVISIBLE: i64 = 32;
const CHAR_LIMIT: i64 = 64;
const MIXED_NUMBERS: i64 = 128;
const HIDDEN_OVERLAY: i64 = 256;
const ALL_CHECKS: i64 = 0xFFFF;
const AUX_INFO: i64 = 0x4000_0000;

const ASCII: i64 = 0x1000_0000;
const SINGLE_SCRIPT_RESTRICTIVE: i64 = 0x2000_0000;
const HIGHLY_RESTRICTIVE: i64 = 0x3000_0000;
const MODERATELY_RESTRICTIVE: i64 = 0x4000_0000;
const MINIMALLY_RESTRICTIVE: i64 = 0x5000_0000;
const UNRESTRICTIVE: i64 = 0x6000_0000;

const IGNORE_SPACE: i64 = 1;
const CASE_INSENSITIVE: i64 = 2;
const ADD_CASE_MAPPINGS: i64 = 4;
const SIMPLE_CASE_INSENSITIVE: i64 = 6;

/// Which characters `CHAR_LIMIT` lets through.
#[derive(Clone)]
enum Allowed {
    All,
    /// A locale list's scripts, plus Common and Inherited.
    Scripts(Vec<Script>),
    /// A `UnicodeSet` pattern, and whether its case closure counts too.
    Set(CodePointInversionListAndStringList<'static>, bool),
}

impl Allowed {
    fn contains(&self, c: char) -> bool {
        match self {
            Allowed::All => true,
            Allowed::Scripts(list) => {
                let s = CodePointMapData::<Script>::new().get(c);
                s == Script::Common || s == Script::Inherited || list.contains(&s)
            }
            Allowed::Set(set, closure) => {
                if set.contains(c) {
                    return true;
                }
                if !closure {
                    return false;
                }
                let mut sink = Closure(Vec::new());
                CaseMapCloser::new().add_case_closure_to(c, &mut sink);
                sink.0.iter().any(|&x| set.contains(x))
            }
        }
    }
}

struct Closure(Vec<char>);

impl ClosureSink for Closure {
    fn add_char(&mut self, c: char) {
        self.0.push(c);
    }
    fn add_string(&mut self, _: &str) {}
}

/// The checker's configuration.
#[derive(Clone)]
pub struct SpoofState {
    checks: i64,
    level: i64,
    allowed: Allowed,
}

// ---- script sets ------------------------------------------------------------------------

/// A resolved script set: `None` is "every script".
type ScriptSet = Option<Vec<&'static str>>;

/// `getAugmentedScriptSet`: Script_Extensions with UTS #39's Hanb, Jpan
/// and Kore added; Common or Inherited is every script.
fn augmented(c: char) -> ScriptSet {
    let names = PropertyNamesShort::<Script>::new();
    let mut out: Vec<&'static str> = Vec::new();
    for s in ScriptWithExtensions::new().get_script_extensions_val(c).iter() {
        if s == Script::Common || s == Script::Inherited {
            return None;
        }
        let name = names.get(s).unwrap_or("Zzzz");
        out.push(name);
        let extra: &[&'static str] = match name {
            "Hani" => &["Hanb", "Jpan", "Kore"],
            "Hira" | "Kana" => &["Jpan"],
            "Hang" => &["Kore"],
            "Bopo" => &["Hanb"],
            _ => &[],
        };
        out.extend_from_slice(extra);
    }
    out.sort_unstable();
    out.dedup();
    Some(out)
}

fn intersect(a: ScriptSet, b: ScriptSet) -> ScriptSet {
    match (a, b) {
        (None, x) | (x, None) => x,
        (Some(a), Some(b)) => Some(a.into_iter().filter(|s| b.contains(s)).collect()),
    }
}

/// `getResolvedScriptSetWithout`: the intersection over the characters
/// whose set does not name `without`.
fn resolved(s: &str, without: Option<&str>) -> ScriptSet {
    let mut acc: ScriptSet = None;
    for c in s.chars() {
        let set = augmented(c);
        if let (Some(w), Some(list)) = (without, &set) {
            if list.contains(&w) {
                continue;
            }
        }
        if without.is_some() && set.is_none() {
            continue;
        }
        acc = intersect(acc, set);
    }
    acc
}

fn is_empty(s: &ScriptSet) -> bool {
    s.as_ref().is_some_and(Vec::is_empty)
}

fn has(s: &ScriptSet, name: &str) -> bool {
    s.as_ref().is_none_or(|v| v.contains(&name))
}

fn intersects(a: &ScriptSet, b: &ScriptSet) -> bool {
    match (a, b) {
        (None, None) => true,
        (None, Some(x)) | (Some(x), None) => !x.is_empty(),
        (Some(a), Some(b)) => a.iter().any(|s| b.contains(s)),
    }
}

// ---- the checks ---------------------------------------------------------------------------

impl SpoofState {
    /// `getRestrictionLevel` (UTS #39 section 5.2).
    fn restriction_level(&self, s: &str) -> i64 {
        if !s.chars().all(|c| self.allowed.contains(c)) {
            return UNRESTRICTIVE;
        }
        if s.is_ascii() {
            return ASCII;
        }
        if !is_empty(&resolved(s, None)) {
            return SINGLE_SCRIPT_RESTRICTIVE;
        }
        let no_latin = resolved(s, Some("Latn"));
        if has(&no_latin, "Hanb") || has(&no_latin, "Jpan") || has(&no_latin, "Kore") {
            return HIGHLY_RESTRICTIVE;
        }
        if !is_empty(&no_latin) && !has(&no_latin, "Cyrl") && !has(&no_latin, "Grek") && !has(&no_latin, "Cher") {
            return MODERATELY_RESTRICTIVE;
        }
        MINIMALLY_RESTRICTIVE
    }

    /// `uspoof_check2`.
    fn check(&self, s: &str) -> i64 {
        let mut result = 0;
        if self.checks & RESTRICTION_LEVEL != 0 && self.restriction_level(s) > self.level {
            result |= RESTRICTION_LEVEL;
        }
        if self.checks & MIXED_NUMBERS != 0 {
            let gc = CodePointMapData::<GeneralCategory>::new();
            let mut zeros: Vec<u32> = Vec::new();
            for c in s.chars() {
                if gc.get(c) == GeneralCategory::DecimalNumber {
                    let zero = c as u32 - c.to_digit(10).unwrap_or_else(|| digit_value(c));
                    if !zeros.contains(&zero) {
                        zeros.push(zero);
                    }
                }
            }
            if zeros.len() > 1 {
                result |= MIXED_NUMBERS;
            }
        }
        if self.checks & HIDDEN_OVERLAY != 0 && hidden_overlay(s) {
            result |= HIDDEN_OVERLAY;
        }
        if self.checks & CHAR_LIMIT != 0 && !s.chars().all(|c| self.allowed.contains(c)) {
            result |= CHAR_LIMIT;
        }
        if self.checks & INVISIBLE != 0 && invisible(s) {
            result |= INVISIBLE;
        }
        result
    }

    /// `uspoof_areConfusable`; `None` is ICU's `U_INVALID_STATE_ERROR`
    /// (no confusable check enabled).
    fn confusable(&self, a: &str, b: &str) -> Option<i64> {
        if self.checks & CONFUSABLE == 0 {
            return None;
        }
        if skeleton(a) != skeleton(b) {
            return Some(0);
        }
        let (ra, rb) = (resolved(a, None), resolved(b, None));
        let mut result = if intersects(&ra, &rb) {
            SINGLE_SCRIPT_CONFUSABLE
        } else if !is_empty(&ra) && !is_empty(&rb) {
            MIXED_SCRIPT_CONFUSABLE | WHOLE_SCRIPT_CONFUSABLE
        } else {
            MIXED_SCRIPT_CONFUSABLE
        };
        result &= self.checks | !CONFUSABLE;
        Some(result)
    }
}

/// The value of a decimal digit ICU4X's `to_digit` does not know.
fn digit_value(c: char) -> u32 {
    let gc = CodePointMapData::<GeneralCategory>::new();
    let mut v = 0;
    let mut cp = c as u32;
    while v < 9 {
        match char::from_u32(cp - 1) {
            Some(p) if gc.get(p) == GeneralCategory::DecimalNumber => {
                v += 1;
                cp -= 1;
            }
            _ => break,
        }
    }
    v
}

/// The same nonspacing mark twice in one run of marks, after NFD.
fn invisible(s: &str) -> bool {
    let nfd = DecomposingNormalizerBorrowed::new_nfd().normalize(s);
    let gc = CodePointMapData::<GeneralCategory>::new();
    let mut seen: Vec<char> = Vec::new();
    for c in nfd.chars() {
        if gc.get(c) != GeneralCategory::NonspacingMark {
            seen.clear();
            continue;
        }
        if seen.contains(&c) {
            return true;
        }
        seen.push(c);
    }
    false
}

/// `isIllegalCombiningDotLeadCharacterNoLookup`.
fn dotted_lead(c: char) -> bool {
    matches!(c, 'i' | 'j' | 'ı' | 'ȷ' | 'l') || CodePointSetData::new::<SoftDotted>().contains(c)
}

/// `findHiddenOverlay`: a U+0307 after a (possibly confusable) dotted
/// letter, skipping marks of other combining classes.
fn hidden_overlay(s: &str) -> bool {
    let ccc = CodePointMapData::<CanonicalCombiningClass>::new();
    let mut lead = false;
    for c in s.chars() {
        if lead && c == '\u{0307}' {
            return true;
        }
        let cc = ccc.get(c);
        if cc == CanonicalCombiningClass::NotReordered || cc == CanonicalCombiningClass::Above {
            lead = dotted_lead(c) || {
                let proto = prototype(c);
                proto.and_then(|p| p.chars().next_back()).is_some_and(|f| f != c && dotted_lead(f))
            };
        }
    }
    false
}

fn prototype(c: char) -> Option<&'static str> {
    data::CONFUSABLES.binary_search_by_key(&(c as u32), |r| r.0).ok().map(|i| data::CONFUSABLES[i].1)
}

/// UTS #39's skeleton: NFD, each character to its prototype, NFD.
fn skeleton(s: &str) -> String {
    let nfd = DecomposingNormalizerBorrowed::new_nfd();
    let mut mapped = String::new();
    for c in nfd.normalize(s).chars() {
        match prototype(c) {
            Some(p) => mapped.push_str(p),
            None => mapped.push(c),
        }
    }
    nfd.normalize(&mapped).into_owned()
}

// ---- the class ----------------------------------------------------------------------------

fn this(o: Option<&Object>) -> Result<&Object, Unwind> {
    o.ok_or_else(|| Unwind::error("Non-static method called statically"))
}

fn state<R>(o: Option<&Object>, f: impl FnOnce(&mut SpoofState) -> R) -> Result<R, Unwind> {
    this(o)?.with_payload::<SpoofState, _>(f).ok_or_else(|| Unwind::error("Found unconstructed Spoofchecker"))
}

fn payload_clone(_: &mut Interp, src: &Object, dst: &Object) -> Result<(), Unwind> {
    if let Some(copy) = src.with_payload::<SpoofState, _>(|s| s.clone()) {
        dst.set_payload(Payload::Native(Box::new(copy)));
    }
    Ok(())
}

/// ICU's UTF-8 reading: an ill-formed sequence is U+FFFD.
fn text(v: &[u8]) -> String {
    String::from_utf8_lossy(v).into_owned()
}

fn construct(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    o.set_payload(Payload::Native(Box::new(SpoofState {
        checks: ALL_CHECKS,
        level: HIGHLY_RESTRICTIVE,
        allowed: Allowed::All,
    })));
    Ok(Value::Null)
}

fn is_suspicious(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let s = text(&str_arg(args, 0));
    let r = state(o, |st| st.check(&s))?;
    if let Some(slot) = args.get_mut(1) {
        Value::assign(slot, Value::Int(r));
    }
    Ok(Value::Bool(r != 0))
}

fn are_confusable(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let a = text(&str_arg(args, 0));
    let b = text(&str_arg(args, 1));
    let Some(r) = state(o, |st| st.confusable(&a, &b))? else {
        let who = ctx.active_function_name();
        ctx.warn(&format!("{who}(): (27) U_INVALID_STATE_ERROR"))?;
        return Ok(Value::Bool(true));
    };
    if let Some(slot) = args.get_mut(2) {
        Value::assign(slot, Value::Int(r));
    }
    Ok(Value::Bool(r != 0))
}

/// `uscript_getCode()` for a locale: its scripts.
fn locale_scripts(loc: &str) -> Vec<Script> {
    let lang = loc.split(['_', '-']).next().unwrap_or("").to_ascii_lowercase();
    match lang.as_str() {
        "ja" => return vec![Script::Katakana, Script::Hiragana, Script::Han],
        "ko" => return vec![Script::Hangul, Script::Han],
        _ => {}
    }
    match crate::translit::script_code(loc) {
        Some(s) => vec![s],
        // ICU's likely subtags make an unknown language Latin.
        None if !loc.is_empty() => vec![Script::Latin],
        None => vec![],
    }
}

fn set_allowed_locales(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let list = text(&str_arg(args, 0));
    let mut scripts: Vec<Script> = Vec::new();
    for loc in list.split(',') {
        let loc = loc.trim();
        if loc.is_empty() {
            continue;
        }
        for s in locale_scripts(loc) {
            if !scripts.contains(&s) {
                scripts.push(s);
            }
        }
    }
    state(o, |st| {
        if scripts.is_empty() {
            st.allowed = Allowed::All;
            st.checks &= !CHAR_LIMIT;
        } else {
            st.allowed = Allowed::Scripts(scripts);
            st.checks |= CHAR_LIMIT;
        }
    })?;
    Ok(Value::Null)
}

fn set_checks(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let checks = args.first().map_or(0, Value::to_int);
    if checks & !(ALL_CHECKS | AUX_INFO) != 0 {
        let who = ctx.active_function_name();
        ctx.warn(&format!("{who}(): (1) U_ILLEGAL_ARGUMENT_ERROR"))?;
        return Ok(Value::Null);
    }
    state(o, |st| st.checks = checks)?;
    Ok(Value::Null)
}

fn set_restriction_level(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let level = args.first().map_or(0, Value::to_int);
    if ![ASCII, SINGLE_SCRIPT_RESTRICTIVE, HIGHLY_RESTRICTIVE, MODERATELY_RESTRICTIVE, MINIMALLY_RESTRICTIVE, UNRESTRICTIVE]
        .contains(&level)
    {
        let who = ctx.active_function_name();
        return Err(Unwind::value_error(format!(
            "{who}(): Argument #1 ($level) must be one of Spoofchecker::ASCII, Spoofchecker::SINGLE_SCRIPT_RESTRICTIVE, Spoofchecker::HIGHLY_RESTRICTIVE, Spoofchecker::MODERATELY_RESTRICTIVE, Spoofchecker::MINIMALLY_RESTRICTIVE, or Spoofchecker::UNRESTRICTIVE"
        )));
    }
    state(o, |st| st.level = level)?;
    Ok(Value::Null)
}

fn set_allowed_chars(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let raw = str_arg(args, 0);
    let option = args.get(1).map_or(0, Value::to_int);
    let who = ctx.active_function_name();
    state(o, |_| ())?;
    if raw.first() != Some(&b'[') || raw.last() != Some(&b']') {
        return Err(Unwind::value_error(format!(
            "{who}(): Argument #1 ($pattern) must be a valid regular expression character set pattern"
        )));
    }
    if option != 0
        && ![IGNORE_SPACE, IGNORE_SPACE | CASE_INSENSITIVE, IGNORE_SPACE | ADD_CASE_MAPPINGS, IGNORE_SPACE | SIMPLE_CASE_INSENSITIVE]
            .contains(&option)
    {
        return Err(Unwind::value_error(format!(
            "{who}(): Argument #2 ($patternOptions) must be a valid pattern option, 0 or (SpoofChecker::IGNORE_SPACE|(<none> or SpoofChecker::CASE_INSENSITIVE or SpoofChecker::ADD_CASE_MAPPINGS or SpoofChecker::SIMPLE_CASE_INSENSITIVE))"
        )));
    }
    let pattern = text(&raw);
    // Without IGNORE_SPACE a space in the pattern is a member.
    let pattern = if option & IGNORE_SPACE == 0 { pattern.replace(' ', "\\u0020") } else { pattern };
    let set = match icu::properties::unicodeset_parse::parse(&pattern) {
        Ok((set, used)) if used == pattern.len() => set,
        _ => {
            return Err(Unwind::value_error(format!(
                "{who}(): Argument #1 ($pattern) must be a valid regular expression character set pattern (65563) U_MALFORMED_SET"
            )))
        }
    };
    state(o, |st| st.allowed = Allowed::Set(set, option & (CASE_INSENSITIVE | ADD_CASE_MAPPINGS) != 0))?;
    Ok(Value::Null)
}

static METHODS: &[MethodImpl] = &[
    ("__construct", construct),
    ("isSuspicious", is_suspicious),
    ("areConfusable", are_confusable),
    ("setAllowedLocales", set_allowed_locales),
    ("setChecks", set_checks),
    ("setRestrictionLevel", set_restriction_level),
    ("setAllowedChars", set_allowed_chars),
];

pub fn register(r: &mut Registry) {
    register_class(r, &generated::classes::SPOOFCHECKER, METHODS, |b| b.payload_clone(payload_clone));
}
