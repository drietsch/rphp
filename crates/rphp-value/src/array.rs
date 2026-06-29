//! The PHP array: an insertion-ordered map keyed by `int` or byte-string.
//!
//! **Scope so far:** a single insertion-ordered representation (an entry vector
//! plus a key→position index), refcounted with copy-on-write via
//! [`Rc::make_mut`]. The target (`specs/base/03-heap-types.md` §11.2) is a
//! dual representation that auto-promotes a packed `Vec<Value>` to a SwissTable
//! `OrderedMap`; that optimization lands later behind this same API. Deletes
//! (`unset`) and the legacy internal pointer are not modelled yet.
use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;

use crate::Value;

/// A normalized array key. PHP coerces array keys to either an `int` or a byte
/// string: integer-valued strings become `int` keys (`$a["5"]` is `$a[5]`),
/// `bool`/`null`/`float` keys coerce per the rules in [`array_key`].
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum ArrayKey {
    Int(i64),
    Str(Box<[u8]>),
}

impl ArrayKey {
    /// The key as a runtime value (as seen by `foreach ($a as $k => $v)`).
    pub fn to_value(&self) -> Value {
        match self {
            ArrayKey::Int(i) => Value::Int(*i),
            ArrayKey::Str(b) => Value::string(b),
        }
    }
}

/// Normalize a value used as an array key, per PHP's coercion rules. Returns
/// `None` for an illegal offset type (array/object), which the runtime reports
/// as a warning and skips.
pub fn array_key(v: &Value) -> Option<ArrayKey> {
    Some(match v {
        Value::Int(i) => ArrayKey::Int(*i),
        Value::Bool(b) => ArrayKey::Int(*b as i64),
        Value::Null => ArrayKey::Str(Box::from(&b""[..])),
        Value::Float(_) => ArrayKey::Int(v.to_int()),
        Value::Str(s) => match canonical_int_key(s.as_bytes()) {
            Some(i) => ArrayKey::Int(i),
            None => ArrayKey::Str(Box::from(s.as_bytes())),
        },
        Value::Array(_) | Value::Closure(_) | Value::Object(_) => return None,
    })
}

/// PHP's integer-string key rule: a string is used as an `int` key iff it is a
/// canonical decimal integer — optional leading `-`, no redundant leading zeros,
/// no `-0`, fits in `i64`, and round-trips `(string)(int)$s === $s`.
fn canonical_int_key(b: &[u8]) -> Option<i64> {
    if b.is_empty() {
        return None;
    }
    let digits = if b[0] == b'-' { &b[1..] } else { b };
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    // No leading zeros (but "0" itself is fine); no "-0".
    if digits.len() > 1 && digits[0] == b'0' {
        return None;
    }
    if b[0] == b'-' && digits == b"0" {
        return None;
    }
    std::str::from_utf8(b).ok()?.parse::<i64>().ok()
}

#[derive(Clone, Default)]
struct ArrayData {
    entries: Vec<(ArrayKey, Value)>,
    index: HashMap<ArrayKey, usize>,
    /// The key a bare `$a[] =` append will use next.
    next_int: i64,
}

/// A PHP array value: refcounted, copy-on-write.
#[derive(Clone, Default)]
pub struct Array(Rc<ArrayData>);

impl Array {
    pub fn new() -> Self {
        Array::default()
    }

    pub fn len(&self) -> usize {
        self.0.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.entries.is_empty()
    }

    /// Look up by normalized key.
    pub fn get(&self, key: &ArrayKey) -> Option<&Value> {
        self.0.index.get(key).map(|&i| &self.0.entries[i].1)
    }

    /// The `(key, value)` at insertion position `pos`, for `foreach` iteration.
    pub fn entry_at(&self, pos: usize) -> Option<(&ArrayKey, &Value)> {
        self.0.entries.get(pos).map(|(k, v)| (k, v))
    }

    /// Entries in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = (&ArrayKey, &Value)> {
        self.0.entries.iter().map(|(k, v)| (k, v))
    }

    /// Insert or overwrite `key`. Triggers a COW separation if the backing is
    /// shared (refcount > 1), so PHP value semantics hold (`$b = $a; $b[0]=1;`
    /// must not touch `$a`).
    pub fn set(&mut self, key: ArrayKey, value: Value) {
        let data = Rc::make_mut(&mut self.0);
        if let Some(&i) = data.index.get(&key) {
            data.entries[i].1 = value;
            return;
        }
        if let ArrayKey::Int(k) = &key {
            data.next_int = data.next_int.max(k.saturating_add(1));
        }
        let pos = data.entries.len();
        data.index.insert(key.clone(), pos);
        data.entries.push((key, value));
    }

    /// `$a[] = value`: append under the next integer key. COW as in [`set`].
    pub fn push(&mut self, value: Value) {
        let key = ArrayKey::Int(self.0.next_int);
        self.set(key, value);
    }

    /// Union (`+`): all of `self`'s entries, plus `other`'s keys not in `self`.
    pub fn union(&self, other: &Array) -> Array {
        let mut out = self.clone();
        for (k, v) in other.iter() {
            if out.get(k).is_none() {
                out.set(k.clone(), v.clone());
            }
        }
        out
    }

    /// Loose `==`: same count and the same key⇒(loosely-equal) value pairs,
    /// order-independent.
    pub fn loose_eq(&self, other: &Array) -> bool {
        self.len() == other.len()
            && self
                .iter()
                .all(|(k, v)| other.get(k).is_some_and(|ov| v.loose_eq(ov)))
    }

    /// Strict `===`: same key/value pairs in the **same order**, identically.
    pub fn identical(&self, other: &Array) -> bool {
        self.len() == other.len()
            && self
                .iter()
                .zip(other.iter())
                .all(|((k1, v1), (k2, v2))| k1 == k2 && v1.identical(v2))
    }

    /// `<=>`: fewer elements compare less; at equal count, element-wise by
    /// `self`'s key order (a key missing on the right makes `self` greater).
    pub fn spaceship(&self, other: &Array) -> i64 {
        use std::cmp::Ordering;
        match self.len().cmp(&other.len()) {
            Ordering::Less => -1,
            Ordering::Greater => 1,
            Ordering::Equal => {
                for (k, v) in self.iter() {
                    match other.get(k) {
                        None => return 1,
                        Some(ov) => {
                            let c = v.spaceship(ov);
                            if c != 0 {
                                return c;
                            }
                        }
                    }
                }
                0
            }
        }
    }
}

impl PartialEq for Array {
    /// Structural (order-sensitive) equality, used by `Value`'s derived
    /// `PartialEq` in tests — not PHP's `==` (that is [`Array::loose_eq`]).
    fn eq(&self, other: &Self) -> bool {
        self.identical(other)
    }
}

impl fmt::Debug for Array {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn int_string_keys_are_normalized() {
        assert_eq!(array_key(&Value::string(b"5")), Some(ArrayKey::Int(5)));
        assert_eq!(array_key(&Value::Int(5)), Some(ArrayKey::Int(5)));
        // Leading zero and "-0" stay string keys.
        assert_eq!(array_key(&Value::string(b"05")), Some(ArrayKey::Str(Box::from(&b"05"[..]))));
        assert_eq!(array_key(&Value::string(b"-0")), Some(ArrayKey::Str(Box::from(&b"-0"[..]))));
        assert_eq!(array_key(&Value::string(b"-5")), Some(ArrayKey::Int(-5)));
        assert_eq!(array_key(&Value::Bool(true)), Some(ArrayKey::Int(1)));
        assert_eq!(array_key(&Value::Null), Some(ArrayKey::Str(Box::from(&b""[..]))));
    }

    #[test]
    fn set_get_and_append() {
        let mut a = Array::new();
        a.push(Value::Int(10)); // key 0
        a.push(Value::Int(20)); // key 1
        a.set(ArrayKey::Str(Box::from(&b"k"[..])), Value::Int(99));
        a.push(Value::Int(30)); // key 2 (next int unaffected by string key)
        assert_eq!(a.len(), 4);
        assert_eq!(a.get(&ArrayKey::Int(0)), Some(&Value::Int(10)));
        assert_eq!(a.get(&ArrayKey::Int(2)), Some(&Value::Int(30)));
        assert_eq!(a.get(&ArrayKey::Str(Box::from(&b"k"[..]))), Some(&Value::Int(99)));
    }

    #[test]
    fn next_int_follows_explicit_int_keys() {
        let mut a = Array::new();
        a.set(ArrayKey::Int(5), Value::Int(1));
        a.push(Value::Int(2)); // should land at key 6
        assert_eq!(a.get(&ArrayKey::Int(6)), Some(&Value::Int(2)));
    }

    #[test]
    fn copy_on_write_separates() {
        let mut a = Array::new();
        a.push(Value::Int(1));
        let b = a.clone(); // shares backing
        a.push(Value::Int(2)); // must separate, leaving b untouched
        assert_eq!(a.len(), 2);
        assert_eq!(b.len(), 1);
    }
}
