//! `ext/pdo` and `ext/pdo_sqlite` (S14, ADR-035): the `PDO`, `Pdo\Sqlite`,
//! `PDOStatement`, `PDOException` and `PDORow` classes over a driver
//! boundary ([`driver::Driver`]), with SQLite compiled in through rusqlite.
//!
//! What php decides, this crate decides the same way: the error modes and
//! every `SQLSTATE[..]: description: code message` text, the fetch modes
//! and their flags, the way parameters are typed on binding, what
//! `rowCount()`/`exec()` count, what `lastInsertId()` answers, php 8.4's
//! driver-specific subclass (`Pdo\Sqlite`, from `PDO::connect()`), and the
//! 8.5 deprecations of the `PDO::sqlite*()` methods and `PDO::SQLITE_*`
//! constants. Result sets are buffered whole at `execute()` time — SQLite
//! steps lazily, php over it too, but nothing observable depends on it
//! short of memory.
//!
//! `bridge.rs` holds the crate's one `unsafe` corner (SQLite's synchronous
//! callbacks need the interpreter); `COVERAGE.md` records it.
#![deny(unsafe_op_in_unsafe_fn)]

mod bridge;
pub mod driver;
pub mod host;
mod pdo;
mod sqlite;
mod sqlstate;
mod stmt;

pub use host::{register_host_driver, DriverFactory, HostDsn};
pub use pdo::register;
