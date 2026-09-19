//! Class autoloading (plan E7).
//!
//! A class name that the class table does not know is handed to the
//! `spl_autoload_register` stack, in registration order, until one of them
//! declares it. That is the whole mechanism Composer's `vendor/autoload.php`
//! relies on.
//!
//! **Where autoload fires matters.** php triggers it for `new`, static access
//! (`A::m()`, `A::$p`, `A::C`), `extends`/`implements`/`use` at declaration
//! time, a type check against a declared class, and `class_exists($n, true)`.
//! It deliberately does **not** fire for `instanceof`, for `catch` matching,
//! or for `class_exists($n, false)` — an undeclared class simply does not
//! match, which is why a `catch (SomeMissing $e)` never loads anything.
//!
//! A loader that is already running for the same name is skipped, so a loader
//! that itself touches the class it is loading cannot recurse forever.

use rphp_value::Value;

use crate::registry::Unwind;
use crate::Interp;

impl Interp {
    /// Look `name` up, consulting the autoloader stack when it is unknown.
    ///
    /// Returns `None` when no loader declared it. Errors thrown by a loader
    /// propagate (php lets an autoloader throw).
    pub fn lookup_class(&mut self, name: &[u8]) -> Result<Option<u32>, Unwind> {
        let name = name.strip_prefix(b"\\").unwrap_or(name);
        if let Some(id) = self.class_by_name(name) {
            return Ok(Some(id));
        }
        if self.autoloaders.is_empty() {
            return Ok(None);
        }
        let key: Box<[u8]> = name.to_ascii_lowercase().into();
        if self.autoloading.contains(&key) {
            return Ok(None);
        }
        self.autoloading.push(key);
        let r = self.run_autoloaders(name);
        self.autoloading.pop();
        r?;
        Ok(self.class_by_name(name))
    }

    /// Call each registered loader with `name` until the class appears.
    fn run_autoloaders(&mut self, name: &[u8]) -> Result<(), Unwind> {
        for i in 0..self.autoloaders.len() {
            let Some(loader) = self.autoloaders.get(i).cloned() else {
                break;
            };
            self.call_value(&loader, &[Value::string(name)])?;
            if self.class_by_name(name).is_some() {
                return Ok(());
            }
        }
        Ok(())
    }

    /// `lookup_class` with php's `Class "X" not found` when nothing declares
    /// it — the form every call site that *requires* a class wants.
    pub fn lookup_class_or_error(&mut self, name: &[u8]) -> Result<u32, Unwind> {
        match self.lookup_class(name)? {
            Some(id) => Ok(id),
            None => Err(Unwind::error(format!(
                "Class \"{}\" not found",
                String::from_utf8_lossy(name)
            ))),
        }
    }
}

// ---- the php-facing surface (`spl_autoload.rs` in the stdlib) -------------

impl Interp {
    /// The registered loaders, in call order.
    pub fn autoloaders(&self) -> Vec<Value> {
        self.autoloaders.clone()
    }

    /// Whether this callable is already on the stack. php compares the
    /// *resolved* callable, so registering `'f'` twice is one entry, and so
    /// is `[$o, 'm']` built twice over the same object.
    pub fn autoloader_registered(&self, callback: &Value) -> bool {
        self.autoloaders.iter().any(|f| same_callable(f, callback))
    }

    /// Push a loader onto the stack (`$prepend` puts it first).
    pub fn autoloader_add(&mut self, callback: Value, prepend: bool) {
        if prepend {
            self.autoloaders.insert(0, callback);
        } else {
            self.autoloaders.push(callback);
        }
    }

    /// Remove a loader; `true` when one was there.
    pub fn autoloader_remove(&mut self, callback: &Value) -> bool {
        let before = self.autoloaders.len();
        self.autoloaders.retain(|f| !same_callable(f, callback));
        self.autoloaders.len() != before
    }

    /// `spl_autoload_call($name)` — run the stack unconditionally.
    pub fn autoload_call(&mut self, name: &[u8]) -> Result<(), Unwind> {
        let key: Box<[u8]> = name.to_ascii_lowercase().into();
        if self.autoloading.contains(&key) {
            return Ok(());
        }
        self.autoloading.push(key);
        let r = self.run_autoloaders(name);
        self.autoloading.pop();
        r
    }

    /// `class_alias()`: make `alias` name the same class definition.
    pub fn alias_class(&mut self, alias: &[u8], id: u32) {
        let key = alias
            .strip_prefix(b"\\")
            .unwrap_or(alias)
            .to_ascii_lowercase();
        self.class_index.insert(key.into_boxed_slice(), id);
    }
}

/// Whether two callable values name the same target, for the stack's
/// duplicate check. Strings compare case-insensitively (php function names
/// are), `[$obj, 'm']` pairs by object identity, everything else by value.
fn same_callable(a: &Value, b: &Value) -> bool {
    match (&*a.deref(), &*b.deref()) {
        (Value::Str(x), Value::Str(y)) => x.as_bytes().eq_ignore_ascii_case(y.as_bytes()),
        (Value::Array(x), Value::Array(y)) if x.len() == 2 && y.len() == 2 => {
            let key0 = rphp_value::ArrayKey::Int(0);
            let key1 = rphp_value::ArrayKey::Int(1);
            let (x0, x1) = (x.get_deref(&key0), x.get_deref(&key1));
            let (y0, y1) = (y.get_deref(&key0), y.get_deref(&key1));
            let same_target = match (&x0, &y0) {
                (Some(Value::Object(o1)), Some(Value::Object(o2))) => o1.id() == o2.id(),
                (Some(Value::Str(s1)), Some(Value::Str(s2))) => {
                    s1.as_bytes().eq_ignore_ascii_case(s2.as_bytes())
                }
                _ => false,
            };
            let same_method = match (&x1, &y1) {
                (Some(Value::Str(m1)), Some(Value::Str(m2))) => {
                    m1.as_bytes().eq_ignore_ascii_case(m2.as_bytes())
                }
                _ => false,
            };
            same_target && same_method
        }
        (Value::Closure(x), Value::Closure(y)) => std::ptr::eq(x, y),
        _ => false,
    }
}
