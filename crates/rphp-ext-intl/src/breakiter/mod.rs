//! `IntlBreakIterator` and its family — `IntlRuleBasedBreakIterator` (every
//! factory but one hands one out), `IntlCodePointBreakIterator`,
//! `IntlPartsIterator` — over ICU4X's segmenters (`segment.rs`).
//!
//! The boundaries of the text are computed when it is set; the cursor
//! then moves over them the way ICU's `RuleBasedBreakIterator` and its
//! break cache move (offsets are UTF-8 byte offsets, as php's `UText`
//! gives them; an offset inside a code point is moved back to its start;
//! a `DONE` reached going forward stays sticky for `next()` until the
//! cursor is repositioned, as in ICU). Which rule set a locale loads and
//! the locales `getLocale()` reports are dumped from php (`data.rs`); the
//! rule sources are kept so `getRules()` and the rules constructor
//! round-trip.
//!
//! Not implemented (catalogued): rules other than ICU's own built-in rule
//! sets in the constructor, the compiled-rules form, `getBinaryRules()`.

mod data;
mod segment;

use rphp_runtime::{Ctx, Interp, NativeResult, Registry, Unwind};
use rphp_value::{Array, Object, Payload, Value};

use crate::shape::{register_class, MethodImpl};
use crate::state::{self, IntlError, U_ILLEGAL_ARGUMENT_ERROR};
use crate::{generated, locale, locale_arg, str_arg};
use segment::{Config, Text};

/// The factories' iterator kinds (the dump's rows).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Word,
    Line,
    Sentence,
    Character,
    Title,
}

const DONE: i64 = -1;

/// A break iterator: its rule set (or the code point iterator), locales,
/// text with boundaries, cursor and last error.
pub struct BiState {
    /// `None` for `IntlCodePointBreakIterator`.
    cfg: Option<(Config, usize)>,
    valid: String,
    actual: String,
    text: Option<Vec<u8>>,
    t: Text,
    bounds: Vec<usize>,
    status: Vec<i64>,
    pos: usize,
    done: bool,
    last_cp: i64,
    pub err: IntlError,
}

impl BiState {
    fn new(cfg: Option<(Config, usize)>, valid: String, actual: String) -> Self {
        let mut s = BiState {
            cfg,
            valid,
            actual,
            text: None,
            t: Text::new(b""),
            bounds: vec![0],
            status: vec![0],
            pos: 0,
            done: false,
            last_cp: -1,
            err: IntlError::default(),
        };
        s.set_text(b"");
        s.text = None;
        s
    }

    fn copy(&self) -> Self {
        BiState {
            cfg: self.cfg,
            valid: self.valid.clone(),
            actual: self.actual.clone(),
            text: self.text.clone(),
            t: Text::new(self.text.as_deref().unwrap_or(b"")),
            bounds: self.bounds.clone(),
            status: self.status.clone(),
            pos: self.pos,
            done: self.done,
            last_cp: self.last_cp,
            err: self.err.clone(),
        }
    }

    fn len(&self) -> usize {
        *self.t.offsets.last().unwrap_or(&0)
    }

    fn set_text(&mut self, bytes: &[u8]) {
        self.t = Text::new(bytes);
        self.text = Some(bytes.to_vec());
        match self.cfg {
            Some((cfg, _)) => {
                let (b, s) = segment::boundaries(cfg, &self.t);
                self.bounds = b;
                self.status = s;
            }
            None => {
                self.bounds = self.t.offsets.clone();
                self.status = vec![0; self.bounds.len()];
            }
        }
        self.pos = 0;
        self.done = false;
        self.last_cp = -1;
    }

    fn ret(&self) -> i64 {
        if self.done {
            DONE
        } else {
            self.pos as i64
        }
    }

    // ---- ICU's RuleBasedBreakIterator ------------------------------------------------

    fn first(&mut self) -> i64 {
        self.pos = 0;
        self.done = false;
        self.last_cp = -1;
        0
    }

    fn last(&mut self) -> i64 {
        self.pos = self.len();
        self.done = false;
        self.last_cp = -1;
        self.pos as i64
    }

    /// The boundary at or before `o`.
    fn seek(&mut self, o: usize) {
        self.pos = match self.bounds.binary_search(&o) {
            Ok(i) => self.bounds[i],
            Err(i) => self.bounds[i.saturating_sub(1)],
        };
        self.done = false;
    }

    fn next(&mut self) -> i64 {
        if self.cfg.is_none() {
            return self.cp_next();
        }
        match self.bounds.iter().find(|&&b| b > self.pos) {
            // The break cache's step keeps a DONE flag set.
            Some(&b) => self.pos = b,
            None => self.done = true,
        }
        self.ret()
    }

    fn previous(&mut self) -> i64 {
        if self.cfg.is_none() {
            return self.cp_previous();
        }
        match self.bounds.iter().rev().find(|&&b| b < self.pos) {
            Some(&b) => {
                self.pos = b;
                self.done = false;
            }
            None => self.done = true,
        }
        self.ret()
    }

    fn next_n(&mut self, n: i64) -> i64 {
        if self.cfg.is_none() {
            return self.cp_move(n);
        }
        let mut result = 0;
        if n > 0 {
            for _ in 0..n {
                result = self.next();
                if result == DONE {
                    break;
                }
            }
        } else if n < 0 {
            for _ in n..0 {
                result = self.previous();
                if result == DONE {
                    break;
                }
            }
        } else {
            result = self.pos as i64;
        }
        result
    }

    fn following(&mut self, o: i64) -> i64 {
        if self.cfg.is_none() {
            self.pos = self.t.snap(o);
            return self.cp_next();
        }
        if o < 0 {
            return self.first();
        }
        let o = self.t.snap(o);
        self.seek(o);
        self.next()
    }

    fn preceding(&mut self, o: i64) -> i64 {
        if self.cfg.is_none() {
            self.pos = self.t.snap(o);
            return self.cp_previous();
        }
        if o > self.len() as i64 {
            return self.last();
        }
        let o = self.t.snap(o);
        if self.bounds.binary_search(&o).is_ok() {
            self.pos = o;
            self.previous()
        } else {
            self.seek(o);
            self.ret()
        }
    }

    fn is_boundary(&mut self, o: i64) -> bool {
        if self.cfg.is_none() {
            let s = self.t.snap(o);
            self.pos = s;
            return o == s as i64;
        }
        if o < 0 {
            self.first();
            return false;
        }
        let adjusted = self.t.snap(o);
        self.seek(adjusted);
        let result = self.pos as i64 == o;
        if !result {
            self.next();
        }
        result
    }

    fn rule_status(&self) -> i64 {
        self.bounds.binary_search(&self.pos).ok().map_or(0, |i| self.status[i])
    }

    // ---- php's CodePointBreakIterator ------------------------------------------------

    fn cp_next(&mut self) -> i64 {
        match self.t.char_at(self.pos) {
            Some(i) => {
                self.last_cp = self.t.chars[i] as i64;
                self.pos = self.t.offsets[i + 1];
                self.pos as i64
            }
            None => {
                self.last_cp = -1;
                DONE
            }
        }
    }

    fn cp_previous(&mut self) -> i64 {
        if self.pos == 0 {
            self.last_cp = -1;
            return DONE;
        }
        let i = match self.t.offsets.binary_search(&self.pos) {
            Ok(i) | Err(i) => i - 1,
        };
        self.last_cp = self.t.chars[i] as i64;
        self.pos = self.t.offsets[i];
        self.pos as i64
    }

    /// `utext_moveIndex32`, then the code point at the new position.
    fn cp_move(&mut self, n: i64) -> i64 {
        let mut ok = true;
        if n > 0 {
            for _ in 0..n {
                if self.t.char_at(self.pos).is_none() {
                    ok = false;
                    break;
                }
                self.cp_next();
            }
        } else {
            for _ in n..0 {
                if self.pos == 0 {
                    ok = false;
                    break;
                }
                self.cp_previous();
            }
        }
        if !ok {
            self.last_cp = -1;
            return DONE;
        }
        self.last_cp = self.t.char_at(self.pos).map_or(-1, |i| self.t.chars[i] as i64);
        self.pos as i64
    }
}

// ---- plumbing ---------------------------------------------------------------------------

fn with_state<R>(o: &Object, f: impl FnOnce(&mut BiState) -> R) -> Option<R> {
    o.with_payload::<BiState, _>(f)
}

fn this(o: Option<&Object>) -> Result<&Object, Unwind> {
    o.ok_or_else(|| Unwind::error("Non-static method called statically"))
}

/// The receiver's state, or php's error for an unconstructed one.
fn state<R>(o: Option<&Object>, f: impl FnOnce(&mut BiState) -> R) -> Result<R, Unwind> {
    let o = this(o)?;
    with_state(o, |s| {
        s.err.reset();
        f(s)
    })
    .ok_or_else(|| Unwind::error("Found unconstructed BreakIterator"))
}

fn payload_clone(_: &mut Interp, src: &Object, dst: &Object) -> Result<(), Unwind> {
    if let Some(copy) = with_state(src, |s| s.copy()) {
        dst.set_payload(Payload::Native(Box::new(copy)));
    }
    Ok(())
}

fn int32_arg(ctx: &mut Ctx, args: &[Value], i: usize, name: &str) -> Result<i64, Unwind> {
    let v = args.get(i).map_or(0, Value::to_int);
    if v < i32::MIN as i64 || v > i32::MAX as i64 {
        let who = ctx.active_function_name();
        return Err(Unwind::value_error(format!(
            "{who}(): Argument #{} (${name}) must be between {} and {}",
            i + 1,
            i32::MIN,
            i32::MAX
        )));
    }
    Ok(v)
}

/// What a factory's locale resolves to: the valid and actual locale and
/// the rule set, from the dump; ICU's bundle fallback for the rest.
fn resolve(kind: Kind, requested: &str) -> (String, String, usize) {
    let root = data::ROOT.iter().find(|r| r.0 == kind).map_or(0, |r| r.1);
    let canonical = locale::canonical(requested);
    let find = |loc: &str| data::LOCALES.iter().find(|r| r.0 == kind && r.1 == loc);
    if let Some(r) = find(&canonical) {
        return (r.2.to_string(), r.3.to_string(), r.4);
    }
    let (base, keywords) = match canonical.split_once('@') {
        Some((b, k)) => (b.to_string(), Some(k.to_ascii_lowercase())),
        None => (canonical.clone(), None),
    };
    let mut cand = base.clone();
    let mut found = None;
    loop {
        if let Some(r) = find(&cand) {
            found = Some((r.2.to_string(), r.3.to_string(), r.4));
            break;
        }
        match cand.rfind('_') {
            Some(i) => cand.truncate(i),
            None => break,
        }
    }
    let (valid, actual, mut rules) = found.unwrap_or((String::new(), String::new(), root));
    if kind == Kind::Line {
        let lb = keywords.as_deref().and_then(|k| k.split(';').find_map(|kv| kv.strip_prefix("lb=")));
        let cj = matches!(rules, 5..=7) || (rules == 4 && !actual.is_empty());
        rules = match (lb, cj) {
            (Some("loose"), true) => 5,
            (Some("normal"), true) => 6,
            (Some("strict"), true) => 7,
            (Some("loose"), false) => 3,
            (Some("normal"), false) => 4,
            (Some("strict"), false) => 2,
            _ => rules,
        };
    }
    (valid, actual, rules)
}

fn new_object(ctx: &mut Ctx, class: &[u8], st: BiState) -> NativeResult {
    let cid = ctx.lookup_class_or_error(class)?;
    let obj = ctx.instantiate(cid);
    obj.set_payload(Payload::Native(Box::new(st)));
    Ok(Value::Object(obj))
}

fn factory(ctx: &mut Ctx, args: &[Value], kind: Kind) -> NativeResult {
    state::reset_global(ctx);
    let loc = locale_arg(ctx, args, 0);
    let (valid, actual, rules) = resolve(kind, &loc);
    let st = BiState::new(Some((segment::config(kind, rules), rules)), valid, actual);
    new_object(ctx, b"IntlRuleBasedBreakIterator", st)
}

// ---- IntlBreakIterator ------------------------------------------------------------------

fn construct(_: &mut Ctx, _: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Err(Unwind::exception("Exception", "An object of this type cannot be created with the new operator"))
}

fn create_word(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    factory(ctx, args, Kind::Word)
}

fn create_line(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    factory(ctx, args, Kind::Line)
}

fn create_sentence(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    factory(ctx, args, Kind::Sentence)
}

fn create_character(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    factory(ctx, args, Kind::Character)
}

fn create_title(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    factory(ctx, args, Kind::Title)
}

fn create_code_point(ctx: &mut Ctx, _: Option<&Object>, _: &mut [Value]) -> NativeResult {
    state::reset_global(ctx);
    new_object(ctx, b"IntlCodePointBreakIterator", BiState::new(None, String::new(), String::new()))
}

fn get_text(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    state(o, |s| s.text.as_ref().map_or(Value::Null, |t| Value::string(t.as_slice())))
}

fn set_text(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let text = str_arg(args, 0);
    state(o, |s| s.set_text(&text))?;
    Ok(Value::Bool(true))
}

fn first(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Ok(Value::Int(state(o, BiState::first)?))
}

fn last(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Ok(Value::Int(state(o, BiState::last)?))
}

fn previous(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Ok(Value::Int(state(o, BiState::previous)?))
}

fn next(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    if crate::opt_arg(args, 0).is_none() {
        return Ok(Value::Int(state(o, BiState::next)?));
    }
    let n = int32_arg(ctx, args, 0, "offset")?;
    Ok(Value::Int(state(o, |s| s.next_n(n))?))
}

fn current(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Ok(Value::Int(state(o, |s| s.pos as i64)?))
}

fn following(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let n = int32_arg(ctx, args, 0, "offset")?;
    Ok(Value::Int(state(o, |s| s.following(n))?))
}

fn preceding(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let n = int32_arg(ctx, args, 0, "offset")?;
    Ok(Value::Int(state(o, |s| s.preceding(n))?))
}

fn is_boundary(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let n = int32_arg(ctx, args, 0, "offset")?;
    Ok(Value::Bool(state(o, |s| s.is_boundary(n))?))
}

fn get_locale(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let ty = args.first().map_or(0, Value::to_int);
    if ty != 0 && ty != 1 {
        let who = ctx.active_function_name();
        state::set_global(ctx, &who, U_ILLEGAL_ARGUMENT_ERROR, "invalid locale type")?;
        return Ok(Value::Bool(false));
    }
    let l = state(o, |s| if ty == 0 { s.actual.clone() } else { s.valid.clone() })?;
    Ok(Value::string(l.as_bytes()))
}

fn get_parts_iterator(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let key_type = args.first().map_or(0, Value::to_int);
    if !(0..=2).contains(&key_type) {
        let who = ctx.active_function_name();
        return Err(Unwind::value_error(format!(
            "{who}(): Argument #1 ($type) must be one of IntlPartsIterator::KEY_SEQUENTIAL, IntlPartsIterator::KEY_LEFT, or IntlPartsIterator::KEY_RIGHT"
        )));
    }
    let bi = this(o)?.clone();
    state(Some(&bi), |_| ())?;
    let cid = ctx.lookup_class_or_error(b"IntlPartsIterator")?;
    let obj = ctx.instantiate(cid);
    obj.set_payload(Payload::Native(Box::new(PartsState { bi, key_type, current: None, key: 0 })));
    Ok(Value::Object(obj))
}

fn get_error_code(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Int(with_state(o, |s| s.err.code).unwrap_or(0)))
}

fn get_error_message(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let m = with_state(o, |s| s.err.message()).unwrap_or_else(|| state::error_name(0).to_string());
    Ok(Value::string(m.as_bytes()))
}

/// `getIterator()`: php's `InternalIterator` driving this iterator —
/// `first()` on rewind, `next()` per step, keyed by the step count.
fn get_iterator(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let bi = this(o)?.clone();
    state(Some(&bi), |_| ())?;
    let mut key = 0i64;
    let step = move |rewind: bool| -> Option<(Value, Value)> {
        let pos = with_state(&bi, |s| {
            s.err.reset();
            if rewind {
                s.first()
            } else {
                s.next()
            }
        })?;
        key = if rewind { 0 } else { key + 1 };
        (pos != DONE).then_some((Value::Int(key), Value::Int(pos)))
    };
    Ok(Value::Object(rphp_stdlib::new_live_internal_iterator(ctx, Box::new(step))))
}

// ---- IntlRuleBasedBreakIterator -----------------------------------------------------------

fn rbbi_construct(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    state::reset_global(ctx);
    let rules = str_arg(args, 0);
    let compiled = args.get(1).is_some_and(Value::to_bool);
    let who = ctx.active_function_name();
    if compiled {
        return Err(Unwind::error(format!("{who}(): compiled break rules are not implemented by rphp's intl yet")));
    }
    let squeeze = |s: &[u8]| s.iter().copied().filter(|b| !b.is_ascii_whitespace()).collect::<Vec<u8>>();
    let wanted = squeeze(&rules);
    let Some(idx) = data::RULES.iter().position(|r| squeeze(r.as_bytes()) == wanted) else {
        return Err(Unwind::error(format!(
            "{who}(): break rules other than ICU's own rule sets are not implemented by rphp's intl yet"
        )));
    };
    let kind = match idx {
        0 | 1 => Kind::Word,
        8 | 9 => Kind::Sentence,
        10 => Kind::Character,
        11 => Kind::Title,
        _ => Kind::Line,
    };
    o.set_payload(Payload::Native(Box::new(BiState::new(
        Some((segment::config(kind, idx), idx)),
        String::new(),
        String::new(),
    ))));
    Ok(Value::Null)
}

fn get_rules(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let r = state(o, |s| s.cfg.map_or("", |c| data::RULES[c.1]))?;
    Ok(Value::string(r.as_bytes()))
}

fn get_rule_status(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Ok(Value::Int(state(o, |s| s.rule_status())?))
}

fn get_rule_status_vec(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let st = state(o, |s| s.rule_status())?;
    let mut a = Array::new();
    a.push(Value::Int(st));
    Ok(Value::Array(a))
}

fn get_last_code_point(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Ok(Value::Int(state(o, |s| s.last_cp)?))
}

// ---- IntlPartsIterator --------------------------------------------------------------------

/// The parts iterator: the break iterator it walks, the key kind, the
/// current part and key.
pub struct PartsState {
    bi: Object,
    key_type: i64,
    current: Option<Value>,
    key: i64,
}

fn parts<R>(o: Option<&Object>, f: impl FnOnce(&mut PartsState) -> R) -> Result<R, Unwind> {
    this(o)?.with_payload::<PartsState, _>(f).ok_or_else(|| Unwind::error("Found unconstructed IntlIterator"))
}

/// `_breakiterator_parts_move_forward`.
fn parts_step(p: &mut PartsState) {
    p.current = None;
    let Some(step) = with_state(&p.bi, |s| {
        let cur = s.pos;
        let next = s.next();
        (next != DONE).then(|| {
            let text = s.text.clone().unwrap_or_default();
            (cur, next as usize, Value::string(&text[cur..next as usize]))
        })
    })
    .flatten() else {
        return;
    };
    let (cur, next, v) = step;
    match p.key_type {
        1 => p.key = cur as i64,
        2 => p.key = next as i64,
        _ => {}
    }
    p.current = Some(v);
}

fn parts_rewind(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    parts(o, |p| {
        with_state(&p.bi, BiState::first);
        p.key = 0;
        parts_step(p);
    })?;
    Ok(Value::Null)
}

fn parts_next(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    parts(o, |p| {
        parts_step(p);
        if p.key_type == 0 {
            p.key += 1;
        }
    })?;
    Ok(Value::Null)
}

fn parts_valid(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Ok(Value::Bool(parts(o, |p| p.current.is_some())?))
}

fn parts_current(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Ok(parts(o, |p| p.current.clone())?.unwrap_or(Value::Null))
}

fn parts_key(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    parts(o, |p| if p.current.is_some() { Value::Int(p.key) } else { Value::Null })
}

fn parts_get_break_iterator(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Ok(Value::Object(parts(o, |p| p.bi.clone())?))
}

fn parts_get_rule_status(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let bi = parts(o, |p| p.bi.clone())?;
    Ok(Value::Int(with_state(&bi, |s| s.rule_status()).unwrap_or(0)))
}

// ---- registration ----------------------------------------------------------------------------

static METHODS: &[MethodImpl] = &[
    ("__construct", construct),
    ("createWordInstance", create_word),
    ("createLineInstance", create_line),
    ("createSentenceInstance", create_sentence),
    ("createCharacterInstance", create_character),
    ("createTitleInstance", create_title),
    ("createCodePointInstance", create_code_point),
    ("getText", get_text),
    ("setText", set_text),
    ("first", first),
    ("last", last),
    ("previous", previous),
    ("next", next),
    ("current", current),
    ("following", following),
    ("preceding", preceding),
    ("isBoundary", is_boundary),
    ("getLocale", get_locale),
    ("getPartsIterator", get_parts_iterator),
    ("getErrorCode", get_error_code),
    ("getErrorMessage", get_error_message),
    ("getIterator", get_iterator),
];

static RBBI_METHODS: &[MethodImpl] = &[
    ("__construct", rbbi_construct),
    ("getRules", get_rules),
    ("getRuleStatus", get_rule_status),
    ("getRuleStatusVec", get_rule_status_vec),
];

static CP_METHODS: &[MethodImpl] = &[("getLastCodePoint", get_last_code_point)];

static PARTS_METHODS: &[MethodImpl] =
    &[("getBreakIterator", parts_get_break_iterator), ("getRuleStatus", parts_get_rule_status)];

pub fn register(r: &mut Registry) {
    use rphp_runtime::NativeMethod;
    register_class(r, &generated::classes::INTLBREAKITERATOR, METHODS, |b| b.payload_clone(payload_clone));
    register_class(r, &generated::classes::INTLRULEBASEDBREAKITERATOR, RBBI_METHODS, |b| b.payload_clone(payload_clone));
    register_class(r, &generated::classes::INTLCODEPOINTBREAKITERATOR, CP_METHODS, |b| b.payload_clone(payload_clone));
    // The IntlIterator methods, over this class's own cursor.
    let m = |handler| NativeMethod { handler, min_args: 0, max_args: Some(0), params: &[], by_ref: 0, is_static: false, is_final: false };
    register_class(r, &generated::classes::INTLPARTSITERATOR, PARTS_METHODS, |b| {
        b.method("current", m(parts_current))
            .method("key", m(parts_key))
            .method("next", m(parts_next))
            .method("rewind", m(parts_rewind))
            .method("valid", m(parts_valid))
            .uncloneable()
    });
}
