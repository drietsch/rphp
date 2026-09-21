//! The `PDO` class (and `Pdo\Sqlite`, `PDOException`, `PDORow`): the
//! connection's state, its attributes, php's three error modes, and the
//! registration of everything the extension declares.

use rphp_runtime::{nm, Ctx, NativeMethod, NativeResult, Registry, Unwind, Visibility};
use rphp_value::{Array, ArrayKey, Object, Payload, Value};

use crate::driver::{DbError, Driver, ParamKey, SqlValue};
use crate::sqlite::Sqlite;
use crate::sqlstate;

// ---- constants -----------------------------------------------------------------

pub const PARAM_NULL: i64 = 0;
pub const PARAM_INT: i64 = 1;
pub const PARAM_STR: i64 = 2;
pub const PARAM_LOB: i64 = 3;
pub const PARAM_STMT: i64 = 4;
pub const PARAM_BOOL: i64 = 5;
pub const PARAM_INPUT_OUTPUT: i64 = 2_147_483_648;

pub const FETCH_DEFAULT: i64 = 0;
pub const FETCH_LAZY: i64 = 1;
pub const FETCH_ASSOC: i64 = 2;
pub const FETCH_NUM: i64 = 3;
pub const FETCH_BOTH: i64 = 4;
pub const FETCH_OBJ: i64 = 5;
pub const FETCH_BOUND: i64 = 6;
pub const FETCH_COLUMN: i64 = 7;
pub const FETCH_CLASS: i64 = 8;
pub const FETCH_INTO: i64 = 9;
pub const FETCH_FUNC: i64 = 10;
pub const FETCH_NAMED: i64 = 11;
pub const FETCH_KEY_PAIR: i64 = 12;
pub const FETCH_GROUP: i64 = 32;
pub const FETCH_UNIQUE: i64 = 64;
pub const FETCH_CLASSTYPE: i64 = 128;
pub const FETCH_PROPS_LATE: i64 = 256;
pub const FETCH_SERIALIZE: i64 = 512;
/// The mode bits without the flags.
pub const FETCH_MODE_MASK: i64 = 0x1f;

pub const ATTR_AUTOCOMMIT: i64 = 0;
pub const ATTR_PREFETCH: i64 = 1;
pub const ATTR_TIMEOUT: i64 = 2;
pub const ATTR_ERRMODE: i64 = 3;
pub const ATTR_SERVER_VERSION: i64 = 4;
pub const ATTR_CLIENT_VERSION: i64 = 5;
pub const ATTR_SERVER_INFO: i64 = 6;
pub const ATTR_CONNECTION_STATUS: i64 = 7;
pub const ATTR_CASE: i64 = 8;
pub const ATTR_CURSOR_NAME: i64 = 9;
pub const ATTR_CURSOR: i64 = 10;
pub const ATTR_ORACLE_NULLS: i64 = 11;
pub const ATTR_PERSISTENT: i64 = 12;
pub const ATTR_STATEMENT_CLASS: i64 = 13;
pub const ATTR_FETCH_TABLE_NAMES: i64 = 14;
pub const ATTR_FETCH_CATALOG_NAMES: i64 = 15;
pub const ATTR_DRIVER_NAME: i64 = 16;
pub const ATTR_STRINGIFY_FETCHES: i64 = 17;
pub const ATTR_MAX_COLUMN_LEN: i64 = 18;
pub const ATTR_DEFAULT_FETCH_MODE: i64 = 19;
pub const ATTR_EMULATE_PREPARES: i64 = 20;
pub const ATTR_DEFAULT_STR_PARAM: i64 = 21;

pub const ERRMODE_SILENT: i64 = 0;
pub const ERRMODE_WARNING: i64 = 1;
pub const ERRMODE_EXCEPTION: i64 = 2;

pub const CASE_NATURAL: i64 = 0;
pub const CASE_UPPER: i64 = 1;
pub const CASE_LOWER: i64 = 2;

pub const NULL_NATURAL: i64 = 0;
pub const NULL_EMPTY_STRING: i64 = 1;
pub const NULL_TO_STRING: i64 = 2;

/// `Pdo\Sqlite::ATTR_OPEN_FLAGS`.
pub const SQLITE_ATTR_OPEN_FLAGS: i64 = 1000;

// ---- state ---------------------------------------------------------------------

/// The last error, as `errorCode()` / `errorInfo()` report it.
#[derive(Clone, Debug)]
pub struct ErrorState {
    pub sqlstate: String,
    pub code: Option<i64>,
    pub message: Option<String>,
}

impl Default for ErrorState {
    fn default() -> Self {
        ErrorState {
            sqlstate: "00000".to_string(),
            code: None,
            message: None,
        }
    }
}

impl ErrorState {
    pub fn info(&self) -> Value {
        let mut a = Array::new();
        a.push(Value::string(self.sqlstate.as_bytes()));
        match (&self.code, &self.message) {
            (Some(c), Some(m)) => {
                a.push(Value::Int(*c));
                a.push(Value::string(m.as_bytes()));
            }
            _ => {
                a.push(Value::Null);
                a.push(Value::Null);
            }
        }
        Value::Array(a)
    }
}

/// The fetch mode a statement is in: the mode bits and their arguments
/// (a column index, a class name with constructor arguments, an object).
#[derive(Clone, Debug)]
pub struct FetchSpec {
    pub mode: i64,
    pub args: Vec<Value>,
}

impl FetchSpec {
    pub fn both() -> FetchSpec {
        FetchSpec {
            mode: FETCH_BOTH,
            args: Vec::new(),
        }
    }
}

/// The connection's attributes.
#[derive(Clone, Debug)]
pub struct Attrs {
    pub errmode: i64,
    pub case: i64,
    pub oracle_nulls: i64,
    pub stringify: bool,
    pub default_fetch: FetchSpec,
    pub statement_class: Option<(Box<[u8]>, Vec<Value>)>,
    pub timeout: i64,
}

impl Default for Attrs {
    fn default() -> Self {
        Attrs {
            errmode: ERRMODE_EXCEPTION,
            case: CASE_NATURAL,
            oracle_nulls: NULL_NATURAL,
            stringify: false,
            default_fetch: FetchSpec::both(),
            statement_class: None,
            timeout: 0,
        }
    }
}

/// A `PDO` instance's payload.
pub struct PdoState {
    pub conn: Box<dyn Driver>,
    pub attrs: Attrs,
    pub error: ErrorState,
    /// `beginTransaction()` was called and neither `commit()` nor
    /// `rollBack()` since.
    pub in_tx: bool,
}

/// Run `f` on a `PDO` object's state, or php's error for an object whose
/// constructor never ran.
pub fn with_pdo<R>(o: &Object, f: impl FnOnce(&mut PdoState) -> R) -> Result<R, Unwind> {
    o.with_payload::<PdoState, _>(f)
        .ok_or_else(|| Unwind::error("PDO object is not initialized, constructor was not called"))
}

/// The receiver of an instance method.
pub fn this(o: Option<&Object>) -> Result<&Object, Unwind> {
    o.ok_or_else(|| Unwind::error("Non-static method called statically"))
}

pub fn static_method(min: u8, max: Option<u8>, f: rphp_runtime::NativeMethodHandler) -> NativeMethod {
    NativeMethod {
        handler: f,
        min_args: min,
        max_args: max,
        params: &[],
        by_ref: 0,
        is_static: true,
        is_final: false,
    }
}

pub fn method_ref(min: u8, max: Option<u8>, by_ref: u32, f: rphp_runtime::NativeMethodHandler) -> NativeMethod {
    NativeMethod {
        handler: f,
        min_args: min,
        max_args: max,
        params: &[],
        by_ref,
        is_static: false,
        is_final: false,
    }
}

// ---- errors --------------------------------------------------------------------

/// php's `PDOException` for `err`, thrown by `who` (`PDO::query`): the
/// message `SQLSTATE[..]: <description>: <code> <message>`, the string
/// code, and `$errorInfo`.
pub fn exception(ctx: &mut Ctx, err: &DbError) -> Unwind {
    let message = error_text(err);
    let Some(cid) = ctx.class_by_name(b"PDOException") else {
        return Unwind::error(message);
    };
    let o = ctx.create_throwable(cid, &message, 0, None, None);
    if err.sqlstate.is_empty() {
        o.set(b"code", Value::Int(err.code));
    } else {
        o.set(b"code", Value::string(err.sqlstate.as_bytes()));
        let mut info = Array::new();
        info.push(Value::string(err.sqlstate.as_bytes()));
        info.push(Value::Int(err.code));
        info.push(Value::string(err.message.as_bytes()));
        o.set(b"errorInfo", Value::Array(info));
    }
    Unwind::Throw(o)
}

/// `SQLSTATE[..]: <description>: <code> <message>` — or, for an error PDO
/// itself raises (`IM001`, `HY093`: `code` 0 and no driver behind it),
/// `SQLSTATE[..]: <description>: <message>`, as `pdo_raise_impl_error`
/// spells it.
pub fn error_text(err: &DbError) -> String {
    if err.sqlstate.is_empty() {
        err.message.clone()
    } else if err.code == 0 {
        format!("SQLSTATE[{}]: {}: {}", err.sqlstate, sqlstate::describe(&err.sqlstate), err.message)
    } else {
        format!(
            "SQLSTATE[{}]: {}: {} {}",
            err.sqlstate,
            sqlstate::describe(&err.sqlstate),
            err.code,
            err.message
        )
    }
}

/// A `PDOException` without SQLSTATE: the messages php raises with
/// `zend_throw_exception` (an integer code, no `errorInfo`).
pub fn plain_exception(ctx: &mut Ctx, message: &str, code: i64) -> Unwind {
    let Some(cid) = ctx.class_by_name(b"PDOException") else {
        return Unwind::error(message);
    };
    let o = ctx.create_throwable(cid, message, code, None, None);
    Unwind::Throw(o)
}

/// Record `err` on the connection (and the statement, when there is one)
/// and act on the error mode: an exception, a warning naming `who`, or
/// silence. Answers `Ok(())` when the caller should go on to return
/// `false`.
pub fn report(ctx: &mut Ctx, pdo: &Object, stmt: Option<&Object>, who: &str, err: &DbError) -> Result<(), Unwind> {
    let state = ErrorState {
        sqlstate: err.sqlstate.clone(),
        code: Some(err.code),
        message: Some(err.message.clone()),
    };
    // php records the error on the statement when there is one, else on
    // the connection — never on both.
    let errmode = with_pdo(pdo, |st| {
        if stmt.is_none() {
            st.error = state.clone();
        }
        st.attrs.errmode
    })?;
    if let Some(s) = stmt {
        crate::stmt::with_stmt(s, |st| st.error = state.clone())?;
    }
    match errmode {
        ERRMODE_EXCEPTION => Err(exception(ctx, err)),
        ERRMODE_WARNING => {
            let text = format!("{who}(): {}", error_text(err));
            ctx.warn(&text)?;
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Clear the last error before an operation (php resets it to `00000`).
pub fn clear_error(pdo: &Object, stmt: Option<&Object>) -> Result<(), Unwind> {
    with_pdo(pdo, |st| st.error = ErrorState::default())?;
    if let Some(s) = stmt {
        crate::stmt::with_stmt(s, |st| st.error = ErrorState::default())?;
    }
    Ok(())
}

// ---- arguments -----------------------------------------------------------------

pub fn str_arg(args: &[Value], i: usize) -> Vec<u8> {
    args.get(i).map(|v| v.to_php_bytes()).unwrap_or_default()
}

pub fn int_arg(args: &[Value], i: usize, default: i64) -> i64 {
    args.get(i)
        .map(|v| v.deref().into_owned())
        .filter(|v| !matches!(v, Value::Null))
        .map_or(default, |v| v.to_int())
}

/// A php value as a statement parameter of `ty` (`PDO::PARAM_*`): the
/// conversions php's drivers apply on binding.
pub fn param_value(v: &Value, ty: i64) -> SqlValue {
    let v = v.deref().into_owned();
    match ty & !PARAM_INPUT_OUTPUT {
        PARAM_NULL => SqlValue::Null,
        PARAM_INT => match v {
            Value::Null => SqlValue::Null,
            other => SqlValue::Int(other.to_int()),
        },
        PARAM_BOOL => match v {
            Value::Null => SqlValue::Null,
            other => SqlValue::Int(i64::from(other.to_bool())),
        },
        PARAM_LOB => match v {
            Value::Null => SqlValue::Null,
            other => SqlValue::Blob(other.to_php_bytes()),
        },
        _ => match v {
            Value::Null => SqlValue::Null,
            other => SqlValue::Text(other.to_php_bytes()),
        },
    }
}

// ---- PDO methods ---------------------------------------------------------------

/// `PDO::__construct(string $dsn, ?string $username = null, ?string $password = null, ?array $options = null)`
fn pdo_construct(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let obj = this(o)?;
    let dsn = String::from_utf8_lossy(&str_arg(args, 0)).into_owned();
    let options = match args.get(3).map(|v| v.deref().into_owned()) {
        Some(Value::Array(a)) => a,
        _ => Array::new(),
    };
    let class = ctx.class(obj.class_id()).name_str().to_string();
    let Some((driver, rest)) = dsn.split_once(':') else {
        return Err(plain_exception(
            ctx,
            &format!("{class}::__construct(): Argument #1 ($dsn) must be a valid data source name"),
            0,
        ));
    };
    let conn: Box<dyn Driver> = match driver {
        "sqlite" => {
            let flags = options.get_deref(&ArrayKey::Int(SQLITE_ATTR_OPEN_FLAGS)).map(|v| v.to_int());
            match Sqlite::open(rest, flags) {
                Ok(s) => Box::new(s),
                Err(e) => {
                    // A connection failure has its own message shape.
                    let message = format!("SQLSTATE[{}] [{}] {}", e.sqlstate, e.code, e.message);
                    return Err(plain_exception(ctx, &message, e.code));
                }
            }
        }
        scheme => {
            // A scheme the host answers (`host.rs`): its factory opens the
            // connection, or refuses with php's connection-failure shape.
            let username = args.get(1).map(|v| v.deref().into_owned()).filter(|v| !matches!(v, Value::Null)).map(|v| String::from_utf8_lossy(&v.to_php_bytes()).into_owned());
            let password = args.get(2).map(|v| v.deref().into_owned()).filter(|v| !matches!(v, Value::Null)).map(|v| String::from_utf8_lossy(&v.to_php_bytes()).into_owned());
            let dsn = crate::host::HostDsn { scheme, rest, username: username.as_deref(), password: password.as_deref(), options: &options };
            match crate::host::host_drivers(ctx).and_then(|h| h.open(&dsn)) {
                Some(Ok(conn)) => conn,
                Some(Err(e)) => {
                    let message = format!("SQLSTATE[{}] [{}] {}", e.sqlstate, e.code, e.message);
                    return Err(plain_exception(ctx, &message, e.code));
                }
                None => return Err(plain_exception(ctx, "could not find driver", 0)),
            }
        }
    };
    let mut state = PdoState {
        conn,
        attrs: Attrs::default(),
        error: ErrorState::default(),
        in_tx: false,
    };
    // Options are attributes set before anything else runs.
    let mut pending: Vec<(i64, Value)> = Vec::new();
    for (k, v) in options.iter() {
        if let ArrayKey::Int(attr) = k {
            if *attr == SQLITE_ATTR_OPEN_FLAGS {
                continue;
            }
            pending.push((*attr, v.deref().into_owned()));
        }
    }
    for (attr, v) in &pending {
        apply_attribute(ctx, &mut state, *attr, v)?;
    }
    obj.set_payload(Payload::Native(Box::new(state)));
    Ok(Value::Null)
}

/// `PDO::connect(...)`: php 8.4's factory, answering the driver's own
/// subclass (`Pdo\Sqlite`) — or the class it was called on, when that is
/// already one.
fn pdo_connect(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let dsn = String::from_utf8_lossy(&str_arg(args, 0)).into_owned();
    let driver = dsn.split_once(':').map(|(d, _)| d).unwrap_or("");
    let class: &[u8] = crate::host::subclass_for(driver);
    let cid = ctx.class_by_name(class).unwrap_or_else(|| ctx.class_by_name(b"PDO").expect("PDO is registered"));
    let obj = ctx.instantiate(cid);
    let mut ctor_args: Vec<Value> = args.iter().map(|v| v.deref().into_owned()).collect();
    pdo_construct(ctx, Some(&obj), &mut ctor_args)?;
    Ok(Value::Object(obj))
}

/// `PDO::getAvailableDrivers(): array`
fn pdo_get_available_drivers(ctx: &mut Ctx, _: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let mut a = Array::new();
    a.push(Value::string(b"sqlite"));
    if let Some(h) = crate::host::host_drivers(ctx) {
        for scheme in h.schemes() {
            if scheme != "sqlite" {
                a.push(Value::string(scheme.as_bytes()));
            }
        }
    }
    Ok(Value::Array(a))
}

/// Apply one `PDO::ATTR_*` to the connection state.
fn apply_attribute(ctx: &mut Ctx, st: &mut PdoState, attr: i64, v: &Value) -> Result<bool, Unwind> {
    match attr {
        ATTR_ERRMODE => {
            let m = v.to_int();
            if !(ERRMODE_SILENT..=ERRMODE_EXCEPTION).contains(&m) {
                return Err(Unwind::value_error(
                    "PDO::setAttribute(): Argument #2 ($value) Error mode must be one of the PDO::ERRMODE_* constants",
                ));
            }
            st.attrs.errmode = m;
        }
        ATTR_CASE => {
            let m = v.to_int();
            if !(CASE_NATURAL..=CASE_LOWER).contains(&m) {
                return Err(Unwind::value_error(
                    "PDO::setAttribute(): Argument #2 ($value) Case folding mode must be one of the PDO::CASE_* constants",
                ));
            }
            st.attrs.case = m;
        }
        ATTR_ORACLE_NULLS => st.attrs.oracle_nulls = v.to_int(),
        ATTR_STRINGIFY_FETCHES => st.attrs.stringify = v.to_bool(),
        ATTR_DEFAULT_FETCH_MODE => {
            st.attrs.default_fetch = FetchSpec {
                mode: v.to_int(),
                args: Vec::new(),
            };
        }
        ATTR_STATEMENT_CLASS => {
            let Value::Array(a) = v.deref().into_owned() else {
                return Err(Unwind::type_error(format!(
                    "PDO::setAttribute(): Argument #2 ($value) PDO::ATTR_STATEMENT_CLASS value must be of type array, {} given",
                    v.type_name()
                )));
            };
            let name = a.get_deref(&ArrayKey::Int(0)).map(|v| v.to_php_bytes()).unwrap_or_default();
            let Some(cid) = ctx.lookup_class(&name)? else {
                return Err(Unwind::type_error(
                    "PDO::setAttribute(): Argument #2 ($value) PDO::ATTR_STATEMENT_CLASS class must be a valid class",
                ));
            };
            let base = ctx.class_by_name(b"PDOStatement").expect("registered");
            if !ctx.is_subclass_or_eq(cid, base) {
                return Err(Unwind::type_error(
                    "PDO::setAttribute(): Argument #2 ($value) PDO::ATTR_STATEMENT_CLASS class must be derived from PDOStatement",
                ));
            }
            let ctor_args = match a.get_deref(&ArrayKey::Int(1)) {
                Some(Value::Array(args)) => args.iter().map(|(_, v)| v.deref().into_owned()).collect(),
                _ => Vec::new(),
            };
            st.attrs.statement_class = Some((name.into_boxed_slice(), ctor_args));
        }
        ATTR_TIMEOUT => {
            st.attrs.timeout = v.to_int();
            st.conn.set_attribute(ATTR_TIMEOUT, v).map_err(|e| exception(ctx, &e))?;
        }
        // Everything else is the driver's; what it does not know is a
        // plain `false`, as php answers.
        other => {
            return st.conn.set_attribute(other, v).map_err(|e| exception(ctx, &e));
        }
    }
    Ok(true)
}

/// `PDO::setAttribute(int $attribute, mixed $value): bool`
fn pdo_set_attribute(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let obj = this(o)?;
    let attr = int_arg(args, 0, 0);
    let v = args.get(1).map(|v| v.deref().into_owned()).unwrap_or(Value::Null);
    // The state leaves the payload for the call so the driver can throw.
    let mut st = take_state(obj)?;
    let r = apply_attribute(ctx, &mut st, attr, &v);
    obj.set_payload(Payload::Native(Box::new(st)));
    Ok(Value::Bool(r?))
}

/// Move the state out of the payload (so a driver call can re-enter the
/// interpreter without the payload borrowed).
pub fn take_state(obj: &Object) -> Result<PdoState, Unwind> {
    let mut slot: Option<PdoState> = None;
    obj.with_payload::<PdoState, _>(|st| {
        let taken = std::mem::replace(
            st,
            PdoState {
                conn: Box::new(Detached),
                attrs: Attrs::default(),
                error: ErrorState::default(),
                in_tx: false,
            },
        );
        slot = Some(taken);
    });
    slot.ok_or_else(|| Unwind::error("PDO object is not initialized, constructor was not called"))
}

/// The driver standing in while the real one is out of the payload: any
/// use is a re-entrant call from a callback, which php refuses too.
struct Detached;

impl Driver for Detached {
    fn name(&self) -> &'static str {
        "sqlite"
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
    fn exec(&mut self, _: &str) -> Result<i64, DbError> {
        Err(busy())
    }
    fn prepare(&mut self, _: &str) -> Result<Vec<Option<String>>, DbError> {
        Err(busy())
    }
    fn run(&mut self, _: &str, _: &[(ParamKey, SqlValue)]) -> Result<crate::driver::ResultSet, DbError> {
        Err(busy())
    }
    fn last_insert_id(&mut self, _: Option<&str>) -> Result<String, DbError> {
        Err(busy())
    }
    fn begin(&mut self) -> Result<(), DbError> {
        Err(busy())
    }
    fn commit(&mut self) -> Result<(), DbError> {
        Err(busy())
    }
    fn rollback(&mut self) -> Result<(), DbError> {
        Err(busy())
    }
    fn in_transaction(&self) -> bool {
        false
    }
    fn quote(&self, _: &[u8]) -> Result<String, DbError> {
        Err(busy())
    }
    fn server_version(&self) -> String {
        String::new()
    }
    fn get_attribute(&self, _: i64) -> Option<Value> {
        None
    }
    fn set_attribute(&mut self, _: i64, _: &Value) -> Result<bool, DbError> {
        Err(busy())
    }
}

fn busy() -> DbError {
    DbError::new("HY000", 0, "The connection is in use by a callback")
}

/// Run `f` against the connection with the interpreter available to
/// SQLite's callbacks, then put the state back.
pub fn with_conn<R>(ctx: &mut Ctx, obj: &Object, f: impl FnOnce(&mut PdoState) -> R) -> Result<R, Unwind> {
    let mut st = take_state(obj)?;
    let r = {
        let _scope = crate::bridge::InterpScope::enter(ctx);
        f(&mut st)
    };
    obj.set_payload(Payload::Native(Box::new(st)));
    // An exception a callback raised inside SQLite outranks the driver's
    // own report of the failed step.
    if let Some(u) = crate::bridge::take_unwind() {
        return Err(u);
    }
    Ok(r)
}

/// `PDO::getAttribute(int $attribute): mixed`
fn pdo_get_attribute(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let obj = this(o)?;
    let attr = int_arg(args, 0, 0);
    let (found, driver_answer) = with_pdo(obj, |st| {
        let a = &st.attrs;
        let v = match attr {
            ATTR_ERRMODE => Some(Value::Int(a.errmode)),
            ATTR_CASE => Some(Value::Int(a.case)),
            ATTR_ORACLE_NULLS => Some(Value::Int(a.oracle_nulls)),
            ATTR_STRINGIFY_FETCHES => Some(Value::Bool(a.stringify)),
            ATTR_DEFAULT_FETCH_MODE => Some(Value::Int(a.default_fetch.mode)),
            ATTR_DRIVER_NAME => Some(Value::string(st.conn.name().as_bytes())),
            ATTR_SERVER_VERSION | ATTR_CLIENT_VERSION => Some(Value::string(st.conn.server_version().as_bytes())),
            ATTR_STATEMENT_CLASS => {
                let mut out = Array::new();
                match &a.statement_class {
                    Some((name, ctor)) => {
                        out.push(Value::string(name));
                        let mut c = Array::new();
                        for v in ctor {
                            c.push(v.clone());
                        }
                        out.push(Value::Array(c));
                    }
                    None => out.push(Value::string(b"PDOStatement")),
                }
                Some(Value::Array(out))
            }
            other => st.conn.get_attribute(other),
        };
        (v.is_some(), v)
    })?;
    if found {
        return Ok(driver_answer.unwrap_or(Value::Null));
    }
    let err = DbError::new("IM001", 0, "driver does not support that attribute");
    report(ctx, obj, None, "PDO::getAttribute", &err)?;
    Ok(Value::Bool(false))
}

/// `PDO::exec(string $statement): int|false`
fn pdo_exec(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let obj = this(o)?;
    let sql = String::from_utf8_lossy(&str_arg(args, 0)).into_owned();
    if sql.is_empty() {
        return Err(Unwind::value_error("PDO::exec(): Argument #1 ($statement) must not be empty"));
    }
    clear_error(obj, None)?;
    let r = with_conn(ctx, obj, |st| st.conn.exec(&sql))?;
    match r {
        Ok(n) => Ok(Value::Int(n)),
        Err(e) => {
            report(ctx, obj, None, "PDO::exec", &e)?;
            Ok(Value::Bool(false))
        }
    }
}

/// `PDO::prepare(string $query, array $options = []): PDOStatement|false`
fn pdo_prepare(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let obj = this(o)?;
    let sql = String::from_utf8_lossy(&str_arg(args, 0)).into_owned();
    if sql.is_empty() {
        return Err(Unwind::value_error("PDO::prepare(): Argument #1 ($query) must not be empty"));
    }
    clear_error(obj, None)?;
    match crate::stmt::new_statement(ctx, obj, &sql)? {
        Ok(stmt) => Ok(Value::Object(stmt)),
        Err(e) => {
            report(ctx, obj, None, "PDO::prepare", &e)?;
            Ok(Value::Bool(false))
        }
    }
}

/// `PDO::query(string $query, ?int $fetchMode = null, mixed ...$fetchModeArgs): PDOStatement|false`
fn pdo_query(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let obj = this(o)?;
    let sql = String::from_utf8_lossy(&str_arg(args, 0)).into_owned();
    if sql.is_empty() {
        return Err(Unwind::value_error("PDO::query(): Argument #1 ($query) must not be empty"));
    }
    clear_error(obj, None)?;
    let stmt = match crate::stmt::new_statement(ctx, obj, &sql)? {
        Ok(s) => s,
        Err(e) => {
            report(ctx, obj, None, "PDO::query", &e)?;
            return Ok(Value::Bool(false));
        }
    };
    if let Some(mode) = args.get(1).map(|v| v.deref().into_owned()).filter(|v| !matches!(v, Value::Null)) {
        let mode_args: Vec<Value> = args.iter().skip(2).map(|v| v.deref().into_owned()).collect();
        crate::stmt::set_fetch_mode(ctx, &stmt, mode.to_int(), &mode_args, "PDO::query")?;
    }
    if !crate::stmt::execute(ctx, &stmt, None, "PDO::query")? {
        return Ok(Value::Bool(false));
    }
    Ok(Value::Object(stmt))
}

/// `PDO::lastInsertId(?string $name = null): string|false`
fn pdo_last_insert_id(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let obj = this(o)?;
    let name = args
        .first()
        .map(|v| v.deref().into_owned())
        .filter(|v| !matches!(v, Value::Null))
        .map(|v| String::from_utf8_lossy(&v.to_php_bytes()).into_owned());
    let r = with_conn(ctx, obj, |st| st.conn.last_insert_id(name.as_deref()))?;
    match r {
        Ok(id) => Ok(Value::string(id.as_bytes())),
        Err(e) => {
            report(ctx, obj, None, "PDO::lastInsertId", &e)?;
            Ok(Value::Bool(false))
        }
    }
}

/// `PDO::beginTransaction(): bool`
fn pdo_begin_transaction(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let obj = this(o)?;
    if with_pdo(obj, |st| st.in_tx)? {
        return Err(plain_exception(ctx, "There is already an active transaction", 0));
    }
    clear_error(obj, None)?;
    let r = with_conn(ctx, obj, |st| st.conn.begin())?;
    match r {
        Ok(()) => {
            with_pdo(obj, |st| st.in_tx = true)?;
            Ok(Value::Bool(true))
        }
        Err(e) => {
            report(ctx, obj, None, "PDO::beginTransaction", &e)?;
            Ok(Value::Bool(false))
        }
    }
}

/// `PDO::commit(): bool`
fn pdo_commit(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let obj = this(o)?;
    if !with_pdo(obj, |st| st.in_tx)? {
        return Err(plain_exception(ctx, "There is no active transaction", 0));
    }
    clear_error(obj, None)?;
    let r = with_conn(ctx, obj, |st| st.conn.commit())?;
    with_pdo(obj, |st| st.in_tx = false)?;
    match r {
        Ok(()) => Ok(Value::Bool(true)),
        Err(e) => {
            report(ctx, obj, None, "PDO::commit", &e)?;
            Ok(Value::Bool(false))
        }
    }
}

/// `PDO::rollBack(): bool`
fn pdo_roll_back(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let obj = this(o)?;
    if !with_pdo(obj, |st| st.in_tx)? {
        return Err(plain_exception(ctx, "There is no active transaction", 0));
    }
    clear_error(obj, None)?;
    let r = with_conn(ctx, obj, |st| st.conn.rollback())?;
    with_pdo(obj, |st| st.in_tx = false)?;
    match r {
        Ok(()) => Ok(Value::Bool(true)),
        Err(e) => {
            report(ctx, obj, None, "PDO::rollBack", &e)?;
            Ok(Value::Bool(false))
        }
    }
}

/// `PDO::inTransaction(): bool` — php asks the driver, which for SQLite
/// is whether autocommit is off (a `BEGIN` sent through `exec()` counts).
fn pdo_in_transaction(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let obj = this(o)?;
    let r = with_pdo(obj, |st| st.conn.in_transaction())?;
    Ok(Value::Bool(r))
}

/// `PDO::quote(string $string, int $type = PDO::PARAM_STR): string|false`
fn pdo_quote(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let obj = this(o)?;
    if matches!(args.first().map(|v| v.deref().into_owned()), Some(Value::Null)) {
        ctx.deprecated("PDO::quote(): Passing null to parameter #1 ($string) of type string is deprecated")?;
    }
    let s = str_arg(args, 0);
    let r = with_pdo(obj, |st| st.conn.quote(&s))?;
    match r {
        Ok(q) => Ok(Value::string(q.as_bytes())),
        Err(e) if e.sqlstate.is_empty() || e.code == 0 && e.message.starts_with("SQLite PDO::quote") => {
            Err(plain_exception(ctx, &e.message, 0))
        }
        Err(e) => {
            report(ctx, obj, None, "PDO::quote", &e)?;
            Ok(Value::Bool(false))
        }
    }
}

/// `PDO::errorCode(): ?string`
fn pdo_error_code(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let obj = this(o)?;
    let s = with_pdo(obj, |st| st.error.sqlstate.clone())?;
    Ok(Value::string(s.as_bytes()))
}

/// `PDO::errorInfo(): array`
fn pdo_error_info(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let obj = this(o)?;
    with_pdo(obj, |st| st.error.info())
}

// ---- Pdo\Sqlite ------------------------------------------------------------------

/// The `callable` argument of the `Pdo\Sqlite` registration methods.
fn callable_arg(ctx: &mut Ctx, who: &str, args: &[Value], i: usize, name: &str) -> Result<Value, Unwind> {
    let v = args.get(i).map(|v| v.deref().into_owned()).unwrap_or(Value::Null);
    if !ctx.is_callable_autoload(&v)? {
        return Err(Unwind::type_error(format!(
            "{who}(): Argument #{} (${name}) must be a valid callback, no array or string given",
            i + 1
        )));
    }
    Ok(v)
}

fn sqlite_of<R>(ctx: &mut Ctx, obj: &Object, f: impl FnOnce(&mut Sqlite) -> Result<R, DbError>) -> Result<Result<R, DbError>, Unwind> {
    with_conn(ctx, obj, |st| {
        // The connection is SQLite's own when the object is a `Pdo\Sqlite`
        // (or a `PDO` over a `sqlite:` DSN); a downcast through `Any`.
        let any: &mut dyn std::any::Any = st.conn.as_any_mut();
        match any.downcast_mut::<Sqlite>() {
            Some(s) => f(s),
            None => Err(DbError::new("HY000", 0, "not an SQLite connection")),
        }
    })
}

/// `Pdo\Sqlite::createFunction(string $function_name, callable $callback, int $num_args = -1, int $flags = 0): bool`
fn sqlite_create_function(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let obj = this(o)?;
    let name = String::from_utf8_lossy(&str_arg(args, 0)).into_owned();
    let cb = callable_arg(ctx, "Pdo\\Sqlite::createFunction", args, 1, "callback")?;
    let n = int_arg(args, 2, -1);
    let flags = int_arg(args, 3, 0);
    let r = sqlite_of(ctx, obj, |s| s.create_function(&name, cb, n, flags))?;
    Ok(Value::Bool(r.is_ok()))
}

/// `Pdo\Sqlite::createAggregate(string $name, callable $step, callable $finalize, int $numArgs = -1): bool`
fn sqlite_create_aggregate(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let obj = this(o)?;
    let name = String::from_utf8_lossy(&str_arg(args, 0)).into_owned();
    let step = callable_arg(ctx, "Pdo\\Sqlite::createAggregate", args, 1, "step")?;
    let fin = callable_arg(ctx, "Pdo\\Sqlite::createAggregate", args, 2, "finalize")?;
    let n = int_arg(args, 3, -1);
    let r = sqlite_of(ctx, obj, |s| s.create_aggregate(&name, step, fin, n))?;
    Ok(Value::Bool(r.is_ok()))
}

/// `Pdo\Sqlite::createCollation(string $name, callable $callback): bool`
fn sqlite_create_collation(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let obj = this(o)?;
    let name = String::from_utf8_lossy(&str_arg(args, 0)).into_owned();
    let cb = callable_arg(ctx, "Pdo\\Sqlite::createCollation", args, 1, "callback")?;
    let r = sqlite_of(ctx, obj, |s| s.create_collation(&name, cb))?;
    Ok(Value::Bool(r.is_ok()))
}

/// `Pdo\Sqlite::loadExtension(string $name): void`
fn sqlite_load_extension(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let obj = this(o)?;
    let name = String::from_utf8_lossy(&str_arg(args, 0)).into_owned();
    let r = sqlite_of(ctx, obj, |s| s.load_extension(&name))?;
    match r {
        Ok(()) => Ok(Value::Null),
        Err(e) => Err(plain_exception(ctx, &e.message, 0)),
    }
}

/// The deprecated `PDO::sqliteCreate*()` spellings: a deprecation, then the
/// `Pdo\Sqlite` method.
fn deprecated_sqlite(ctx: &mut Ctx, old: &str, new: &str) -> Result<(), Unwind> {
    ctx.deprecated(&format!(
        "Method PDO::{old}() is deprecated since 8.5, use Pdo\\Sqlite::{new}() instead"
    ))
}

fn pdo_sqlite_create_function(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    deprecated_sqlite(ctx, "sqliteCreateFunction", "createFunction")?;
    sqlite_create_function(ctx, o, args)
}

fn pdo_sqlite_create_aggregate(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    deprecated_sqlite(ctx, "sqliteCreateAggregate", "createAggregate")?;
    sqlite_create_aggregate(ctx, o, args)
}

fn pdo_sqlite_create_collation(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    deprecated_sqlite(ctx, "sqliteCreateCollation", "createCollation")?;
    sqlite_create_collation(ctx, o, args)
}

// ---- registration ------------------------------------------------------------------

/// Register the extension's classes and constants.
pub fn register(r: &mut Registry) {
    if r.interp().class_by_name(b"PDO").is_some() {
        return;
    }
    r.extension("PDO");
    r.extension("pdo_sqlite");
    let i = |n: i64| Value::Int(n);
    let s = |v: &str| Value::string(v.as_bytes());

    r.class("PDOException")
        .extends("RuntimeException")
        .prop("errorInfo", Visibility::Public, Value::Null)
        .finish();

    let mut pdo = r
        .class("PDO")
        .method("__construct", nm!(1, Some(4), pdo_construct))
        .method("connect", static_method(1, Some(4), pdo_connect))
        .method("getAvailableDrivers", static_method(0, Some(0), pdo_get_available_drivers))
        .method("setAttribute", nm!(2, Some(2), pdo_set_attribute))
        .method("getAttribute", nm!(1, Some(1), pdo_get_attribute))
        .method("exec", nm!(1, Some(1), pdo_exec))
        .method("prepare", nm!(1, Some(2), pdo_prepare))
        .method("query", nm!(1, None, pdo_query))
        .method("lastInsertId", nm!(0, Some(1), pdo_last_insert_id))
        .method("beginTransaction", nm!(0, Some(0), pdo_begin_transaction))
        .method("commit", nm!(0, Some(0), pdo_commit))
        .method("rollBack", nm!(0, Some(0), pdo_roll_back))
        .method("inTransaction", nm!(0, Some(0), pdo_in_transaction))
        .method("quote", nm!(1, Some(2), pdo_quote))
        .method("errorCode", nm!(0, Some(0), pdo_error_code))
        .method("errorInfo", nm!(0, Some(0), pdo_error_info))
        .method("sqliteCreateFunction", nm!(2, Some(4), pdo_sqlite_create_function))
        .method("sqliteCreateAggregate", nm!(3, Some(4), pdo_sqlite_create_aggregate))
        .method("sqliteCreateCollation", nm!(2, Some(2), pdo_sqlite_create_collation));
    for (name, v) in [
        ("PARAM_NULL", PARAM_NULL),
        ("PARAM_BOOL", PARAM_BOOL),
        ("PARAM_INT", PARAM_INT),
        ("PARAM_STR", PARAM_STR),
        ("PARAM_LOB", PARAM_LOB),
        ("PARAM_STMT", PARAM_STMT),
        ("PARAM_INPUT_OUTPUT", PARAM_INPUT_OUTPUT),
        ("PARAM_STR_NATL", 1_073_741_824),
        ("PARAM_STR_CHAR", 536_870_912),
        ("PARAM_EVT_ALLOC", 0),
        ("PARAM_EVT_FREE", 1),
        ("PARAM_EVT_EXEC_PRE", 2),
        ("PARAM_EVT_EXEC_POST", 3),
        ("PARAM_EVT_FETCH_PRE", 4),
        ("PARAM_EVT_FETCH_POST", 5),
        ("PARAM_EVT_NORMALIZE", 6),
        ("FETCH_DEFAULT", FETCH_DEFAULT),
        ("FETCH_LAZY", FETCH_LAZY),
        ("FETCH_ASSOC", FETCH_ASSOC),
        ("FETCH_NUM", FETCH_NUM),
        ("FETCH_BOTH", FETCH_BOTH),
        ("FETCH_OBJ", FETCH_OBJ),
        ("FETCH_BOUND", FETCH_BOUND),
        ("FETCH_COLUMN", FETCH_COLUMN),
        ("FETCH_CLASS", FETCH_CLASS),
        ("FETCH_INTO", FETCH_INTO),
        ("FETCH_FUNC", FETCH_FUNC),
        ("FETCH_GROUP", FETCH_GROUP),
        ("FETCH_UNIQUE", FETCH_UNIQUE),
        ("FETCH_KEY_PAIR", FETCH_KEY_PAIR),
        ("FETCH_CLASSTYPE", FETCH_CLASSTYPE),
        ("FETCH_SERIALIZE", FETCH_SERIALIZE),
        ("FETCH_PROPS_LATE", FETCH_PROPS_LATE),
        ("FETCH_NAMED", FETCH_NAMED),
        ("ATTR_AUTOCOMMIT", ATTR_AUTOCOMMIT),
        ("ATTR_PREFETCH", ATTR_PREFETCH),
        ("ATTR_TIMEOUT", ATTR_TIMEOUT),
        ("ATTR_ERRMODE", ATTR_ERRMODE),
        ("ATTR_SERVER_VERSION", ATTR_SERVER_VERSION),
        ("ATTR_CLIENT_VERSION", ATTR_CLIENT_VERSION),
        ("ATTR_SERVER_INFO", ATTR_SERVER_INFO),
        ("ATTR_CONNECTION_STATUS", ATTR_CONNECTION_STATUS),
        ("ATTR_CASE", ATTR_CASE),
        ("ATTR_CURSOR_NAME", ATTR_CURSOR_NAME),
        ("ATTR_CURSOR", ATTR_CURSOR),
        ("ATTR_ORACLE_NULLS", ATTR_ORACLE_NULLS),
        ("ATTR_PERSISTENT", ATTR_PERSISTENT),
        ("ATTR_STATEMENT_CLASS", ATTR_STATEMENT_CLASS),
        ("ATTR_FETCH_TABLE_NAMES", ATTR_FETCH_TABLE_NAMES),
        ("ATTR_FETCH_CATALOG_NAMES", ATTR_FETCH_CATALOG_NAMES),
        ("ATTR_DRIVER_NAME", ATTR_DRIVER_NAME),
        ("ATTR_STRINGIFY_FETCHES", ATTR_STRINGIFY_FETCHES),
        ("ATTR_MAX_COLUMN_LEN", ATTR_MAX_COLUMN_LEN),
        ("ATTR_EMULATE_PREPARES", ATTR_EMULATE_PREPARES),
        ("ATTR_DEFAULT_FETCH_MODE", ATTR_DEFAULT_FETCH_MODE),
        ("ATTR_DEFAULT_STR_PARAM", ATTR_DEFAULT_STR_PARAM),
        ("ERRMODE_SILENT", ERRMODE_SILENT),
        ("ERRMODE_WARNING", ERRMODE_WARNING),
        ("ERRMODE_EXCEPTION", ERRMODE_EXCEPTION),
        ("CASE_NATURAL", CASE_NATURAL),
        ("CASE_LOWER", CASE_LOWER),
        ("CASE_UPPER", CASE_UPPER),
        ("NULL_NATURAL", NULL_NATURAL),
        ("NULL_EMPTY_STRING", NULL_EMPTY_STRING),
        ("NULL_TO_STRING", NULL_TO_STRING),
        ("FETCH_ORI_NEXT", 0),
        ("FETCH_ORI_PRIOR", 1),
        ("FETCH_ORI_FIRST", 2),
        ("FETCH_ORI_LAST", 3),
        ("FETCH_ORI_ABS", 4),
        ("FETCH_ORI_REL", 5),
        ("CURSOR_FWDONLY", 0),
        ("CURSOR_SCROLL", 1),
    ] {
        pdo = pdo.class_const(name, i(v));
    }
    pdo = pdo.class_const("ERR_NONE", s("00000"));
    // The driver-specific constants php deprecated in 8.5 in favour of the
    // `Pdo\<Driver>` subclasses' own.
    for (name, v, new) in [
        ("SQLITE_DETERMINISTIC", 2048, "DETERMINISTIC"),
        ("SQLITE_ATTR_OPEN_FLAGS", 1000, "ATTR_OPEN_FLAGS"),
        ("SQLITE_OPEN_READONLY", 1, "OPEN_READONLY"),
        ("SQLITE_OPEN_READWRITE", 2, "OPEN_READWRITE"),
        ("SQLITE_OPEN_CREATE", 4, "OPEN_CREATE"),
        ("SQLITE_ATTR_READONLY_STATEMENT", 1001, "ATTR_READONLY_STATEMENT"),
        ("SQLITE_ATTR_EXTENDED_RESULT_CODES", 1002, "ATTR_EXTENDED_RESULT_CODES"),
    ] {
        pdo = pdo.deprecated_class_const(
            name,
            i(v),
            Box::leak(format!(" since 8.5, use Pdo\\Sqlite::{new} instead").into_boxed_str()),
        );
    }
    for (name, v, new) in [
        ("MYSQL_ATTR_USE_BUFFERED_QUERY", 1000, "Pdo\\Mysql::ATTR_USE_BUFFERED_QUERY"),
        ("MYSQL_ATTR_LOCAL_INFILE", 1001, "Pdo\\Mysql::ATTR_LOCAL_INFILE"),
        ("MYSQL_ATTR_INIT_COMMAND", 1002, "Pdo\\Mysql::ATTR_INIT_COMMAND"),
        ("MYSQL_ATTR_COMPRESS", 1003, "Pdo\\Mysql::ATTR_COMPRESS"),
        ("MYSQL_ATTR_DIRECT_QUERY", 20, "Pdo\\Mysql::ATTR_DIRECT_QUERY"),
        ("MYSQL_ATTR_FOUND_ROWS", 1004, "Pdo\\Mysql::ATTR_FOUND_ROWS"),
        ("MYSQL_ATTR_IGNORE_SPACE", 1005, "Pdo\\Mysql::ATTR_IGNORE_SPACE"),
        ("MYSQL_ATTR_SSL_KEY", 1006, "Pdo\\Mysql::ATTR_SSL_KEY"),
        ("MYSQL_ATTR_SSL_CERT", 1007, "Pdo\\Mysql::ATTR_SSL_CERT"),
        ("MYSQL_ATTR_SSL_CA", 1008, "Pdo\\Mysql::ATTR_SSL_CA"),
        ("MYSQL_ATTR_SSL_CAPATH", 1009, "Pdo\\Mysql::ATTR_SSL_CAPATH"),
        ("MYSQL_ATTR_SSL_CIPHER", 1010, "Pdo\\Mysql::ATTR_SSL_CIPHER"),
        ("MYSQL_ATTR_SERVER_PUBLIC_KEY", 1011, "Pdo\\Mysql::ATTR_SERVER_PUBLIC_KEY"),
        ("MYSQL_ATTR_MULTI_STATEMENTS", 1012, "Pdo\\Mysql::ATTR_MULTI_STATEMENTS"),
        ("MYSQL_ATTR_SSL_VERIFY_SERVER_CERT", 1013, "Pdo\\Mysql::ATTR_SSL_VERIFY_SERVER_CERT"),
        ("MYSQL_ATTR_LOCAL_INFILE_DIRECTORY", 1014, "Pdo\\Mysql::ATTR_LOCAL_INFILE_DIRECTORY"),
        ("PGSQL_ATTR_DISABLE_PREPARES", 1000, "Pdo\\Pgsql::ATTR_DISABLE_PREPARES"),
        ("PGSQL_TRANSACTION_IDLE", 0, "Pdo\\Pgsql::TRANSACTION_IDLE"),
        ("PGSQL_TRANSACTION_ACTIVE", 1, "Pdo\\Pgsql::TRANSACTION_ACTIVE"),
        ("PGSQL_TRANSACTION_INTRANS", 2, "Pdo\\Pgsql::TRANSACTION_INTRANS"),
        ("PGSQL_TRANSACTION_INERROR", 3, "Pdo\\Pgsql::TRANSACTION_INERROR"),
        ("PGSQL_TRANSACTION_UNKNOWN", 4, "Pdo\\Pgsql::TRANSACTION_UNKNOWN"),
    ] {
        pdo = pdo.deprecated_class_const(
            name,
            i(v),
            Box::leak(format!(" since 8.5, use {new} instead").into_boxed_str()),
        );
    }
    // `PDO::MYSQL_ATTR_*`, deprecated in 8.5 like the SQLite ones in favour
    // of `Pdo\Mysql::ATTR_*`.
    for (name, v) in crate::host::MYSQL_ATTRS {
        pdo = pdo.deprecated_class_const(
            &format!("MYSQL_{name}"),
            Value::Int(*v),
            Box::leak(format!(" since 8.5, use Pdo\\Mysql::{name} instead").into_boxed_str()),
        );
    }
    pdo.finish();

    // php 8.4's per-driver subclass for a host-answered `mysql:` scheme, with
    // the driver constants both spellings carry (`PDO::MYSQL_ATTR_*` on the
    // parent is registered above alongside the other PDO constants).
    let mut mysql = r.class("Pdo\\Mysql").extends("PDO");
    for (name, v) in crate::host::MYSQL_ATTRS {
        mysql = mysql.class_const(name, Value::Int(*v));
    }
    mysql.finish();

    let mut sqlite = r
        .class("Pdo\\Sqlite")
        .extends("PDO")
        .method("createFunction", nm!(2, Some(4), sqlite_create_function))
        .method("createAggregate", nm!(3, Some(4), sqlite_create_aggregate))
        .method("createCollation", nm!(2, Some(2), sqlite_create_collation))
        .method("loadExtension", nm!(1, Some(1), sqlite_load_extension));
    for (name, v) in [
        ("DETERMINISTIC", 2048),
        ("OPEN_READONLY", 1),
        ("OPEN_READWRITE", 2),
        ("OPEN_CREATE", 4),
        ("ATTR_OPEN_FLAGS", 1000),
        ("ATTR_READONLY_STATEMENT", 1001),
        ("ATTR_EXTENDED_RESULT_CODES", 1002),
        ("ATTR_BUSY_STATEMENT", 1003),
        ("ATTR_EXPLAIN_STATEMENT", 1004),
        ("ATTR_TRANSACTION_MODE", 1005),
        ("TRANSACTION_MODE_DEFERRED", 0),
        ("TRANSACTION_MODE_IMMEDIATE", 1),
        ("TRANSACTION_MODE_EXCLUSIVE", 2),
        ("EXPLAIN_MODE_PREPARED", 0),
        ("EXPLAIN_MODE_EXPLAIN", 1),
        ("EXPLAIN_MODE_EXPLAIN_QUERY_PLAN", 2),
        ("OK", 0),
        ("DENY", 1),
        ("IGNORE", 2),
    ] {
        sqlite = sqlite.class_const(name, i(v));
    }
    sqlite.finish();

    crate::stmt::register(r);
}
