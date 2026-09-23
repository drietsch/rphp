//! The standard library as a bundle of native-function modules
//! (`specs/base/08-stdlib-ext.md`, ADR-014): one module per php-src source
//! file, each exposing a `FUNCTIONS` slice of [`NativeFn`] descriptors, all
//! registered into an interpreter through [`register`]. The engine
//! (`rphp-runtime`) sits *below* this crate: natives receive a
//! [`Ctx`](rphp_runtime::Ctx) and return `Result<Value, Unwind>`.
//!
//! Adding a function is one table row in its module; adding an extension is
//! one module in [`MODULES`]. Tooling (`xtask missing`) walks
//! [`all_functions`] to compare against the PHP manifest.
#![forbid(unsafe_code)]

use rphp_runtime::{NativeFn, Registry};

mod array2;
mod arrays;
mod base64;
mod basic_functions;
mod closure_class;
mod ctype;
mod date;
mod dechunk;
mod dir;
mod errorfunc;
mod exec;
mod file;
mod file2;
mod filestat;
mod filters;
mod filter;
mod formatted_print;
mod funcs;
mod hash;
mod head;
mod html;
mod http;
mod iconv;
mod info;
mod json;
mod libmbfl;
mod math;
mod mbstring;
mod net;
mod output;
mod output_buffering;
mod pack;
mod password;
mod pcre;
mod random;
mod randomizer;
mod session;
mod socket;
mod tls;
mod tokenizer;
mod highlight;
mod reflection;
mod spl_autoload;
mod spl_containers;
mod spl_containers2;
mod spl_decorators;
mod spl_directory;
mod spl_exceptions;
mod spl_fixedarray;
mod spl_heaps;
mod spl_interfaces;
mod spl_iterators;
mod string2;
mod strings;
mod types;
mod syslog;
mod uniqid;
mod url;
mod user_filters;
mod var;
mod versioning;
mod weak;
mod zend_exceptions;
mod zlib;

/// Every module's `FUNCTIONS` slice, in registration order.
const MODULES: &[&[NativeFn]] = &[
    filter::FUNCTIONS,
    date::FUNCTIONS,
    date::CLASS_FUNCTIONS,
    reflection::FUNCTIONS,
    spl_containers2::FUNCTIONS,
    password::FUNCTIONS,
    exec::FUNCTIONS,
    file2::FUNCTIONS,
    head::FUNCTIONS,
    spl_decorators::FUNCTIONS,
    spl_directory::FUNCTIONS,
    spl_fixedarray::FUNCTIONS,
    spl_heaps::FUNCTIONS,
    spl_iterators::FUNCTIONS,
    spl_autoload::FUNCTIONS,
    output::FUNCTIONS,
    types::FUNCTIONS,
    strings::FUNCTIONS,
    arrays::FUNCTIONS,
    math::FUNCTIONS,
    net::FUNCTIONS,
    socket::FUNCTIONS,
    http::FUNCTIONS,
    filters::FUNCTIONS,
    user_filters::FUNCTIONS,
    zlib::FUNCTIONS,
    randomizer::FUNCTIONS,
    session::FUNCTIONS,
    tokenizer::FUNCTIONS,
    highlight::FUNCTIONS,
    ctype::FUNCTIONS,
    funcs::FUNCTIONS,
    hash::FUNCTIONS,
    json::FUNCTIONS,
    pcre::FUNCTIONS,
    output_buffering::FUNCTIONS,
    basic_functions::FUNCTIONS,
    errorfunc::FUNCTIONS,
    string2::FUNCTIONS,
    array2::FUNCTIONS,
    var::FUNCTIONS,
    url::FUNCTIONS,
    info::FUNCTIONS,
    versioning::FUNCTIONS,
    formatted_print::FUNCTIONS,
    html::FUNCTIONS,
    base64::FUNCTIONS,
    uniqid::FUNCTIONS,
    syslog::FUNCTIONS,
    random::FUNCTIONS,
    mbstring::FUNCTIONS,
    iconv::FUNCTIONS,
    pack::FUNCTIONS,
    file::FUNCTIONS,
    filestat::FUNCTIONS,
    dir::FUNCTIONS,
];

/// Register every module's functions and constants into an interpreter.
pub use date::bridge as date_bridge;
pub use exec::{SpawnPolicy, SpawnRequest, POLICY_SLOT as SPAWN_POLICY_SLOT};
pub use file::{stream_fd, StreamFd};
pub use url::{parse_query, register_variable};
pub use weak::new_internal_iterator;

pub fn register(r: &mut Registry) {
    for m in MODULES {
        r.functions(m);
    }
    zend_exceptions::register_classes(r);
    spl_exceptions::register_classes(r);
    spl_interfaces::register_classes(r);
    closure_class::register_classes(r);
    weak::register_classes(r);
    hash::register_classes(r);
    spl_containers::register_classes(r);
    spl_containers2::register_classes(r);
    spl_fixedarray::register_classes(r);
    spl_heaps::register_classes(r);
    spl_decorators::register_classes(r);
    spl_directory::register_classes(r);
    session::register_classes(r);
    tokenizer::register_classes(r);
    date::register_classes(r);
    reflection::register_classes(r);
    randomizer::register_classes(r);
    user_filters::register_classes(r);
    zlib::register_classes(r);
    math::register_constants(r);
    net::register_constants(r);
    session::register_constants(r);
    tokenizer::register_constants(r);
    pcre::register_constants(r);
    json::register_constants(r);
    hash::register_constants(r);
    output_buffering::register_constants(r);
    string2::register_constants(r);
    array2::register_constants(r);
    var::register_constants(r);
    random::register_constants(r);
    url::register_constants(r);
    info::register_constants(r);
    mbstring::register_constants(r);
    iconv::register_constants(r);
    pack::register_constants(r);
    file::register_constants(r);
    exec::register_constants(r);
    file2::register_constants(r);
    head::register_constants(r);
    head::register_server_functions(r);
    highlight::register_ini(r);
    date::register_constants(r);
    filter::register_constants(r);
    reflection::register_constants(r);
    spl_containers2::register_constants(r);
    spl_fixedarray::register_constants(r);
    spl_heaps::register_constants(r);
    spl_decorators::register_constants(r);
    spl_directory::register_constants(r);
    password::register_constants(r);
    filestat::register_constants(r);
    dir::register_constants(r);
    syslog::register_constants(r);
    user_filters::register_constants(r);
    zlib::register_constants(r);
}

/// php's per-request module shutdown (`RSHUTDOWN`) for the state this
/// crate keeps per thread: a server's worker thread lives across requests,
/// and the next one must not see this one's session, `mb_*` settings,
/// default timezone, `strtok` cursor, locale or `mt_rand` seed. Runs after
/// the output is flushed, as php's does.
pub fn request_shutdown(it: &mut rphp_runtime::Interp) {
    session::request_shutdown(it);
    mbstring::request_shutdown();
    string2::request_shutdown();
    date::funcs::request_shutdown();
    random::request_shutdown();
    http::request_shutdown();
}

/// Every native this crate provides (for tooling: coverage, `xtask missing`).
pub fn all_functions() -> impl Iterator<Item = &'static NativeFn> {
    MODULES.iter().flat_map(|m| m.iter())
}

#[cfg(test)]
mod tests {
    use rphp_runtime::{Interp, Registry, Unwind};
    use rphp_value::Value;

    /// A test interpreter with the whole stdlib registered.
    pub(crate) fn interp() -> Interp {
        let mut it = Interp::new_for_tests();
        super::register(&mut Registry(&mut it));
        it
    }

    /// Call a builtin by name with the given args, returning its result.
    pub(crate) fn call_named(name: &[u8], args: &[Value]) -> Value {
        interp().call_function(name, args).unwrap()
    }

    /// Call a builtin by name expecting a fault.
    pub(crate) fn call_err(name: &[u8], args: &[Value]) -> Unwind {
        interp().call_function(name, args).unwrap_err()
    }

    pub(crate) fn arr(items: &[Value]) -> Value {
        let mut a = rphp_value::Array::new();
        for v in items {
            a.push(v.clone());
        }
        Value::Array(a)
    }

    #[test]
    fn resolve_is_case_insensitive() {
        let it = interp();
        assert!(it.native_by_name(b"strlen").is_some());
        assert!(it.native_by_name(b"STRLEN").is_some());
        assert!(it.native_by_name(b"StrLen").is_some());
        assert!(it.native_by_name(b"no_such_function").is_none());
    }

    #[test]
    fn all_functions_are_registered_and_unique() {
        let it = interp();
        let mut names: Vec<String> = super::all_functions()
            .map(|f| f.name.to_ascii_lowercase())
            .collect();
        let n = names.len();
        names.sort();
        names.dedup();
        assert_eq!(n, names.len(), "duplicate native names");
        assert_eq!(it.natives().len(), n);
        assert!(n > 180);
    }

    #[test]
    fn substr_negative_length_trims_the_tail() {
        let s = Value::string(b"abcdef");
        assert_eq!(
            call_named(b"substr", &[s.clone(), Value::Int(1), Value::Int(-1)]),
            Value::string(b"bcde")
        );
        assert_eq!(
            call_named(b"substr", &[s.clone(), Value::Int(-2)]),
            Value::string(b"ef")
        );
        assert_eq!(
            call_named(b"substr", &[s, Value::Int(0), Value::Int(-10)]),
            Value::string(b"")
        );
    }

    #[test]
    fn explode_respects_a_positive_limit() {
        let parts = call_named(
            b"explode",
            &[
                Value::string(b","),
                Value::string(b"a,b,c,d"),
                Value::Int(2),
            ],
        );
        assert_eq!(parts, arr(&[Value::string(b"a"), Value::string(b"b,c,d")]));
    }

    #[test]
    fn range_descends_and_supports_floats() {
        assert_eq!(
            call_named(b"range", &[Value::Int(3), Value::Int(1)]),
            arr(&[Value::Int(3), Value::Int(2), Value::Int(1)])
        );
        assert_eq!(
            call_named(b"range", &[Value::Int(0), Value::Int(1), Value::Float(0.5)]),
            arr(&[Value::Float(0.0), Value::Float(0.5), Value::Float(1.0)])
        );
    }

    #[test]
    fn str_repeat_rejects_negative_counts_with_a_value_error() {
        let err = call_err(b"str_repeat", &[Value::string(b"x"), Value::Int(-1)]);
        assert_eq!(err.kind(), Some(rphp_runtime::ErrorKind::ValueError));
    }

    #[test]
    fn intdiv_by_zero_is_a_division_by_zero_error() {
        let err = call_err(b"intdiv", &[Value::Int(1), Value::Int(0)]);
        assert_eq!(
            err.kind(),
            Some(rphp_runtime::ErrorKind::DivisionByZeroError)
        );
        assert_eq!(err.message(), Some("Division by zero"));
    }

    #[test]
    fn natives_invoke_native_callbacks() {
        // array_map with a builtin callable re-enters through the interpreter.
        let mapped = call_named(
            b"array_map",
            &[
                Value::string(b"strtoupper"),
                arr(&[Value::string(b"a"), Value::string(b"b")]),
            ],
        );
        assert_eq!(mapped, arr(&[Value::string(b"A"), Value::string(b"B")]));
    }

    #[test]
    fn max_min_over_array_and_args() {
        assert_eq!(
            call_named(b"max", &[Value::Int(3), Value::Int(9), Value::Int(2)]),
            Value::Int(9)
        );
        assert_eq!(
            call_named(
                b"min",
                &[arr(&[Value::Int(4), Value::Int(1), Value::Int(8)])]
            ),
            Value::Int(1)
        );
    }

    #[test]
    fn aliases_share_an_implementation() {
        let a = arr(&[Value::Int(1), Value::Int(2)]);
        assert_eq!(
            call_named(b"count", std::slice::from_ref(&a)),
            Value::Int(2)
        );
        assert_eq!(call_named(b"sizeof", &[a]), Value::Int(2));
    }

    #[test]
    fn var_dump_writes_to_the_output_layer() {
        let mut it = interp();
        it.call_function(b"var_dump", &[Value::Int(1)]).unwrap();
        assert_eq!(it.take_test_output(), b"int(1)\n");
        // With a level active the output is captured there instead.
        it.ob_start(None, 0, rphp_runtime::PHP_OUTPUT_HANDLER_STDFLAGS);
        it.call_function(b"print_r", &[Value::string(b"x")])
            .unwrap();
        assert_eq!(it.take_test_output(), b"");
        assert_eq!(it.ob_discard_top().unwrap().unwrap(), b"x");
    }
}
