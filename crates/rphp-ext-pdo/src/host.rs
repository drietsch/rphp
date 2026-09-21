//! Drivers a host registers for DSN schemes this crate does not ship.
//!
//! php's `PDO` answers `mysql:` with libmysqlclient; an embedding host may
//! answer it with something else entirely — a router to another store, a
//! test double — without the engine growing a MySQL client. The host
//! registers a factory per scheme on the interpreter; `new PDO('mysql:…')`
//! and `PDO::connect('mysql:…')` find it before answering "could not find
//! driver". Everything above the [`Driver`](crate::driver::Driver) boundary
//! — error modes, fetch modes, parameter typing, `lastInsertId()`, the
//! SQLSTATE texts — is the same code that serves SQLite.

use std::collections::HashMap;

use rphp_runtime::Interp;
use rphp_value::Array;

use crate::driver::{DbError, Driver};

/// What a factory is given: the DSN split at its first colon, the
/// credentials, and the options array (`PDO::ATTR_*` and driver keys).
pub struct HostDsn<'a> {
    pub scheme: &'a str,
    /// Everything after `scheme:`, as the script wrote it.
    pub rest: &'a str,
    pub username: Option<&'a str>,
    pub password: Option<&'a str>,
    pub options: &'a Array,
}

/// Opens a connection for a DSN, or refuses with a `DbError` the script
/// sees as php's connection failure (`SQLSTATE[..] [code] message`).
pub type DriverFactory = Box<dyn Fn(&HostDsn<'_>) -> Result<Box<dyn Driver>, DbError>>;

/// The slot on `Interp::ext` the registry lives in.
pub const SLOT: &str = "pdo.host-drivers";

/// The registered factories, one per scheme.
#[derive(Default)]
pub struct HostDrivers {
    factories: HashMap<String, DriverFactory>,
}

impl HostDrivers {
    pub fn schemes(&self) -> Vec<&str> {
        let mut v: Vec<&str> = self.factories.keys().map(String::as_str).collect();
        v.sort_unstable();
        v
    }

    pub fn open(&self, dsn: &HostDsn<'_>) -> Option<Result<Box<dyn Driver>, DbError>> {
        self.factories.get(dsn.scheme).map(|f| f(dsn))
    }
}

/// Register `factory` for `scheme` on `it`; a later registration for the
/// same scheme replaces the earlier one.
pub fn register_host_driver(it: &mut Interp, scheme: &str, factory: DriverFactory) {
    let entry = it.ext.slots.entry(SLOT).or_insert_with(|| Box::new(HostDrivers::default()));
    if let Some(drivers) = entry.downcast_mut::<HostDrivers>() {
        drivers.factories.insert(scheme.to_owned(), factory);
    }
}

/// The registry on `it`, if any host driver was registered.
pub fn host_drivers(it: &Interp) -> Option<&HostDrivers> {
    it.ext.slots.get(SLOT).and_then(|b| b.downcast_ref::<HostDrivers>())
}

/// `PDO::connect()`'s subclass for a scheme: php 8.4's per-driver classes.
pub fn subclass_for(scheme: &str) -> &'static [u8] {
    match scheme {
        "sqlite" => b"Pdo\\Sqlite",
        "mysql" => b"Pdo\\Mysql",
        "pgsql" => b"Pdo\\Pgsql",
        _ => b"PDO",
    }
}

/// php's `PDO::MYSQL_ATTR_*` (and `Pdo\Mysql::ATTR_*`) values.
pub const MYSQL_ATTRS: &[(&str, i64)] = &[
    ("ATTR_USE_BUFFERED_QUERY", 1000),
    ("ATTR_LOCAL_INFILE", 1001),
    ("ATTR_INIT_COMMAND", 1002),
    ("ATTR_COMPRESS", 1003),
    ("ATTR_DIRECT_QUERY", 20),
    ("ATTR_FOUND_ROWS", 1004),
    ("ATTR_IGNORE_SPACE", 1005),
    ("ATTR_SSL_KEY", 1006),
    ("ATTR_SSL_CERT", 1007),
    ("ATTR_SSL_CA", 1008),
    ("ATTR_SSL_CAPATH", 1009),
    ("ATTR_SSL_CIPHER", 1010),
    ("ATTR_SERVER_PUBLIC_KEY", 1011),
    ("ATTR_MULTI_STATEMENTS", 1012),
    ("ATTR_SSL_VERIFY_SERVER_CERT", 1013),
    ("ATTR_LOCAL_INFILE_DIRECTORY", 1014),
];
