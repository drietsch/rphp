//! The ini table (`ini_get`/`ini_set`, `-d key=value`): registered directives
//! with their php-cli defaults under `-n`. Unknown directives are rejected
//! (`ini_set` returns `false`), as php does for unregistered names.

use std::collections::HashMap;

/// One directive: the current value and the registered default.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IniEntry {
    pub value: String,
    pub default: String,
}

/// The registered directives.
#[derive(Clone, Debug, Default)]
pub struct IniTable {
    entries: HashMap<String, IniEntry>,
}

/// php-src `main/main.c` defaults as `php -n` on the CLI reports them
/// (`ini_get`), for the directives the engine and the core extensions read.
/// (`display_errors`, `output_buffering`, `implicit_flush`,
/// `max_execution_time`, `html_errors`, `register_argc_argv` carry the CLI
/// SAPI's `INI_DEFAULT` overrides — see `sapi/cli/php_cli.c`.)
/// What the built-in web server (`php -S`, `cli-server`) sets differently
/// from the CLI: the `main.c` defaults the CLI overrides, as `php -n -S`
/// reports them.
pub const SERVER_DEFAULTS: &[(&str, &str)] = &[
    ("html_errors", "1"),
    ("implicit_flush", "0"),
    ("max_execution_time", "30"),
    ("register_argc_argv", "0"),
];

pub const CORE_DEFAULTS: &[(&str, &str)] = &[
    ("display_errors", "1"),
    ("display_startup_errors", "1"),
    ("log_errors", "0"),
    ("error_log", ""),
    ("error_reporting", ""),
    ("html_errors", "0"),
    ("xmlrpc_errors", "0"),
    ("ignore_repeated_errors", "0"),
    ("ignore_repeated_source", "0"),
    ("report_memleaks", "1"),
    ("docref_root", ""),
    ("docref_ext", ""),
    ("error_prepend_string", ""),
    ("error_append_string", ""),
    ("fatal_error_backtraces", "1"),
    ("output_buffering", "0"),
    ("output_handler", ""),
    ("upload_tmp_dir", ""),
    ("enable_post_data_reading", "1"),
    ("auto_globals_jit", "1"),
    ("implicit_flush", "1"),
    ("max_execution_time", "0"),
    ("max_input_time", "-1"),
    ("memory_limit", "128M"),
    ("register_argc_argv", "0"),
    ("precision", "14"),
    ("serialize_precision", "-1"),
    ("short_open_tag", "1"),
    ("zend.assertions", "1"),
    ("zend.enable_gc", "1"),
    ("expose_php", "1"),
    ("include_path", ".:/usr/share/php"),
    ("open_basedir", ""),
    ("disable_functions", ""),
    ("disable_classes", ""),
    ("default_charset", "UTF-8"),
    ("default_mimetype", "text/html"),
    ("date.timezone", "UTC"),
    ("variables_order", "EGPCS"),
    ("request_order", ""),
    ("arg_separator.output", "&"),
    ("arg_separator.input", "&"),
    ("auto_prepend_file", ""),
    ("auto_append_file", ""),
    ("auto_detect_line_endings", "0"),
    ("allow_url_fopen", "1"),
    ("allow_url_include", "0"),
    ("user_agent", ""),
    ("default_socket_timeout", "60"),
    ("file_uploads", "1"),
    ("post_max_size", "8M"),
    ("upload_max_filesize", "2M"),
    ("max_input_vars", "1000"),
    ("max_file_uploads", "20"),
    ("unserialize_callback_func", ""),
    ("sys_temp_dir", ""),
    ("assert.active", "1"),
    ("assert.exception", "1"),
    ("pcre.backtrack_limit", "1000000"),
    ("pcre.recursion_limit", "100000"),
    ("pcre.jit", "1"),
];

/// Push a directive the value layer reads without an interpreter to hand
/// (`precision`, which every float-to-string conversion uses) to where it
/// reads it.
fn sync(name: &str, value: &str) {
    if name == "precision" {
        rphp_value::set_float_precision(value.trim().parse().unwrap_or(0));
    }
}

impl IniTable {
    /// An empty table.
    pub fn new() -> IniTable {
        IniTable::default()
    }

    /// A table holding [`CORE_DEFAULTS`].
    pub fn with_core_defaults() -> IniTable {
        let mut t = IniTable::new();
        for (k, v) in CORE_DEFAULTS {
            t.register(k, v);
        }
        t
    }

    /// Register a directive with its default (an extension's `INI` block).
    /// Re-registering keeps the current value.
    pub fn register(&mut self, name: &str, default: &str) {
        let fresh = !self.entries.contains_key(name);
        if fresh {
            sync(name, default);
        }
        self.entries
            .entry(name.to_string())
            .and_modify(|e| e.default = default.to_string())
            .or_insert_with(|| IniEntry {
                value: default.to_string(),
                default: default.to_string(),
            });
    }

    /// Whether `name` is a registered directive.
    pub fn contains(&self, name: &str) -> bool {
        self.entries.contains_key(name)
    }

    /// The current value (`ini_get`); `None` for an unknown directive.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.entries.get(name).map(|e| e.value.as_str())
    }

    /// Set a directive (`ini_set`), returning the previous value; `None`
    /// (and no change) for an unknown directive.
    pub fn set(&mut self, name: &str, value: &str) -> Option<String> {
        let e = self.entries.get_mut(name)?;
        sync(name, value);
        Some(std::mem::replace(&mut e.value, value.to_string()))
    }

    /// Reset a directive to its default (`ini_restore`).
    pub fn restore(&mut self, name: &str) {
        if let Some(e) = self.entries.get_mut(name) {
            e.value = e.default.clone();
            sync(name, &e.value);
        }
    }

    /// The directive parsed as php parses boolean ini values (`1`, `on`,
    /// `yes`, `true` ⇒ true; anything else, including unknown ⇒ false).
    pub fn bool(&self, name: &str) -> bool {
        self.get(name).is_some_and(parse_bool)
    }

    /// The directive parsed as an integer (`0` when unknown / unparsable).
    pub fn int(&self, name: &str) -> i64 {
        self.get(name)
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(0)
    }

    /// Every directive, in no particular order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &IniEntry)> {
        self.entries.iter().map(|(k, v)| (k.as_str(), v))
    }
}

/// php's `zend_ini_parse_bool`: `true`, `yes`, `on`, and any non-zero integer
/// are true (case-insensitive); everything else is false.
pub fn parse_bool(s: &str) -> bool {
    let t = s.trim();
    if t.eq_ignore_ascii_case("true")
        || t.eq_ignore_ascii_case("yes")
        || t.eq_ignore_ascii_case("on")
    {
        return true;
    }
    t.parse::<i64>().is_ok_and(|n| n != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_get_set_and_unknown() {
        let mut t = IniTable::with_core_defaults();
        assert_eq!(t.get("display_errors"), Some("1"));
        assert!(t.bool("display_errors"));
        assert!(!t.bool("log_errors"));
        assert_eq!(t.set("precision", "10"), Some("14".to_string()));
        assert_eq!(t.get("precision"), Some("10"));
        assert_eq!(t.int("precision"), 10);
        assert_eq!(t.set("nope.x", "1"), None);
        assert_eq!(t.get("nope.x"), None);
        t.restore("precision");
        assert_eq!(t.get("precision"), Some("14"));
    }

    #[test]
    fn bool_parsing_matches_php() {
        for yes in ["1", "On", "yes", "TRUE", "2"] {
            assert!(parse_bool(yes), "{yes}");
        }
        for no in ["0", "", "off", "false", "no", "none", "stderr"] {
            assert!(!parse_bool(no), "{no}");
        }
    }
}
