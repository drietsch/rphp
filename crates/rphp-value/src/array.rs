//! The PHP array: an insertion-ordered map keyed by `int` or byte-string.
//!
//! **Representation:** one insertion-ordered entry vector with **tombstones**
//! (`unset` leaves a `None` hole so raw positions held by iterators stay
//! valid), a key → raw-position index, the next append key and the legacy
//! internal pointer; refcounted with copy-on-write via [`Rc::make_mut`]. Holes
//! are compacted once they outnumber half the live entries (`pos` is remapped
//! onto the same live element). Elements may be [`Value::Ref`]s (`$r = &$a[0]`);
//! cloning the array shares those cells — PHP's semantics for `$b = $a` — so
//! readers that want the plain value use [`Array::get_deref`] or dereference
//! what [`Array::iter`] yields. The target packed/hash dual representation
//! (`specs/base/03-heap-types.md` §11.2) lands later behind this same API.
use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;

use crate::{PhpRef, Value};

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

    /// A string key from bytes (no integer normalization; use [`array_key`]
    /// for that).
    pub fn str(bytes: &[u8]) -> ArrayKey {
        ArrayKey::Str(Box::from(bytes))
    }
}

/// Normalize a value used as an array key, per PHP's coercion rules. Returns
/// `None` for an illegal offset type (array/object), which the runtime reports
/// as a warning and skips. A reference is dereferenced; a resource is its id
/// (PHP warns and casts).
pub fn array_key(v: &Value) -> Option<ArrayKey> {
    Some(match v {
        Value::Int(i) => ArrayKey::Int(*i),
        Value::Bool(b) => ArrayKey::Int(*b as i64),
        Value::Null | Value::Uninit => ArrayKey::Str(Box::from(&b""[..])),
        Value::Float(_) => ArrayKey::Int(v.to_int()),
        Value::Str(s) => match canonical_int_key(s.as_bytes()) {
            Some(i) => ArrayKey::Int(i),
            None => ArrayKey::Str(Box::from(s.as_bytes())),
        },
        Value::Resource(r) => ArrayKey::Int(i64::from(r.id())),
        Value::Ref(r) => return array_key(&r.borrow()),
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

type Entry = Option<(ArrayKey, Value)>;

#[derive(Clone, Default)]
struct ArrayData {
    /// Insertion-ordered entries; `None` is a tombstone left by `unset`.
    entries: Vec<Entry>,
    /// Number of `Some` entries.
    live: usize,
    /// key → raw position in `entries` (always a live slot).
    index: HashMap<ArrayKey, usize>,
    /// The key a bare `$a[] =` append will use next.
    next_int: i64,
    /// The internal pointer (`current()`/`next()`/…): a raw position, or
    /// `entries.len()` when past the end.
    pos: usize,
}

impl ArrayData {
    fn tombstones(&self) -> usize {
        self.entries.len() - self.live
    }

    /// First live raw position `>= from`.
    fn next_live(&self, from: usize) -> Option<usize> {
        (from..self.entries.len()).find(|&i| self.entries[i].is_some())
    }

    /// Last live raw position `< before`.
    fn prev_live(&self, before: usize) -> Option<usize> {
        (0..before.min(self.entries.len())).rev().find(|&i| self.entries[i].is_some())
    }

    /// Append a new entry (the key must not be present) and return its raw
    /// position.
    fn insert_new(&mut self, key: ArrayKey, value: Value) -> usize {
        if let ArrayKey::Int(k) = &key {
            self.next_int = self.next_int.max(k.saturating_add(1));
        }
        let raw = self.entries.len();
        self.index.insert(key.clone(), raw);
        self.entries.push(Some((key, value)));
        self.live += 1;
        raw
    }

    /// Drop the tombstones once they outnumber half the live entries.
    fn maybe_compact(&mut self) {
        if self.tombstones() > self.live / 2 {
            self.compact();
        }
    }

    fn compact(&mut self) {
        let old = std::mem::take(&mut self.entries);
        let mut entries: Vec<Entry> = Vec::with_capacity(self.live);
        self.index.clear();
        // `pos` follows the element it points at; past-the-end stays past the end.
        let mut new_pos = None;
        for (raw, e) in old.into_iter().enumerate() {
            if raw == self.pos {
                new_pos = Some(entries.len());
            }
            if let Some((k, v)) = e {
                self.index.insert(k.clone(), entries.len());
                entries.push(Some((k, v)));
            }
        }
        self.pos = new_pos.unwrap_or(entries.len());
        self.entries = entries;
    }

    fn at(&self, raw: usize) -> Option<(&ArrayKey, &Value)> {
        self.entries.get(raw)?.as_ref().map(|(k, v)| (k, v))
    }
}

/// A PHP array value: refcounted, copy-on-write.
#[derive(Clone, Default)]
pub struct Array(Rc<ArrayData>);

impl Array {
    /// The empty array.
    pub fn new() -> Self {
        Array::default()
    }

    /// Number of live elements (`count()`).
    pub fn len(&self) -> usize {
        self.0.live
    }

    /// Whether there are no live elements.
    pub fn is_empty(&self) -> bool {
        self.0.live == 0
    }

    /// The key the next `$a[] =` append will use.
    pub fn next_free_index(&self) -> i64 {
        self.0.next_int
    }

    /// Look up by normalized key, returning the element **as stored** (possibly
    /// a [`Value::Ref`]). Readers usually want [`Array::get_deref`].
    pub fn get(&self, key: &ArrayKey) -> Option<&Value> {
        self.0.index.get(key).and_then(|&i| self.0.at(i)).map(|(_, v)| v)
    }

    /// Look up by normalized key, dereferencing a reference element.
    pub fn get_deref(&self, key: &ArrayKey) -> Option<Value> {
        self.get(key).map(|v| v.deref().into_owned())
    }

    /// Mutable access to an element (COW-separating). The slot may hold a
    /// `Ref`; write with [`Value::assign`] to honour it.
    pub fn get_mut(&mut self, key: &ArrayKey) -> Option<&mut Value> {
        let data = Rc::make_mut(&mut self.0);
        let i = *data.index.get(key)?;
        data.entries[i].as_mut().map(|(_, v)| v)
    }

    /// Whether `key` is present (`array_key_exists`; a `null` element counts).
    pub fn contains_key(&self, key: &ArrayKey) -> bool {
        self.0.index.contains_key(key)
    }

    /// Insert or overwrite `key` by value: an existing reference element is
    /// written *through* (`$b = $a; $b[0] = 9` is visible via `$a[0]` iff
    /// element 0 is a reference), and `value` is dereferenced. Triggers a COW
    /// separation if the backing is shared (refcount > 1), so PHP value
    /// semantics hold (`$b = $a; $b[0]=1;` must not touch `$a`).
    pub fn set(&mut self, key: ArrayKey, value: Value) {
        let value = value.unref();
        let data = Rc::make_mut(&mut self.0);
        if let Some(&i) = data.index.get(&key) {
            if let Some((_, slot)) = data.entries[i].as_mut() {
                Value::assign(slot, value);
            }
            return;
        }
        data.insert_new(key, value);
    }

    /// Bind `key` to the reference cell `r` (`$a[k] = &$x`), replacing any
    /// previous binding rather than writing through it.
    pub fn set_ref(&mut self, key: ArrayKey, r: PhpRef) {
        let data = Rc::make_mut(&mut self.0);
        if let Some(&i) = data.index.get(&key) {
            if let Some((_, slot)) = data.entries[i].as_mut() {
                *slot = Value::Ref(r);
            }
            return;
        }
        data.insert_new(key, Value::Ref(r));
    }

    /// `$a[] = value`: append under the next integer key. COW as in [`set`].
    ///
    /// [`set`]: Array::set
    pub fn push(&mut self, value: Value) {
        let key = ArrayKey::Int(self.0.next_int);
        self.set(key, value);
    }

    /// `$a[] = &$x`: append a reference binding under the next integer key.
    pub fn push_ref(&mut self, r: PhpRef) {
        let key = ArrayKey::Int(self.0.next_int);
        self.set_ref(key, r);
    }

    /// `&$a[k]`: make element `key` a reference cell in place (creating it as
    /// `null` if absent) and return the cell. COW-separates first, so the cell
    /// is shared only by this array (and by copies taken *afterwards*).
    pub fn get_ref(&mut self, key: ArrayKey) -> PhpRef {
        let data = Rc::make_mut(&mut self.0);
        let i = match data.index.get(&key) {
            Some(&i) => i,
            None => data.insert_new(key, Value::Null),
        };
        match data.entries[i].as_mut() {
            Some((_, slot)) => Value::make_ref(slot),
            None => unreachable!("index points at a live entry"),
        }
    }

    /// `unset($a[k])`: remove the element, returning it (as stored). Leaves a
    /// tombstone so raw positions stay valid, moves the internal pointer to
    /// the next element if it pointed at the removed one, and compacts when
    /// holes outnumber half the live entries. The next append key is **not**
    /// reset (`$a = [1, 2]; unset($a[1]); $a[] = 3;` yields key 2).
    pub fn unset(&mut self, key: &ArrayKey) -> Option<Value> {
        if !self.0.index.contains_key(key) {
            return None;
        }
        let data = Rc::make_mut(&mut self.0);
        let i = data.index.remove(key)?;
        let (_, v) = data.entries[i].take()?;
        data.live -= 1;
        if data.pos == i {
            data.pos = data.next_live(i + 1).unwrap_or(data.entries.len());
        }
        data.maybe_compact();
        Some(v)
    }

    // ----- iteration -----

    /// Entries in insertion order (tombstones skipped), values as stored.
    pub fn iter(&self) -> impl Iterator<Item = (&ArrayKey, &Value)> {
        self.0.entries.iter().flatten().map(|(k, v)| (k, v))
    }

    /// Keys in insertion order.
    pub fn keys(&self) -> impl Iterator<Item = &ArrayKey> {
        self.iter().map(|(k, _)| k)
    }

    /// Values (as stored) in insertion order.
    pub fn values(&self) -> impl Iterator<Item = &Value> {
        self.iter().map(|(_, v)| v)
    }

    /// The first live entry.
    pub fn first(&self) -> Option<(&ArrayKey, &Value)> {
        self.0.next_live(0).and_then(|i| self.0.at(i))
    }

    /// The last live entry.
    pub fn last(&self) -> Option<(&ArrayKey, &Value)> {
        self.0.prev_live(self.0.entries.len()).and_then(|i| self.0.at(i))
    }

    /// Length of the raw entry vector (live entries plus tombstones): the
    /// exclusive upper bound of raw positions for [`Array::next_live_from`].
    pub fn raw_len(&self) -> usize {
        self.0.entries.len()
    }

    /// The entry at raw position `raw`, or `None` for a tombstone / out of range.
    pub fn raw_entry(&self, raw: usize) -> Option<(&ArrayKey, &Value)> {
        self.0.at(raw)
    }

    /// The `foreach` cursor: the first live entry at raw position `>= raw_pos`,
    /// with its raw position (the runtime stores `raw + 1` as the next cursor).
    /// Positions are stable across `unset` on *this* backing until a
    /// compaction; the runtime's `foreach` iterates a snapshot (a clone shares
    /// the backing, and any write separates it), so its cursor never observes
    /// one. Iterating a live array by reference (plan E3) must register its
    /// cursor so compaction can remap it — deferred with that op.
    pub fn next_live_from(&self, raw_pos: usize) -> Option<(usize, &ArrayKey, &Value)> {
        let i = self.0.next_live(raw_pos)?;
        self.0.at(i).map(|(k, v)| (i, k, v))
    }

    // ----- internal pointer (`current`/`key`/`next`/`prev`/`reset`/`end`) -----

    /// `current()`/`key()`: the entry under the internal pointer, `None` when it
    /// is past either end.
    pub fn pos_current(&self) -> Option<(&ArrayKey, &Value)> {
        self.0.at(self.0.pos)
    }

    /// `next()`: advance to the following live entry and return it.
    pub fn pos_advance(&mut self) -> Option<(&ArrayKey, &Value)> {
        let data = Rc::make_mut(&mut self.0);
        if data.pos < data.entries.len() {
            data.pos = data.next_live(data.pos + 1).unwrap_or(data.entries.len());
        }
        data.at(data.pos)
    }

    /// `prev()`: step back to the preceding live entry and return it. Stepping
    /// before the first element invalidates the pointer (as in PHP: `current()`
    /// is then `false` until `reset()`/`end()`).
    pub fn pos_prev(&mut self) -> Option<(&ArrayKey, &Value)> {
        let data = Rc::make_mut(&mut self.0);
        data.pos = data.prev_live(data.pos).unwrap_or(data.entries.len());
        data.at(data.pos)
    }

    /// `reset()`: move to the first element and return it.
    pub fn pos_rewind(&mut self) -> Option<(&ArrayKey, &Value)> {
        let data = Rc::make_mut(&mut self.0);
        data.pos = data.next_live(0).unwrap_or(data.entries.len());
        data.at(data.pos)
    }

    /// `end()`: move to the last element and return it.
    pub fn pos_end(&mut self) -> Option<(&ArrayKey, &Value)> {
        let data = Rc::make_mut(&mut self.0);
        data.pos = data.prev_live(data.entries.len()).unwrap_or(data.entries.len());
        data.at(data.pos)
    }

    // ----- shape and comparison -----

    /// `array_is_list`: keys are exactly `0, 1, …, n-1` in order.
    pub fn is_list(&self) -> bool {
        self.keys()
            .enumerate()
            .all(|(i, k)| matches!(k, ArrayKey::Int(n) if *n == i as i64))
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

    /// Strict `===`: same key/value pairs in the **same order**, identically
    /// (reference elements compare their contents, as PHP does).
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
    /// Structural (order-sensitive) equality, used by `Value`'s `PartialEq` in
    /// tests — not PHP's `==` (that is [`Array::loose_eq`]).
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

    fn k(s: &[u8]) -> ArrayKey {
        ArrayKey::str(s)
    }

    fn list(items: &[i64]) -> Array {
        let mut a = Array::new();
        for &i in items {
            a.push(Value::Int(i));
        }
        a
    }

    fn keys_of(a: &Array) -> Vec<ArrayKey> {
        a.keys().cloned().collect()
    }

    #[test]
    fn int_string_keys_are_normalized() {
        assert_eq!(array_key(&Value::string(b"5")), Some(ArrayKey::Int(5)));
        assert_eq!(array_key(&Value::Int(5)), Some(ArrayKey::Int(5)));
        // Leading zero and "-0" stay string keys.
        assert_eq!(array_key(&Value::string(b"05")), Some(k(b"05")));
        assert_eq!(array_key(&Value::string(b"-0")), Some(k(b"-0")));
        assert_eq!(array_key(&Value::string(b"-5")), Some(ArrayKey::Int(-5)));
        assert_eq!(array_key(&Value::Bool(true)), Some(ArrayKey::Int(1)));
        assert_eq!(array_key(&Value::Null), Some(k(b"")));
        // A reference key is dereferenced; a resource key is its id.
        let mut slot = Value::string(b"7");
        let r = Value::make_ref(&mut slot);
        assert_eq!(array_key(&Value::Ref(r)), Some(ArrayKey::Int(7)));
        let res = crate::Resource::new(3, "stream", Box::new(()));
        assert_eq!(array_key(&Value::Resource(res)), Some(ArrayKey::Int(3)));
        assert_eq!(array_key(&Value::empty_array()), None);
    }

    #[test]
    fn set_get_and_append() {
        let mut a = Array::new();
        a.push(Value::Int(10)); // key 0
        a.push(Value::Int(20)); // key 1
        a.set(k(b"k"), Value::Int(99));
        a.push(Value::Int(30)); // key 2 (next int unaffected by string key)
        assert_eq!(a.len(), 4);
        assert_eq!(a.get(&ArrayKey::Int(0)), Some(&Value::Int(10)));
        assert_eq!(a.get(&ArrayKey::Int(2)), Some(&Value::Int(30)));
        assert_eq!(a.get(&k(b"k")), Some(&Value::Int(99)));
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

    #[test]
    fn unset_keeps_order_positions_and_next_index() {
        // $a = [10, 20, 30]; unset($a[1]); $a[] = 40;
        let mut a = list(&[10, 20, 30]);
        assert_eq!(a.unset(&ArrayKey::Int(1)), Some(Value::Int(20)));
        assert_eq!(a.unset(&ArrayKey::Int(1)), None);
        assert_eq!(a.len(), 2);
        assert!(!a.contains_key(&ArrayKey::Int(1)));
        a.push(Value::Int(40)); // key 3, not 2
        assert_eq!(keys_of(&a), vec![ArrayKey::Int(0), ArrayKey::Int(2), ArrayKey::Int(3)]);
        assert_eq!(a.next_free_index(), 4);
        assert!(!a.is_list());
        // Re-adding key 1 appends at the end (PHP order).
        a.set(ArrayKey::Int(1), Value::Int(21));
        let vals: Vec<Value> = a.values().cloned().collect();
        assert_eq!(vals, vec![Value::Int(10), Value::Int(30), Value::Int(40), Value::Int(21)]);
        // The foreach cursor skips the tombstone.
        let mut seen = Vec::new();
        let mut raw = 0;
        while let Some((r, key, _)) = a.next_live_from(raw) {
            seen.push(key.clone());
            raw = r + 1;
        }
        assert_eq!(seen, keys_of(&a));
    }

    #[test]
    fn compaction_preserves_lookup_order_and_pointer() {
        let mut a = list(&[0, 1, 2, 3, 4, 5, 6, 7]);
        a.pos_advance();
        a.pos_advance();
        a.pos_advance(); // at key 3
        for i in [0, 1, 2, 4, 5] {
            a.unset(&ArrayKey::Int(i));
        }
        // 5 holes vs 3 live => compacted.
        assert_eq!(a.raw_len(), 3);
        assert_eq!(a.len(), 3);
        assert_eq!(keys_of(&a), vec![ArrayKey::Int(3), ArrayKey::Int(6), ArrayKey::Int(7)]);
        for i in [3, 6, 7] {
            assert_eq!(a.get(&ArrayKey::Int(i)), Some(&Value::Int(i)));
        }
        // The internal pointer still points at key 3.
        assert_eq!(a.pos_current().map(|(k, _)| k.clone()), Some(ArrayKey::Int(3)));
        assert_eq!(a.pos_advance().map(|(k, _)| k.clone()), Some(ArrayKey::Int(6)));
    }

    #[test]
    fn unset_current_moves_pointer_forward_and_unset_at_end_invalidates() {
        let mut a = list(&[1, 2, 3]);
        a.pos_advance(); // key 1
        a.unset(&ArrayKey::Int(1));
        assert_eq!(a.pos_current().map(|(_, v)| v.clone()), Some(Value::Int(3)));
        a.unset(&ArrayKey::Int(2));
        assert!(a.pos_current().is_none());
        assert_eq!(a.pos_rewind().map(|(_, v)| v.clone()), Some(Value::Int(1)));
    }

    #[test]
    fn element_refs_are_shared_by_copies_but_plain_elements_are_not() {
        // php -r '$a=[1,2]; $r=&$a[0]; $b=$a; $b[0]=9; echo $a[0];'  => 9
        let mut a = list(&[1, 2]);
        let r = a.get_ref(ArrayKey::Int(0));
        assert!(a.get(&ArrayKey::Int(0)).unwrap().is_ref());
        let mut b = a.clone();
        b.set(ArrayKey::Int(0), Value::Int(9));
        assert_eq!(a.get_deref(&ArrayKey::Int(0)), Some(Value::Int(9)));
        assert_eq!(r.get(), Value::Int(9));
        // ... and the write also went through the variable bound to the cell.
        r.set(Value::Int(11));
        assert_eq!(b.get_deref(&ArrayKey::Int(0)), Some(Value::Int(11)));
        // Element 1 is a plain value: the copy separated it.
        b.set(ArrayKey::Int(1), Value::Int(7));
        assert_eq!(a.get_deref(&ArrayKey::Int(1)), Some(Value::Int(2)));
        // php -r '$a=[1,2]; $b=$a; $b[0]=9; echo $a[0];'  => 1
        let a2 = list(&[1, 2]);
        let mut b2 = a2.clone();
        b2.set(ArrayKey::Int(0), Value::Int(9));
        assert_eq!(a2.get_deref(&ArrayKey::Int(0)), Some(Value::Int(1)));
        // get_ref on a missing key creates a null element; `set` of a Ref value
        // stores the contents, not a binding; `set_ref` binds.
        let mut c = Array::new();
        let rc = c.get_ref(k(b"x"));
        assert_eq!(rc.get(), Value::Null);
        assert_eq!(c.len(), 1);
        c.set(k(b"y"), Value::Ref(rc.clone()));
        assert!(!c.get(&k(b"y")).unwrap().is_ref());
        c.set_ref(k(b"z"), rc.clone());
        assert!(c.get(&k(b"z")).unwrap().is_ref());
        c.push_ref(rc.clone());
        assert_eq!(rc.strong_count(), 4); // rc, x, z, [0]
    }

    #[test]
    fn get_ref_separates_a_shared_backing_first() {
        // $a = [1]; $b = $a; $r = &$b[0]; $r = 5; echo $a[0];  => 1
        let a = list(&[1]);
        let mut b = a.clone();
        let r = b.get_ref(ArrayKey::Int(0));
        r.set(Value::Int(5));
        assert_eq!(a.get_deref(&ArrayKey::Int(0)), Some(Value::Int(1)));
        assert_eq!(b.get_deref(&ArrayKey::Int(0)), Some(Value::Int(5)));
    }

    #[test]
    fn internal_pointer_walks_like_php() {
        let mut a = list(&[1, 2, 3]);
        let cur = |a: &Array| a.pos_current().map(|(_, v)| v.to_int());
        assert_eq!(cur(&a), Some(1));
        assert_eq!(a.pos_advance().map(|(_, v)| v.to_int()), Some(2));
        assert_eq!(a.pos_advance().map(|(_, v)| v.to_int()), Some(3));
        assert!(a.pos_advance().is_none());
        assert!(a.pos_advance().is_none()); // stays past the end
        assert_eq!(a.pos_end().map(|(_, v)| v.to_int()), Some(3));
        assert_eq!(a.pos_prev().map(|(_, v)| v.to_int()), Some(2));
        assert_eq!(a.pos_prev().map(|(_, v)| v.to_int()), Some(1));
        assert!(a.pos_prev().is_none()); // before the start => invalid
        assert!(a.pos_advance().is_none()); // PHP: next() does not recover
        assert_eq!(a.pos_rewind().map(|(_, v)| v.to_int()), Some(1));
        // The pointer is part of the value: a copy carries it, moving one copy
        // does not move the other.
        a.pos_advance();
        let mut b = a.clone();
        assert_eq!(cur(&b), Some(2));
        b.pos_advance();
        assert_eq!(cur(&a), Some(2));
        assert_eq!(cur(&b), Some(3));
        assert!(Array::new().pos_current().is_none());
        assert!(Array::new().pos_rewind().is_none());
    }

    #[test]
    fn is_list_first_last_and_iterators() {
        assert!(Array::new().is_list());
        assert!(list(&[5, 6]).is_list());
        let mut a = list(&[5, 6]);
        a.set(k(b"x"), Value::Int(1));
        assert!(!a.is_list());
        assert_eq!(a.first().map(|(_, v)| v.to_int()), Some(5));
        assert_eq!(a.last().map(|(k, _)| k.clone()), Some(k(b"x")));
        assert_eq!(a.values().count(), 3);
        assert_eq!(a.keys().count(), 3);
        assert!(Array::new().first().is_none());
        assert!(Array::new().last().is_none());
    }
}
