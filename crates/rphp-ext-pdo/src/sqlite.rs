//! `pdo_sqlite` over rusqlite (SQLite compiled in): the connection, the
//! statement runs, php's SQLSTATE mapping of SQLite's result codes, and the
//! user-defined functions, aggregates and collations of `Pdo\Sqlite`.

use std::cmp::Ordering;

use rphp_value::Value;
use rusqlite::functions::{Aggregate, Context, FunctionFlags};
use rusqlite::types::{ToSqlOutput, Value as SqlV, ValueRef};
use rusqlite::{Connection, OpenFlags};

use crate::bridge::{stash_unwind, with_interp, Sendable};
use crate::driver::{Column, DbError, Driver, ParamKey, ResultSet, SqlValue};

/// php's mapping of `sqlite3_errcode()` to a SQLSTATE
/// (`_pdo_sqlite_error`).
pub fn sqlstate_of(code: i64) -> &'static str {
    match code {
        12 => "42S02", // SQLITE_NOTFOUND
        9 => "01002",  // SQLITE_INTERRUPT
        22 => "HYC00", // SQLITE_NOLFS
        18 => "22001", // SQLITE_TOOBIG
        19 => "23000", // SQLITE_CONSTRAINT
        _ => "HY000",
    }
}

/// A rusqlite error as php reports it: the primary result code and
/// `sqlite3_errmsg()`.
fn map_err(e: rusqlite::Error) -> DbError {
    match e {
        rusqlite::Error::SqliteFailure(ffi, msg) => {
            // The primary result code (`sqlite3_errcode()`), not the
            // extended one.
            let code = i64::from(ffi.extended_code & 0xff);
            let message = msg.unwrap_or_else(|| ffi.to_string());
            DbError::new(sqlstate_of(code), code, message)
        }
        rusqlite::Error::InvalidParameterName(_) | rusqlite::Error::InvalidParameterCount(..) => {
            DbError::new("HY000", 25, "column index out of range")
        }
        rusqlite::Error::InvalidColumnIndex(_) | rusqlite::Error::InvalidColumnName(_) => {
            DbError::new("HY000", 25, "column index out of range")
        }
        rusqlite::Error::UserFunctionError(inner) => DbError::new("HY000", 1, inner.to_string()),
        // rusqlite files an error SQLite can point into the SQL under its own
        // variant; php shows `sqlite3_errmsg()` alone.
        rusqlite::Error::SqlInputError { error, msg, .. } => {
            let code = i64::from(error.extended_code & 0xff);
            DbError::new(sqlstate_of(code), code, msg)
        }
        other => DbError::new("HY000", 1, other.to_string()),
    }
}

/// The SQLite connection behind a `PDO` / `Pdo\Sqlite`.
pub struct Sqlite {
    conn: Connection,
    /// `Pdo\Sqlite::ATTR_EXTENDED_RESULT_CODES`.
    extended_codes: bool,
    /// `Pdo\Sqlite::ATTR_TRANSACTION_MODE`.
    transaction_mode: i64,
    /// `Pdo\Sqlite::ATTR_BUSY_STATEMENT` timeout (ms), `PDO::ATTR_TIMEOUT`.
    timeout_ms: i64,
}

impl Sqlite {
    /// Open the database a `sqlite:` DSN names: `:memory:`, a path, or
    /// nothing (a private temporary database).
    pub fn open(target: &str, open_flags: Option<i64>) -> Result<Sqlite, DbError> {
        let flags = match open_flags {
            Some(f) => OpenFlags::from_bits_truncate(f as i32) | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            None => OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        };
        let conn = match target {
            ":memory:" => Connection::open_in_memory_with_flags(flags),
            "" => Connection::open_with_flags("", flags),
            path => Connection::open_with_flags(path, flags),
        }
        .map_err(|e| {
            // rusqlite appends the path to an open failure; php reports
            // `sqlite3_errmsg()` as is.
            let mut err = map_err(e);
            if let Some(stripped) = err.message.strip_suffix(&format!(": {target}")) {
                err.message = stripped.to_string();
            }
            err
        })?;
        Ok(Sqlite {
            conn,
            extended_codes: false,
            transaction_mode: 0,
            timeout_ms: 0,
        })
    }

    fn convert_ref(v: ValueRef<'_>) -> SqlValue {
        match v {
            ValueRef::Null => SqlValue::Null,
            ValueRef::Integer(i) => SqlValue::Int(i),
            ValueRef::Real(f) => SqlValue::Float(f),
            ValueRef::Text(t) => SqlValue::Text(t.to_vec()),
            ValueRef::Blob(b) => SqlValue::Blob(b.to_vec()),
        }
    }

    fn native_type(v: &SqlValue) -> &'static str {
        match v {
            SqlValue::Null => "null",
            SqlValue::Int(_) => "integer",
            SqlValue::Float(_) => "double",
            SqlValue::Text(_) | SqlValue::Blob(_) => "string",
        }
    }

    // ---- Pdo\Sqlite --------------------------------------------------------

    /// `Pdo\Sqlite::createFunction()`: `callback` is called with the
    /// arguments as php values and its return becomes the SQL value.
    pub fn create_function(&mut self, name: &str, callback: Value, num_args: i64, flags: i64) -> Result<(), DbError> {
        let cb = Sendable(callback);
        let n = i32::try_from(num_args).unwrap_or(-1);
        let mut fflags = FunctionFlags::SQLITE_UTF8;
        if flags & 2048 != 0 {
            fflags |= FunctionFlags::SQLITE_DETERMINISTIC;
        }
        self.conn
            .create_scalar_function(name, n, fflags, move |ctx: &Context<'_>| {
                let args: Vec<Value> = (0..ctx.len())
                    .map(|i| ctx.get_raw(i))
                    .map(|v| sql_to_php(&Sqlite::convert_ref(v)))
                    .collect();
                let result = with_interp(|it| it.call_value(cb.get(), &args))
                    .unwrap_or_else(|| Err(rphp_runtime::Unwind::error("no interpreter")));
                match result {
                    Ok(v) => Ok(php_to_sql_output(&v)),
                    Err(u) => {
                        stash_unwind(u);
                        Err(rusqlite::Error::UserFunctionError(Box::new(CallbackError)))
                    }
                }
            })
            .map_err(map_err)
    }

    /// `Pdo\Sqlite::createAggregate()`: `step($context, $rowNumber, ...$args)`
    /// and `finalize($context, $rowNumber)`.
    pub fn create_aggregate(&mut self, name: &str, step: Value, finalize: Value, num_args: i64) -> Result<(), DbError> {
        let agg = PhpAggregate {
            step: Sendable(step),
            finalize: Sendable(finalize),
        };
        let n = i32::try_from(num_args).unwrap_or(-1);
        self.conn
            .create_aggregate_function(name, n, FunctionFlags::SQLITE_UTF8, agg)
            .map_err(map_err)
    }

    /// `Pdo\Sqlite::createCollation()`: the callback answers `<0`, `0`, `>0`.
    pub fn create_collation(&mut self, name: &str, callback: Value) -> Result<(), DbError> {
        let cb = Sendable(callback);
        self.conn
            .create_collation(name, move |a: &str, b: &str| {
                let r = with_interp(|it| it.call_value(cb.get(), &[Value::string(a.as_bytes()), Value::string(b.as_bytes())]));
                match r {
                    Some(Ok(v)) => v.to_int().cmp(&0),
                    Some(Err(u)) => {
                        stash_unwind(u);
                        Ordering::Equal
                    }
                    None => Ordering::Equal,
                }
            })
            .map_err(map_err)
    }

    /// `Pdo\Sqlite::loadExtension()`.
    pub fn load_extension(&mut self, path: &str) -> Result<(), DbError> {
        // SAFETY of loading is SQLite's own concern; rusqlite marks the call
        // unsafe because an extension is arbitrary native code, exactly as
        // php's `loadExtension()` is.
        #[allow(unsafe_code)]
        unsafe {
            self.conn.load_extension_enable().map_err(map_err)?;
            let r = self.conn.load_extension(path, None::<&str>).map_err(map_err);
            let _ = self.conn.load_extension_disable();
            r
        }
    }
}

/// A php value produced by SQLite's callback machinery.
fn sql_to_php(v: &SqlValue) -> Value {
    match v {
        SqlValue::Null => Value::Null,
        SqlValue::Int(i) => Value::Int(*i),
        SqlValue::Float(f) => Value::Float(*f),
        SqlValue::Text(t) | SqlValue::Blob(t) => Value::string(t),
    }
}

/// A php value returned by a callback, as SQLite takes it.
fn php_to_sql_output(v: &Value) -> ToSqlOutput<'static> {
    let v = v.deref().into_owned();
    ToSqlOutput::Owned(match v {
        Value::Null | Value::Uninit => SqlV::Null,
        Value::Bool(b) => SqlV::Integer(i64::from(b)),
        Value::Int(i) => SqlV::Integer(i),
        Value::Float(f) => SqlV::Real(f),
        other => {
            let bytes = other.to_php_bytes();
            match String::from_utf8(bytes) {
                Ok(s) => SqlV::Text(s),
                Err(e) => SqlV::Blob(e.into_bytes()),
            }
        }
    })
}

/// A php value bound as a statement parameter.
fn param_to_sql(v: &SqlValue) -> SqlV {
    match v {
        SqlValue::Null => SqlV::Null,
        SqlValue::Int(i) => SqlV::Integer(*i),
        SqlValue::Float(f) => SqlV::Real(*f),
        SqlValue::Text(t) => match String::from_utf8(t.clone()) {
            Ok(s) => SqlV::Text(s),
            Err(e) => SqlV::Blob(e.into_bytes()),
        },
        SqlValue::Blob(b) => SqlV::Blob(b.clone()),
    }
}

/// The marker SQLite sees when a php callback threw; the exception itself
/// waits in `bridge::take_unwind()`.
#[derive(Debug)]
pub struct CallbackError;

impl std::fmt::Display for CallbackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a php callback threw")
    }
}

impl std::error::Error for CallbackError {}

struct PhpAggregate {
    step: Sendable<Value>,
    finalize: Sendable<Value>,
}

/// The aggregate's running state: php's `$context` and the row count.
struct AggState {
    context: Value,
    rows: i64,
}

// rusqlite asks the accumulator to be unwind-safe; a php value is only ever
// touched between two callbacks on this thread, and a panic in between is
// a bug that ends the request anyway.
impl std::panic::UnwindSafe for AggState {}
impl std::panic::RefUnwindSafe for AggState {}

impl Aggregate<AggState, ToSqlOutput<'static>> for PhpAggregate {
    fn init(&self, _: &mut Context<'_>) -> rusqlite::Result<AggState> {
        Ok(AggState {
            context: Value::Null,
            rows: 0,
        })
    }

    fn step(&self, ctx: &mut Context<'_>, st: &mut AggState) -> rusqlite::Result<()> {
        st.rows += 1;
        let mut args = vec![st.context.clone(), Value::Int(st.rows)];
        for i in 0..ctx.len() {
            args.push(sql_to_php(&Sqlite::convert_ref(ctx.get_raw(i))));
        }
        match with_interp(|it| it.call_value(self.step.get(), &args)) {
            Some(Ok(v)) => {
                st.context = v;
                Ok(())
            }
            Some(Err(u)) => {
                stash_unwind(u);
                Err(rusqlite::Error::UserFunctionError(Box::new(CallbackError)))
            }
            None => Err(rusqlite::Error::UserFunctionError("no interpreter".into())),
        }
    }

    fn finalize(&self, _: &mut Context<'_>, st: Option<AggState>) -> rusqlite::Result<ToSqlOutput<'static>> {
        let st = st.unwrap_or(AggState {
            context: Value::Null,
            rows: 0,
        });
        match with_interp(|it| it.call_value(self.finalize.get(), &[st.context, Value::Int(st.rows)])) {
            Some(Ok(v)) => Ok(php_to_sql_output(&v)),
            Some(Err(u)) => {
                stash_unwind(u);
                Err(rusqlite::Error::UserFunctionError(Box::new(CallbackError)))
            }
            None => Err(rusqlite::Error::UserFunctionError("no interpreter".into())),
        }
    }
}

impl Driver for Sqlite {
    fn name(&self) -> &'static str {
        "sqlite"
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn exec(&mut self, sql: &str) -> Result<i64, DbError> {
        self.conn.execute_batch(sql).map_err(map_err)?;
        Ok(i64::from(self.conn.changes() as i64))
    }

    fn prepare(&mut self, sql: &str) -> Result<Vec<Option<String>>, DbError> {
        let stmt = self.conn.prepare(sql).map_err(map_err)?;
        let n = stmt.parameter_count();
        Ok((1..=n).map(|i| stmt.parameter_name(i).map(str::to_string)).collect())
    }

    fn run(&mut self, sql: &str, params: &[(ParamKey, SqlValue)]) -> Result<ResultSet, DbError> {
        let mut stmt = self.conn.prepare(sql).map_err(map_err)?;
        for (key, value) in params {
            let idx = match key {
                ParamKey::Pos(p) => *p,
                ParamKey::Name(n) => match stmt.parameter_index(n).map_err(map_err)? {
                    Some(i) => i,
                    None => return Err(DbError::new("HY000", 25, "column index out of range")),
                },
            };
            if idx == 0 || idx > stmt.parameter_count() {
                return Err(DbError::new("HY000", 25, "column index out of range"));
            }
            stmt.raw_bind_parameter(idx, param_to_sql(value)).map_err(map_err)?;
        }
        let count = stmt.column_count();
        let metas = stmt.columns_with_metadata();
        let mut columns: Vec<Column> = stmt
            .columns()
            .iter()
            .enumerate()
            .map(|(i, c)| Column {
                name: c.name().to_string(),
                decl_type: c.decl_type().map(str::to_string),
                table: metas.get(i).and_then(|m| m.table_name().map(str::to_string)),
                native_type: None,
            })
            .collect();
        let mut rows: Vec<Vec<SqlValue>> = Vec::new();
        {
            let mut rs = stmt.raw_query();
            loop {
                match rs.next() {
                    Ok(Some(row)) => {
                        let mut out = Vec::with_capacity(count);
                        for i in 0..count {
                            let v = row.get_ref(i).map_err(map_err)?;
                            out.push(Sqlite::convert_ref(v));
                        }
                        rows.push(out);
                    }
                    Ok(None) => break,
                    Err(e) => return Err(map_err(e)),
                }
            }
        }
        if let Some(first) = rows.first() {
            for (c, v) in columns.iter_mut().zip(first.iter()) {
                c.native_type = Some(Sqlite::native_type(v));
            }
        }
        Ok(ResultSet {
            columns,
            rows,
            changes: self.conn.changes() as i64,
        })
    }

    fn last_insert_id(&mut self, _name: Option<&str>) -> Result<String, DbError> {
        Ok(self.conn.last_insert_rowid().to_string())
    }

    fn begin(&mut self) -> Result<(), DbError> {
        let sql = match self.transaction_mode {
            1 => "BEGIN IMMEDIATE",
            2 => "BEGIN EXCLUSIVE",
            _ => "BEGIN",
        };
        self.conn.execute_batch(sql).map_err(map_err)
    }

    fn commit(&mut self) -> Result<(), DbError> {
        self.conn.execute_batch("COMMIT").map_err(map_err)
    }

    fn rollback(&mut self) -> Result<(), DbError> {
        self.conn.execute_batch("ROLLBACK").map_err(map_err)
    }

    fn in_transaction(&self) -> bool {
        !self.conn.is_autocommit()
    }

    fn quote(&self, s: &[u8]) -> Result<String, DbError> {
        if s.contains(&0) {
            return Err(DbError::new("HY000", 0, "SQLite PDO::quote does not support null bytes"));
        }
        let text = String::from_utf8_lossy(s);
        Ok(format!("'{}'", text.replace('\'', "''")))
    }

    fn server_version(&self) -> String {
        rusqlite::version().to_string()
    }

    /// php's `pdo_sqlite_get_attribute`: the transaction mode is the one
    /// driver attribute that reads back.
    fn get_attribute(&self, attr: i64) -> Option<Value> {
        match attr {
            1005 => Some(Value::Int(self.transaction_mode)),
            _ => None,
        }
    }

    /// php's `pdo_sqlite_set_attr`: the busy timeout (`PDO::ATTR_TIMEOUT`,
    /// seconds), extended result codes and the transaction mode.
    fn set_attribute(&mut self, attr: i64, value: &Value) -> Result<bool, DbError> {
        match attr {
            2 => {
                self.timeout_ms = value.to_int() * 1000;
                self.conn
                    .busy_timeout(std::time::Duration::from_millis(self.timeout_ms.max(0) as u64))
                    .map_err(map_err)?;
                Ok(true)
            }
            1002 => {
                self.extended_codes = value.to_bool();
                Ok(true)
            }
            1005 => {
                self.transaction_mode = value.to_int();
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}
