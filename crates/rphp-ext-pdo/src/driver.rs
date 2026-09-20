//! The driver boundary: what `PDO` and `PDOStatement` ask a database for,
//! independent of which one answers (php's `pdo_dbh_methods` /
//! `pdo_stmt_methods`).

/// A value crossing the boundary in either direction.
#[derive(Clone, Debug, PartialEq)]
pub enum SqlValue {
    Null,
    Int(i64),
    Float(f64),
    Text(Vec<u8>),
    Blob(Vec<u8>),
}

/// How a statement parameter is addressed: `?` by 1-based position, `:name`
/// by name (the prefix included).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParamKey {
    Pos(usize),
    Name(String),
}

/// One column of a result set.
#[derive(Clone, Debug)]
pub struct Column {
    pub name: String,
    /// The type the column was declared with (`INTEGER`), when known.
    pub decl_type: Option<String>,
    /// The table the column comes from, when known.
    pub table: Option<String>,
    /// The storage class of the first row's value (`integer`, `string`,
    /// `double`, `null`), php's `native_type`.
    pub native_type: Option<&'static str>,
}

/// A statement's result: every row, buffered.
#[derive(Debug, Default)]
pub struct ResultSet {
    pub columns: Vec<Column>,
    pub rows: Vec<Vec<SqlValue>>,
    /// `sqlite3_changes()` after the statement ran.
    pub changes: i64,
}

/// A database error, already mapped to php's SQLSTATE.
#[derive(Clone, Debug)]
pub struct DbError {
    pub sqlstate: String,
    /// The driver's own code (`sqlite3_errcode()`).
    pub code: i64,
    pub message: String,
}

impl DbError {
    pub fn new(sqlstate: &str, code: i64, message: impl Into<String>) -> DbError {
        DbError {
            sqlstate: sqlstate.to_string(),
            code,
            message: message.into(),
        }
    }
}

/// A connection.
pub trait Driver {
    /// `PDO::ATTR_DRIVER_NAME`.
    fn name(&self) -> &'static str;
    /// The concrete driver, for the driver-specific subclass's methods.
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any;
    /// Run SQL with no result (`PDO::exec()`): the number of rows changed
    /// by the last change statement.
    fn exec(&mut self, sql: &str) -> Result<i64, DbError>;
    /// Compile `sql` without running it (`PDO::prepare()`): the parameter
    /// names it declares, positional ones as `None`.
    fn prepare(&mut self, sql: &str) -> Result<Vec<Option<String>>, DbError>;
    /// Bind `params` to `sql`, run it and buffer the result.
    fn run(&mut self, sql: &str, params: &[(ParamKey, SqlValue)]) -> Result<ResultSet, DbError>;
    /// `PDO::lastInsertId()`.
    fn last_insert_id(&mut self, name: Option<&str>) -> Result<String, DbError>;
    fn begin(&mut self) -> Result<(), DbError>;
    fn commit(&mut self) -> Result<(), DbError>;
    fn rollback(&mut self) -> Result<(), DbError>;
    /// Whether the connection is inside a transaction it did not start
    /// itself (a `BEGIN` sent through `exec()`), which php reports too.
    fn in_transaction(&self) -> bool;
    /// `PDO::quote()`.
    fn quote(&self, s: &[u8]) -> Result<String, DbError>;
    /// `PDO::ATTR_SERVER_VERSION` (and `CLIENT_VERSION`).
    fn server_version(&self) -> String;
    /// A driver-specific attribute (`Pdo\Sqlite::ATTR_*`); `None` when the
    /// driver does not know it.
    fn get_attribute(&self, attr: i64) -> Option<rphp_value::Value>;
    /// Set a driver-specific attribute; `Ok(false)` when unknown.
    fn set_attribute(&mut self, attr: i64, value: &rphp_value::Value) -> Result<bool, DbError>;
}
