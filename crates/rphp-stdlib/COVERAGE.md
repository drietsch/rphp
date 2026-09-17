# stdlib coverage — burn-down dashboard

Tracks the per-extension parity burn-down (`specs/base/08-stdlib-ext.md`, ADR-004).
The oracle is differential testing against stock **PHP 8.5**: each extension ships a
`examples/tier-a/<ext>.php` snippet that exercises its functions, run through both rphp
and `php` by `crates/rphp-sapi-cli/tests/differential.rs` and required to match
byte-for-byte (must-not-regress).

**Registry size: 218 rows** (wave 1: ~103 distinct functions; wave 2: +11 by-reference
builtins; wave 3: +9 higher-order/callable builtins; E1: +36 engine-facing builtins —
`ob_*`, ini/constants, error handling). Plus aliases and the initial slice.

## Engine gaps that bound the burn-down

Functions needing a missing language feature are **cataloged, not faked** (decision: skip
& catalog). They unblock as the language slice grows:

- **by-reference parameters** — ✅ *native* by-ref done (the ABI copies a builtin's
  mutated arg slots back into the caller's variable; `rphp_runtime::nf_ref!` +
  `NativeFn::by_ref`). User-defined `function f(&$x)` is still pending (needs parser/AST).
- **callables** — ✅ function-name strings **and closures/arrow functions**: native
  functions re-enter the engine via `ctx.call_value` (`rphp_runtime::Interp::call_value`,
  ADR-014: natives receive `Ctx(&mut Interp)`), which dispatches a `Value::Closure` to
  `exec_closure` or resolves a callable string to a user function
  (`Module::func_by_name`) / builtin.
  Closures capture by value (`use (...)` / arrow auto-capture). **Still pending:**
  `[$obj, 'method']` arrays, first-class `strlen(...)`, by-reference `use (&$x)`.
- **objects** — ✅ *core* objects/classes landed: class declarations, properties with
  constant defaults, constructors, methods, `$this`, method chaining, reference
  semantics, identity (`===`), **single inheritance** (`extends`) with virtual dispatch
  and inherited constructors, **`parent::`/`self::`/`Class::` scoped calls**,
  **`instanceof`**, and **runtime-enforced `public`/`protected`/`private` visibility**.
  `json_encode` serializes an instance's *public* properties only. **Still pending:**
  interfaces/traits, static members & class constants (`::CONST`/`::$p`), magic methods,
  `clone`, dynamic `new $cls` / `$obj instanceof $cls`, `[$obj,'method']` callables,
  `json_decode`'s default stdClass form, SPL, DateTime.

## Implemented (Tier-A wave 1)

| Extension | Implemented | Notes |
|-----------|-------------|-------|
| output (ob_*) | 14 | E1: `ob_start` (callback + php phase flags), `ob_get_clean/contents/flush`, `ob_end_clean/flush`, `ob_get_level/length/status`, `ob_flush/clean`, `flush`, `ob_implicit_flush`, `ob_list_handlers`; streaming sink, php's no-buffer notices |
| errorfunc | 10 | E1: `set/restore_error_handler`, `set/restore_exception_handler` (stored; invoked in E5), `error_reporting`, `trigger_error`/`user_error` (incl. the 8.4 E_USER_ERROR deprecation + fatal), `error_get_last`, `error_clear_last`, `error_log` (stderr / file) |
| basic_functions | 12 | E1: `ini_get/ini_set/ini_alter/ini_restore`, `define/defined/constant`, `register_shutdown_function`, `php_sapi_name`, `phpversion`, `function_exists`, `set_time_limit` (no-op) |
| ctype     | 11 | all `ctype_*`; ASCII (C-locale) classification; integer special-case matched |
| math      | 36 | trig/exp/log, `pow` (8.4 zero-base deprecation), `fpow`, `log1p`, `hypot`, `fmod`, `fdiv`, `is_nan/finite/infinite`, **`round`** (php's pre-rounding algorithm, negative precision, `PHP_ROUND_HALF_*` + the 8.4 `RoundingMode` values 1–8 as ints, overflow / huge-precision guards), **`base_convert`**, base conversions (`dechex`…`octdec` with whitespace / `0x`/`0o`/`0b` stripping and the invalid-character deprecation). S2 tier-a: `math/{functions,round,base}.php` |
| type (types.rs) | 24 | `gettype`, **`get_debug_type`**, `is_*` incl. **`is_iterable`/`is_countable`/`is_callable`** (strings, closures, `[$obj,'m']`; `'C::m'` forms need static methods → syntax-only), **`settype`** (by-ref; `object` needs stdClass), `intval` (`strtol` bases, `0b` handling, invalid base → 0), `floatval`, `strval`, `boolval`. S2 tier-a: `var/types.php` |
| var (var.rs) | 6 | S2: **`var_export`** (all types; `PHP_INT_MIN` expression form, `serialize_precision` float layout with `.0`, `'\0'` splicing, nested `array (` indentation, `\Class::__set_state(array(` / `(object) array(` / `\Closure::__set_state`, circular-reference warning, `$return`), **`serialize`** (all scalar forms, `d:` floats, mangled `\0*\0`/`\0Class\0` names from the instance layout, `r:`/`R:` back-references with php's slot numbering, closures throw, resources `i:0;`), **`unserialize`** (a transcription of `var_unserializer.re`: `N b i d s S a O C E r R`, `parse_iv` overflow warning, `S:` deprecation, references incl. ancestor cells (`$a[0] = &$a`), repeated keys replacing slots, `allowed_classes` / `max_depth` options with their `TypeError`/`ValueError`s, php's `Error at offset` / `Extra data` / `Unexpected end` / `Maximum depth` / `Insufficient data` texts at php's offsets; objects of declared classes only), `memory_get_usage` family (fixed plausible figures — `platform-value`). Output floats: `var_dump`/`var_export`/`serialize` share `output::php_gcvt` (`serialize_precision` −1 / N). S2 tier-a: `var/{export,serialize,unserialize}.php` |
| url (url.rs) | 7 | S2: **`parse_url`** (transcription of `php_url_parse_ex2`, byte-identical on php-src's `urls.inc` corpus incl. the malformed set, component form, `ValueError`), `urlencode`/`rawurlencode`/`urldecode`/`rawurldecode`, **`http_build_query`** (nesting, numeric prefix, separators incl. `arg_separator.output`, RFC 1738/3986, objects' public props), **`parse_str`** (`php_register_variable_ex`: `[]`/`[k]` nesting, `.`/space → `_`, scalar↔array replacement, `max_input_vars` warning). Constants `PHP_URL_*`, `PHP_QUERY_*`. S2 tier-a: `url/{parse-url,encode,query}.php` |
| info (info.rs) | 30 | S2: `php_uname` (`uname(1)` once + std fallbacks), `phpinfo` (text: general + ini table), `php_ini_loaded_file`/`php_ini_scanned_files`/`get_cfg_var` (`false`: no php.ini, no `-d`), `ini_get_all` (sorted, details, per-extension via a manifest-derived access/extension table), `get_include_path`/`set_include_path`, `getenv`/`putenv` (process env, `ValueError` on bad syntax), `sys_get_temp_dir`, `getmypid`/`getmyuid`/`getmygid`/`getmyinode`/`getlastmod` (script metadata), `get_current_user` (`/etc/passwd` → `id -un` → env), `gc_enable/disable/enabled/collect_cycles/mem_caches/status`, `zend_version` (`4.5.0`), `extension_loaded`/`get_loaded_extensions`/`get_extension_funcs` (the bundle: Core, pcre, ctype, json, hash, standard, random — Core membership from the manifest), `connection_status`/`connection_aborted`/`ignore_user_abort`; `phpversion($ext)` now answers for bundled extensions. Constants `INI_*`, `CONNECTION_*`, `INFO_*`, `CREDITS_*`. S2 tier-a: `info/runtime.php` |
| versioning | 1 | S2: **`version_compare`** — transcription of `php_canonicalize_version` + `php_version_compare` (special forms, `#N#` recursion, operator arg + `ValueError`). S2 tier-a: `info/version-compare.php` |
| uniqid / microtime / hrtime | 8 | S2: `uniqid` (spins until the microsecond changes; `more_entropy`), `microtime`, `gettimeofday`, `hrtime` (monotonic, process-relative), `time` (interim home until `date.rs`, S6), `sleep`, `usleep`, `time_nanosleep` |
| random (random.rs) | 12 | S2: php's exact **Mt19937** (`php_mt_initialize`/`php_mt_reload`, `>> 1`, `rand_range32`/`rand_range64` rejection sampling with php's low-word-first assembly, `MT_RAND_PHP` legacy twist + `RAND_RANGE_BADSCALING` + its deprecation): `mt_srand`/`srand`, `mt_rand`/`rand`, `mt_getrandmax`/`getrandmax`; CSPRNG `random_int`/`random_bytes` (`/dev/urandom`); `lcg_value` (8.4 deprecation); and the engine-backed `shuffle`, `str_shuffle`, `array_rand` (php's Fisher–Yates / bitset walks) — seeded sequences byte-identical (`mt_srand(42); mt_rand()` = 804318771). Constants `MT_RAND_MT19937`/`MT_RAND_PHP`. State is a thread-local until `ExtState` grows a slot. S2 tier-a: `random/mt.php` |
| string    | 29 | `sprintf`/`printf`/`vsprintf`, `number_format`, `str_pad`, `str_split`, `strtr`, `strcmp` family, `bin2hex`/`hex2bin`, … |
| array     | 18 + 11 + 6 | value-returning: `array_slice/flip/unique/diff/intersect/combine/chunk/column/fill/pad/search/product/…`, `array_is_list`. **By-ref:** `sort/rsort/asort/arsort/ksort/krsort`, `array_push/pop/shift/unshift`, `array_splice`. **Higher-order:** `array_map`, `array_filter`, `array_reduce`, `usort/uasort/uksort` |
| funcs     | 2  | `call_user_func`, `call_user_func_array` |
| json      | 2  | `json_encode` (incl. `JSON_PRETTY_PRINT`/`UNESCAPED_SLASHES`/`UNESCAPED_UNICODE`; objects → public-property objects), `json_decode` |
| hash      | 5  | `md5`, `sha1`, `crc32`, `hash` (md5/sha1/sha256/sha384/sha512/crc32b), `hash_algos` |
| pcre      | 5 + 1 | `preg_quote`, `preg_match` (incl. by-ref `$matches`), `preg_replace`, **`preg_replace_callback`**, `preg_split`, `preg_grep` — over PCRE2 |

## Deferred (cataloged, by blocker)

**by-reference params (user-defined `&$x` / remaining out-params):** `shuffle` (also needs
RNG), `preg_match_all`, `preg_replace` ($count out-param), `preg_filter`.

**callables — closures/method-arrays (need parser + value types):** anything passed a
`function(){}` / `fn()=>` / `[$obj,'m']` callable. `array_walk` also needs a by-ref
callback param. Partial: `array_map` (multi-array form), `array_filter` (USE_KEY/BOTH
modes), `preg_replace_callback` (`$limit`/`$count`), `preg_replace_callback_array`.

**objects / resources:** `hash_init` `hash_update` `hash_final` `hash_copy`
(HashContext); `json_decode` default (stdClass — currently decodes to an array).

**filesystem I/O:** `hash_file` `hash_update_file` `hash_hmac_file`.

**stateful (no per-request slot in `Ctx` yet):** `preg_last_error` `preg_last_error_msg`.

**pure, simply not yet done (next wave, no blocker):** `hash_hmac` `hash_pbkdf2`
`hash_equals`; string `wordwrap`; array `array_replace_recursive` / `array_merge_recursive`.

## Deferred by S2 (var / type / url / info half)

**magic serialization hooks (E6 — magic methods):** `__sleep`, `__serialize`, `__wakeup`,
`__unserialize`, `Serializable` cannot be invoked; `serialize`/`unserialize` of a class
declaring one throws an `Error` naming the hook rather than silently producing the wrong
bytes. `C:` (custom data) yields php's "has no unserializer" warning + a bare instance for
a declared class.

**classes the program does not declare (E6 — class model):** `unserialize` of `stdClass`,
`__PHP_Incomplete_Class` (unknown / disallowed classes), enum cases (`E:`) returns `false`
with php's `Error at offset` warning (enums additionally get php's "not an enum" / "not
found" warning). `settype($v, "object")` throws (needs stdClass). Private properties that
shadow a parent's private of the same name share one slot (engine layout).

**static methods (E6):** `is_callable('C::m')` / `is_callable(['C', 'm'])` are only true
with `$syntax_only` — no static method exists in the class model yet.

**engine-owned:** `debug_zval_dump` (refcounts are engine-internal), `debug_print_backtrace`
(E5), `getrusage` (needs libc), `get_headers` (network streams, S12), `phpcredits`.

**value crate:** `pow(PHP_INT_MIN, PHP_INT_MAX)` should be `-INF` (php's overflow path
keeps the sign of the partial product; `Value::pow` does not) — `pow_basiclong_64bit.phpt`.

## Known divergences (documented)

- `var_dump`/`print_r` of an array that references *itself* (`$a[0] = &$a`) prints one
  nesting level before `*RECURSION*` where php prints none: php protects the array, the
  safe heap can only observe the shared reference cell. Objects and reference cells in
  every other position recurse-guard exactly as php.
- `memory_get_usage`/`memory_get_peak_usage` return fixed plausible figures (no allocator
  accounting yet) — `platform-value`.
- `hrtime` is relative to the first call (std has no raw `CLOCK_MONOTONIC`); differences
  are exact — `timing`.
- `get_cfg_var` is always `false`: there is no php.ini and the CLI takes no `-d` yet.
- `ini_get_all()` reports the registered directives only (~60 of php's 288), with php's
  access masks for those.

- `json_decode` returns an **array** for JSON objects regardless of `$assoc` (no object type yet).
- ctype on bytes ≥ 128 is ASCII/C-locale only (stock php here is C.UTF-8 and classifies some high bytes differently); bytes 0–127 match exactly.
- PHP 8.4+ `E_DEPRECATED`/warning notices (non-string ctype args, invalid base chars, NAN-to-string) are not emitted — the engine has no warning channel yet.
