# stdlib coverage — burn-down dashboard

Tracks the per-extension parity burn-down (`specs/base/08-stdlib-ext.md`, ADR-004).
The oracle is differential testing against stock **PHP 8.5**: each extension ships a
`examples/tier-a/<ext>.php` snippet that exercises its functions, run through both rphp
and `php` by `crates/rphp-sapi-cli/tests/differential.rs` and required to match
byte-for-byte (must-not-regress).

**Registry size: 621 functions and 106 classes + 16 interfaces** (what a fresh
CLI interpreter answers with, which `cargo xtask missing` now reads directly
instead of reporting every internal class and constant as absent). The waves so
far: the S2 string/array/var/url/info halves, S3 filesystem + streams, S4 SPL,
S5 Reflection, S6 date, S8 mbstring/iconv, S9 password/crypt, S10 filter.

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
| string (`strings.rs` + `string2.rs`) | 48 + 27 | S2: the whole of `string.c` except the deferrals below — `str_replace`/`str_ireplace` (`&$count`, array forms), `substr_replace` array forms, `strrchr($before_needle)`, `trim` family with `a..z` ranges and php's range warnings, `str_word_count` char lists, `number_format` negative decimals + exact digits, `addcslashes`/`stripcslashes`, `chunk_split`, `count_chars`, `dirname`/`basename`/`pathinfo`, `levenshtein`, `similar_text(&$percent)`, `soundex`, `metaphone`, `str_getcsv` (8.4 `$escape` deprecation), `strip_tags` (string/array allow lists), `wordwrap`, `sscanf` (full format grammar incl. `%[…]`, `%n`, `%n$`, `&...$vars`, every `ValueError` text), `strtok`, `strnatcmp`/`strnatcasecmp`, `strspn`/`strcspn`, `substr_compare`, `str_rot13`, `strcoll`, `utf8_encode`/`utf8_decode` (8.2 deprecation), `setlocale`/`localeconv` (C-locale model), `chop`, 8.5 `chr`/`ord` deprecations, `hex2bin` warnings, PHP 8 `implode` errors, `strpos` offset `ValueError` |
| formatted_print | 4 | `sprintf`/`printf`/`vsprintf`/`vprintf`: every conversion `b c d e E f F g G h H o s u x X`, positional `%n$`, `*` width/precision (incl. `-1` shortest `%g`), `'x` padding, php's per-conversion padding rules, `%c`/NaN/Inf unpadded, the 53-digit precision notice, `ArgumentCountError`/`ValueError` texts, the `% %` and `%.4o` quirks |
| html      | 5 | `htmlspecialchars`/`htmlspecialchars_decode`/`htmlentities`/`html_entity_decode`/`get_html_translation_table`: HTML 4.01/XHTML/XML1/HTML5 tables (2125-name HTML5 decode table, 65 two-code-point entities), all `ENT_*` flags, `$double_encode`, php's numeric-entity validity rules per doctype, malformed-UTF-8 charging, charsets UTF-8/ISO-8859-1/-15/-5/cp1252/cp1251/KOI8-R/cp866/MacRoman (exact) and BIG5/GB2312/SJIS/EUC-JP (structure validation, basic entities only, as php) |
| base64    | 6 | `base64_encode`/`base64_decode` (strict rules), `quoted_printable_encode`/`decode`, `convert_uuencode`/`convert_uudecode` |
| array (`arrays.rs` + `array2.rs`) | 45 + 35 | S2: `array.c` except the deferrals — every sort with all `SORT_*` flags on php's own `zend_sort` (element order matches php even for mixed types), `natsort`/`natcasesort`, `usort` family incl. the deprecated `bool` return, `array_multisort` (1–n arrays, orders, flags), `array_unique` flags, `array_keys` search, `array_map` (n arrays, `null` callback), `array_filter` modes, `count(COUNT_RECURSIVE)` with recursion detection, `array_sum`/`array_product` 8.3 warnings, `range()` with the 8.3 rules and texts, the key/assoc/callback set operations (`array_diff_key/assoc/ukey/uassoc`, `array_udiff*`, `array_intersect_*`, `array_uintersect*`), `array_merge_recursive`, `array_replace_recursive`, `array_change_key_case`, `key_exists`, `array_find`/`array_find_key`/`array_any`/`array_all`, `array_first`/`array_last` (8.5), `current`/`key`/`pos`/`next`/`prev`/`reset`/`end`, `array_walk_recursive` (elements as reference cells), reference elements preserved php-style through copies |
| funcs     | 2  | `call_user_func`, `call_user_func_array` |
| json      | 2  | `json_encode` (incl. `JSON_PRETTY_PRINT`/`UNESCAPED_SLASHES`/`UNESCAPED_UNICODE`; objects → public-property objects), `json_decode` |
| hash      | 5  | `md5`, `sha1`, `crc32`, `hash` (md5/sha1/sha256/sha384/sha512/crc32b), `hash_algos` |
| pcre      | 5 + 1 | `preg_quote`, `preg_match` (incl. by-ref `$matches`), `preg_replace`, **`preg_replace_callback`**, `preg_split`, `preg_grep` — over PCRE2 |
| file (`file.rs`) | 34 | S3: the path functions (`file_get_contents`/`file_put_contents`/`file`/`copy`/`rename`/`unlink`/`readfile`/`tempnam`/`realpath`…) plus the **stream handle family** — `fopen` (real files, `php://memory`/`temp`/`stdout`/`stderr`/`output`), `fread`/`fgets`/`fgetc`/`fwrite`/`fputs`/`stream_get_contents`, `fseek`/`ftell`/`rewind`/`fflush`/`fclose`, and `feof` over php's *end-of-file flag* (set by a read that came back short, cleared by a seek — not "the cursor is at the end"). The mode is enforced the way php's fd does: a read on a write-only file, or a write on a read-only one, is the `Notice: … failed with errno=9 Bad file descriptor` and `false`. S3 tier-a: `file/streams.php` |
| filestat (`filestat.rs`) | 18 | S3: the stat predicates over php's per-request stat cache (`file_exists`, `is_file`/`is_dir`/`is_link`/`is_readable`/`is_writable`/`is_executable`, `filesize`, `filemtime`/`fileatime`/`filectime`, `filetype`, `fileperms`, `realpath`, `touch`, `clearstatcache`) plus **`umask`** (through `rustix`, so `#![forbid(unsafe_code)]` holds): php answers the previous mask, and with no argument reads the current one the only way the C API allows — set to `0`, then put back. Symfony's `GenericRuntime` calls `umask(0o000)` in debug mode. tier-a: `file/umask.php` |
| filter (`filter.rs`) | 5 | S10: `filter_var`, `filter_var_array`, `filter_has_var`, `filter_list`, `filter_id` — the validators (int/bool/float/regexp/domain/url/email/ip/mac) and sanitizers with php's flag set, and the three-way failure outcome (`false` / the `default` option / `null` under `FILTER_NULL_ON_FAILURE`) Symfony's `ParameterBag` leans on. S10 tier-a: `filter/basics.php` |

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

## Deferred by S2 (string / array half)

**RNG (random extension state):** `shuffle`, `str_shuffle`, `array_rand` — need the
`mt_rand`/`random_int` generator so seeded sequences match php.

**streams:** `fprintf`, `vfprintf` (need `STDOUT`/`php://` stream resources).

**objects (E6):** `count()` on `Countable`, `iterator_*`, objects in `array_column`,
`__toString` operands in the string functions, sorting objects by property lists,
`array_multisort`/`sort` flags on objects.

**engine (prefer-ref):** `array_multisort($a, SORT_DESC, ...)` with literal flags — the
`NativeFn` ABI has only a by-ref mask, so a literal in a by-ref slot is "could not be
passed by reference"; the manifest marks these params `prefer_ref` (pass variables for
now). `strtok` keeps its cursor in a thread-local until `ExtState` gains a slot.

**locale:** `setlocale` accepts only `C`/`POSIX`/`C.UTF-8`/`""`/`"0"` (php also accepts
whatever the OS has installed); `nl_langinfo` skipped; `strcoll`/`SORT_LOCALE_STRING`
are byte comparisons (C locale). `hebrev` skipped.

**value crate:** `null == "0"` loose comparison (`in_array`/`array_search`/`array_keys`
with a null needle) — php compares null against a string as `""`; `Array` has no
pointer identity, so `count(COUNT_RECURSIVE)` on a direct self-reference counts one
level deeper than php before warning.

**php-internal ordering:** the `array_intersect`/`array_udiff` family emit "Array to
string conversion" once per element here, php once per comparison of its sort-merge.

## Deferred by S2 (var / type / url / info half)

**magic serialization hooks:** ✅ `__serialize`/`__sleep` and `__unserialize`/`__wakeup`
are invoked exactly as php invokes them — `__serialize`'s array becomes the `O:` payload
keys and all, `__sleep`'s names are looked up and mangled (with php's warning for a name
that is not a property, and its "should return an array" warning + `N;` for a non-array
return), `__unserialize` receives the payload as an array and suppresses `__wakeup`, and
`__wakeup` runs as soon as an object's own properties are restored (so a nested object
wakes first). **Still deferred:** the `Serializable` interface's `C:` (custom data) form,
which yields php's "has no unserializer" warning + a bare instance for a declared class —
it needs a consumed-length out-param from the `unserialize` native.

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

## E6 / E7 / E8 (object model, eval + autoload, generators)

**Object model divergences (E6).**

- `unset()` on a *typed* property is indistinguishable from "never
  initialized": `Value::Uninit` covers both, where php has a separate
  `IS_PROP_UNINIT` bit. Resolved toward the far more common
  never-initialized case, so reading such a slot is the initialization
  `Error` and writing it stores directly; php would route both to
  `__get`/`__set` after an explicit `unset()`. Untyped slots are exact.
  A real fix needs a per-slot flag in `rphp-value`.
- `FetchPropW`/`RefProp`/`AssignRefProp`/`SendRefProp` keep pre-E6
  semantics: php calls `__get` and then emits `Notice: Indirect
  modification of overloaded property C::$p has no effect`.
- `[$obj, 'parent::m']` works but php's `Deprecated: Callables of the form
  ["B", "parent::m"] are deprecated` is not emitted — `resolve_callable` is
  `&self` (the stdlib calls it through a shared borrow) and the deprecation
  channel needs `&mut self`.
- Bad-callable errors lack php's zpp wrapper (`call_user_func(): Argument #1
  ($callback) must be a valid callback, …`); rphp emits the engine message.
- `clone` of an object whose state lives in a native `Payload` (WeakMap,
  WeakReference, the SPL containers) produces an empty payload: `ClassDef`
  has no clone hook yet.
- `Closure::getCurrent()` (8.5) is unimplementable until `Frame` carries the
  running closure — php returns the *identical* closure object.
- `ArrayObject::getIterator()` iterates a copy-on-write snapshot where php
  shares the backing array; `WeakMap::getIterator()` likewise. A fault inside
  a container sort names the builtin (`uasort(): …`) where php names the
  method (`ArrayObject::uasort(): …`).
- `SplDoublyLinkedList` / `SplStack` / `SplQueue` are implemented
  (`spl_containers2.rs`), elements and mode word in the two private slots php
  dumps, with the frozen LIFO/FIFO direction, `IT_MODE_DELETE` consumption and
  both serialization shapes: `examples/tier-a/spl/dllist.php` matches php
  byte for byte. `SplFixedArray`, the heaps and the iterator decorators are
  the next slice.
- The legacy `Serializable` pair (`serialize()`/`unserialize()` *methods*, the
  `C:` wire form) is not implemented for the SPL containers; php's modern
  `__serialize`/`__unserialize` path is exact.
- `ClassFlags::ABSTRACT` is set on interfaces and traits, matching the
  existing native registrations; php's reflection reports `isAbstract()` as
  `false` for both. Unobservable until Reflection (E10) lands.

**date (S6), the procedural half.** `date`/`gmdate`/`idate`, `mktime`/
`gmmktime`, `checkdate`, `strtotime`, `getdate`/`localtime`, `date_parse`/
`date_parse_from_format`, `date_default_timezone_get`/`set`,
`timezone_identifiers_list`/`timezone_abbreviations_list`, `date_sun_info`.
The parser is the hard part and is a port of timelib's scanner: ISO 8601 in its
shapes, textual and numeric dates, times with fractions and meridiem, timezone
ids, abbreviations and offsets, `@<ts>`, and the relative grammar (`+1 week
2 days`, `next monday`, `first day of next month`, `last day of february 2021`,
`tomorrow`, `midnight`, `ago`, …), with php's DST resolution rule across a
gap and an overlap. **The `DateTime` family is the next wave**, so the
procedural functions that return one (`date_create`, `timezone_open`,
`date_diff`, the `date_interval_*` pair) are not registered yet. `time`,
`microtime`, `hrtime` and `sleep` stay in `uniqid.rs`, where they already
lived.

**The filesystem tail (S3).** `tempnam`/`tmpfile`, `glob` with its flag set,
`link`/`symlink`/`readlink`/`linkinfo`, `stat`/`lstat`/`fstat` (php's 26-entry
array — thirteen numeric keys and the thirteen named ones, in php's order),
`chown`/`chgrp`/`fileowner`/`filegroup`, `disk_free_space`/`disk_total_space`/
`diskfreespace`, and the handle functions `flock`, `ftruncate`, `fpassthru`,
`fsync`/`fdatasync`, `fscanf`, `fgetcsv`/`fputcsv` (with php 8.4's `$escape`
deprecation).

**password / crypt (S9).** `password_hash`/`verify`/`needs_rehash`/`get_info`/
`algos` and `crypt()` over the whole scheme table php still answers for —
bcrypt (`$2y$`/`$2a$`/`$2b$`), MD5 (`$1$`), SHA-256/512 (`$5$`/`$6$`, with
php's `strtoul` reading of `rounds=` and its rejection, not clamping, of a
count outside `1000..=999999999`), traditional and extended DES — plus
argon2i/argon2id, byte-compatible with the libargon2 php links (a hash made
here verifies there and the reverse). php's failure convention is exact: an
unusable setting is `*0`, or `*1` when the setting itself began with `*0`.
**Divergences:** a `$1$`/`$5$`/`$6$` salt containing a byte outside crypt's
`./0-9A-Za-z` alphabet answers `*0` where php hashes it verbatim (the backend
refuses it; reachable only through a pathological setting such as
`$6$rounds=abc$`, where php's `strtoul` rule makes `rounds=abc` the salt), and
`$2a$`/`$2x$` are the corrected bcrypt implementation, so the handful of 8-bit
passwords where crypt_blowfish's compatibility bugs bite answer `*0` rather
than a wrong hash.

**`debug_backtrace()` / `debug_print_backtrace()`.** The same frame walk an
exception's trace uses, with php's `DEBUG_BACKTRACE_*` flags and `$limit`, and
without the `debug_backtrace()` frame itself. `debug_print_backtrace()` prints
the `#0 file(line): f()` form and, unlike `getTraceAsString()`, no closing
`#N {main}` line.

**Reflection (S5).** `ReflectionClass`/`Object`/`Enum`, `ReflectionMethod`/
`Function`/`FunctionAbstract`, `ReflectionParameter`, `ReflectionProperty`,
`ReflectionClassConstant`, `ReflectionEnum{Unit,Backed}Case`, the three
`ReflectionType` shapes, `ReflectionAttribute`, `ReflectionReference`,
`ReflectionException`, `Reflector` and `Attribute` — every reflector an
ordinary native class whose php-visible property (`$name`, `$class`) is a real
slot, with the rest hidden in the instance payload. Five tier-a snippets under
`examples/tier-a/reflection/` match php byte for byte, and the L1 Composer gate
is green again with them. **Deferred:** the `__toString()` dump formats (php's
multi-line `Class [ <user> class Foo ] { … }`), `ReflectionGenerator`,
`ReflectionFiber`, `ReflectionExtension`, and
`ReflectionParameter::isDefaultValueConstant` (a compiled default is an opaque
initializer; the constant's *name* is not recorded). **Engine gaps it found,
each blocking a real method:** the compiler drops attributes (`attrs: vec![]`)
and doc comments (`doc: None`), a class records no end line, native functions
carry no arginfo, and `ReflectionProperty::isVirtual()` cannot tell a hooked
property with a backing slot from one without.

**SPL containers and iterators (S4).** `SplFixedArray`, the heap family
(`SplHeap`/`SplMinHeap`/`SplMaxHeap`/`SplPriorityQueue`, php's exact sift
order and corruption latching) and the whole decorator set
(`IteratorIterator`, `FilterIterator`/`CallbackFilterIterator`,
`LimitIterator`, `CachingIterator`, `NoRewindIterator`, `InfiniteIterator`,
`AppendIterator`, `EmptyIterator`, `RegexIterator`, `MultipleIterator`,
`RecursiveIteratorIterator` and the recursive wrappers). The call *pattern*
matters as much as the result — php's `RecursiveIteratorIterator` calls
`callHasChildren`/`callGetChildren` at particular moments and `CachingIterator`
reads one element ahead — so each is pinned by a snippet that drives the
decorator over a user iterator which echoes every call. **Deferred:**
`RecursiveTreeIterator` (a presentation layer needing its own probe round) and
the `spl_directory.c` family (`DirectoryIterator`, `SplFileInfo`,
`SplFileObject`).

**Undefined variables warn like php's.** A register starts `Uninit` rather than
`Null`, so the engine can tell "never assigned" from "assigned null" — which
also makes `get_defined_vars()`, `$GLOBALS` and `isset()` list exactly what php
lists. A *read* of a variable the compiler cannot prove assigned emits
`Op::CheckVar`, which is php's `Warning: Undefined variable $x` at run time, on
every read, with the value `null`. The compiler elides the check for a
parameter, a `use` capture, a `global`/`static` binding and anything assigned
earlier in the same straight-line stretch; anything that branches forgets what
it knew, which only ever costs an extra comparison. A quiet read (`??`,
`isset`, `empty`, `@`) never warns, and an argument sent to a *by-reference*
parameter does not either — php creates the variable there — which the runtime
decides at the send, since by-ref-ness is not known until the callee resolves.

**Response headers, `assert()` and the shell-escaping pair.** The CLI keeps a
header list the way php does — `header`, `header_remove`, `headers_list`,
`headers_sent` (with its two out-parameters) and `http_response_code`, all
refusing once output has reached the SAPI and naming where that output
started, while anything held in an `ob_*` level is not yet sent.
`escapeshellarg`/`escapeshellcmd` are byte-exact, including the two rules that
surprise people: a *paired* quote is left alone and a `0xFF` byte is dropped
rather than escaped. `assert()` follows php's compile-time rule: at
`zend.assertions=-1` the call is not lowered at all, so its argument is never
evaluated; above that it throws the given `Throwable`, or an `AssertionError`
carrying the description — or, with none, the text of the call, which the
compiler passes as a hidden second argument the way php does. **Divergence:**
that text is the source slice where php prints its own decompilation, so
unusual spacing shows through.

The differential oracle now pins the same ini on **both** sides: it used to
pass `-d zend.assertions=-1` (and eleven others) to php only, while rphp ran
on its own defaults — which for `short_open_tag` and `zend.assertions` were
not the same values.

**Not yet:** php 8.5's `Deprecated: Using null as an array offset` (the key
conversion happens at nine call sites, several behind a shared borrow that
cannot reach the diagnostics channel).

**Auto-globals are created the way php creates them.** `$argv`, `$argc`,
`$_GET`, `$_POST`, `$_COOKIE`, `$_FILES` and `$_SERVER` are seeded by the SAPI
at startup, in php's order, so `array_keys($GLOBALS)` matches on a fresh
script. `$_ENV` (the process environment) and `$_REQUEST` (an empty array) are
filled **on first touch**, which is why php does not list them until something
reads one — and does afterwards. `$_SESSION` starts undefined and springs into
existence on write, as php has it. `$GLOBALS` itself is never an entry in the
table.

**`is_a()` / `is_subclass_of()` autoload their subject.** php loads the class
named by the first argument when `$allow_string` is on (the target is then
matched by name up the chain, without loading it), which is what makes
Symfony's `is_a(ContainerConfigurator::class, $type, true)` — the test that
decides how a micro-kernel's `configureContainer()` is called — answer `true`.
rphp looked in the class table only, so it answered `false` for a class nothing
had touched yet.

**Traits autoload.** `use LoggerTrait;` resolved through the class table
without the autoload stack, so a trait that ships in its own file (every
Composer-managed one) was `Trait "X" not found`. It now takes the same path as
`extends`/`implements`.

**Superglobals in every scope.** php's auto-globals (`$_SERVER`, `$_ENV`,
`$_GET`, `$_POST`, `$_COOKIE`, `$_FILES`, `$_REQUEST`, `$_SESSION`) are one
variable in every scope — no `global` statement, no closure capture. A function
body that mentions one now binds its register to the global cell in the
prologue, creating the entry on first write as php's auto-global handler does.
Found by running Symfony's Dotenv, where `$_SERVER += $_ENV;` inside a method
was reading `null`. `$GLOBALS` stays what it was: a view of the table, not a
variable.

**`new $cls` autoloads.** A class name in a register went through the class
table without the autoload stack, so every `new $_SERVER['APP_RUNTIME']`-shaped
call in Symfony's Runtime failed with `Class "…" not found` although the loader
would have found it. php looks the name up with a leading `\` stripped (the
loader is called with `Foo`, never `\Foo`) and keeps the caller's spelling in
the error; `$x instanceof $name` still never autoloads.

**Anonymous classes.** Implemented: a `new class { … }` site is numbered and
lowered by the same class pre-pass as every other declaration, under the name
php gives it (`<parent|interface|class>@anonymous\0<file>:<line>$<n>`), and the
expression declares it before instantiating — idempotently, so a site inside a
loop reuses one class entry as php does. The NUL is what keeps the synthesized
tail out of `var_dump`, `print_r`, `get_debug_type` and every error message
(php formats a class name with `%s`), while `get_class()`, `::class`,
`var_export` and Reflection answer with the whole name; `serialize()` refuses
an anonymous class the way it refuses a closure. **Divergence:** php's counter
runs across a whole request, rphp's across a compilation unit, so the `$<n>`
suffix can differ in a multi-file program — visible only through the full name.

**Enum case values.** A case backed by a constant *expression*
(`case A = 1 << 0;`, `case B = self::X;`, `case C = 'a' . self::S;`) is lowered
to an initializer thunk run in the enum's own scope, like a class constant's.
php builds a backed enum's lookup table on **first touch of a case**, not at
declaration — which is where it evaluates those initializers and where
`Error: Duplicate value in enum E for cases A and B` comes from, naming the
*use* site's line. rphp does the same, so the fatal lands on php's line.

**Still unlowered (compile-time `RPHP_E0300`).** The syntax the ladder has not
needed yet: `__halt_compiler`, backtick shell execution, the 8.5 pipe operator
(`|>`) and `clone with` property list, `declare()` of anything but
`strict_types`, `goto` out of a `try` that has a `finally`, and a handful of
by-reference targets php itself rejects. Everything else the compiler refuses
is invalid php.

**eval / autoload (E7).**

- A `ParseError` from `include`/`eval` is catchable and carries php's file
  and line, but the **message text** is rphp's parser wording, not php's
  bison text (`Expected one of ...` vs `syntax error, unexpected identifier
  "is"`). Every other observable — class, file, line, exit code — matches.
- `spl_autoload_register(null)` is an error rather than registering php's
  default include-path loader, which rphp does not have.

**Generators (E8).**

- A suspended generator that is destroyed does not run its pending `finally`
  blocks (php does). `Generator::rewind()` does not raise php's "Cannot
  rewind a generator that was already run" for a generator advanced past its
  first yield.

**Parser.**

- php 8.5's `Deprecated: Case statements followed by a semicolon (;) are
  deprecated` is not emitted; the syntax is accepted silently. This is the
  only difference when running Composer's generated autoloader over a real
  Symfony vendor tree.

## The object wave (date, SPL filesystem, hash contexts, iconv)

`ext/date`'s object half (`DateTime`, `DateTimeImmutable`, `DateTimeZone`,
`DateInterval`, `DatePeriod`, the nine-class exception tree, the procedural
aliases), SPL's seven filesystem classes, `HashContext`, and `ext/iconv`
whole. Their per-module headers carry the full divergence lists; the ones
worth knowing:

- A `createFromDateString` interval derives its fields from the string it
  kept, so `$i->from_string` and `$i->date_string` are *readable* here where
  php's read handler hides them, and writing a field (`$i->d = 9`) lands in
  a dynamic property rather than the hidden struct. Reads, `format()` and
  `add()` all agree with php either way.
- `SplFileInfo`'s debug view is one member short on `DirectoryIterator`,
  `FilesystemIterator` and `GlobIterator` (php shows a `subPathName` slot
  declared by a class they do not extend), `(array)` casts yield the mangled
  private keys where php yields `[]`, and `serialize()` succeeds where php
  refuses an internal class without a serializer.
- `iconv` reports itself as GNU libiconv 1.11, the implementation whose
  `//TRANSLIT` answers were measured into its table; an empty charset name
  is UTF-8 here where libiconv would ask the locale.

## The ladder's gaps (L3, `bin/console list`)

Walking the L3 rung one fatal at a time found these, in this order — none of
which a reading of php-src would have ranked first:

- `stream_is_local()` (the YAML loader's first call), `fileinode()`, the
  `SCANDIR_SORT_*` constants, and `ip2long`/`long2ip`/`inet_pton`/`inet_ntop`
  (a new `net.rs`).
- **xxHash**: Symfony hashes its container with `hash('xxh128', …)`. The hash
  extension now answers 32 of php's 60 algorithms — MD2/MD4/MD5, SHA-1, the
  SHA-2 and SHA-3 families, RIPEMD, Whirlpool, Whirlpool's neighbours in
  spirit (Adler-32, three CRC-32s, the FNV-1 four, joaat) and all four xxHash
  variants. **Still missing, cataloged:** `tiger{128,160,192},{3,4}`,
  `gost`/`gost-crypto`, `snefru`/`snefru256`, the fifteen `haval*` and
  `murmur3a`/`murmur3c`/`murmur3f`. Every one needs its own implementation;
  none has a blocker beyond the writing.
- **`[&$a[$k], &$o->p]`** — a by-reference element of an array *literal* was
  lowered only for a plain variable. It now shares the whole reference-source
  path with `$x = &…`, so an element, a property, a static property, a
  variable variable and a `$GLOBALS` entry all bind their cell.
- **`unserialize()` did not autoload.** php resolves the serialized class
  name through the autoloader; without that, Symfony's "cast a Definition to
  a ChildDefinition by patching the serialized bytes" trick produced a
  `__PHP_Incomplete_Class`.

Then, in order: **ext/session** (FrameworkExtension refuses to configure a
session without it — the extension is implemented whole, files handler and
user handlers alike), the **php 8.4 Reflection surface** the deep-clone
polyfill reads (`isPrivateSet`, `getMangledName`, `getHooks`,
`getClosureUsedVariables`, `hasPrototype`, `ReflectionParameter::__toString`
— twenty-nine methods), and **doc comments**, which the parser had always
read and the compiler had always dropped.

The rung now compiles the whole container and stops on a Symfony-level
complaint — `http_kernel` depending on a missing `event_dispatcher` — rather
than on an engine gap. **That is where the walk resumes.**

Cataloged along the way, none of them blocking:

- `PropertyHookType` (a native *enum*, which the class registry cannot
  declare yet) and `ReflectionExtension`. (The lazy-object family arrived
  with the L6a walk.)
- `ReflectionFunctionAbstract::getStaticVariables()` and `returnsReference()`
  — the runtime keeps a function's `static` cells keyed per function and
  exposes neither them nor `FnFlags::RETURNS_REF` to the stdlib.
- A native function's parameter *names* (`nf!` carries them only where a
  module bothered), so `ReflectionFunction('str_repeat')->getParameters()` is
  empty where php names `$string` and `$times`.
- php 8.4's compile-time `Implicitly marking parameter $x as nullable is
  deprecated` — Reflection reports the implicit `?` now, but the deprecation
  is not emitted.
- php's scanner gives the last docblock it saw to the next declaration it
  opens (`/** … */ $f = function () {};`); the parser here wants it directly
  before the `function` keyword.

## The L6a walk (2026-09-20)

`bin/console about`, `cache:clear`, `debug:container`, `debug:router`,
`debug:autowiring` and `lint:container` are byte-identical to php, and all
107 dumped container files match. In the order the walk found them:

- **Attributes reach Reflection** — the compiler never filled `AttrDef`;
  FrameworkBundle's `#[RequiredBundle]` declares its dependencies.
- **`&$a[]`**, a reference to an append (`EventDispatcher`).
- **Private property shadowing** — two same-named private slots on one
  object (`OutputStyle` and `SymfonyStyle` both declare `$output`).
- **Objects compare by value** — `==`, `<`, `<=>` property by property, and
  an object beside a number is `1` with php's notice.
- **ext/tokenizer** over the workspace scanner (the routing attribute loader).
- **Every reflector's `__toString()`** — Symfony hashes them into the
  container's resource signatures.
- **A `Stringable` object at a native's `string` parameter** — from the
  manifest's parameter types (`cargo xtask native-params`; a separate
  string-parameter table was generated at first and folded into the object
  check of 2026-09-21), applied at the native call boundary; `sprintf`'s
  `%s`, `implode()` and a loose comparison with a string by hand.

- **php 8.4 lazy objects** — the container dumper tries
  `ReflectionClass::newLazyGhost()` and dumps a ghost when it exists, a proxy
  otherwise; the last artifact (`getUriSignerService.php`) matched once it
  did. The state rides on the object (`LazyState`: kind, initializer,
  options, the declared slots' defaults, the slots initialized ahead); making
  an object lazy empties every declared slot, which is why `var_dump` shows
  `lazy ghost object(C)#n (0) { ["a"]=> uninitialized(int) … }`, `print_r`
  and `(array)` show nothing, and `serialize()` under
  `SKIP_INITIALIZATION_ON_SERIALIZE` writes `O:1:"C":0:{}` — with no special
  casing in any of them. A property access (read, write, `isset`, `empty`,
  `unset`, a dynamic write) runs the initializer; so do `foreach`, `clone`,
  `==`/`<=>`, `json_encode`, `serialize`, `var_export` and
  `get_object_vars`; a method call, `instanceof`, `property_exists`,
  `spl_object_hash` and `===` do not. A ghost's slots hold their defaults
  when its initializer starts and are emptied again if it throws; an
  initialized ghost is an ordinary object again, an initialized proxy
  forwards every access to the real instance (`["instance"]=>` in the dumps)
  and its clone is a new proxy over a clone of it. A class with no declared
  properties has nothing to defer and gets a plain object, as php does.
  Destructors: never for an uninitialized lazy object, never for a proxy
  (the real instance runs its own), at `resetAsLazy*` time for the state
  being discarded unless `SKIP_DESTRUCTOR`. Every error text measured:
  internal classes (`X is internal` / `X inherits internal class Y`),
  `Object is already lazy`, the option checks, the argument types
  (`must be of type C, stdClass given`), and the initializer/factory return
  checks. **Not reproduced:** php's `(3)` count after resetting an object
  that had dynamic properties (its property table survives the reset).
- **`uninitialized(T)` in `var_dump`** for any uninitialized typed property
  (`PropMeta` now records the declared type's canonical text) — a gap the
  lazy dumps exposed.
- **`#[AllowDynamicProperties]`** was parsed and never acted on: the
  compiler now sets `ALLOW_DYNAMIC` from the attribute, inherited as php's
  is.
- **`spl_object_hash()`**.

Also cataloged: `ReflectionClassConstant::getAttributes()` (the compiler
lowers them, the runtime constant does not carry them yet), `PhpToken`
subclasses (`MyToken::tokenize()` answers `PhpToken` instances), and
`TOKEN_PARSE` (accepted, not applied).

## SAPI-3 and the L7 walk (2026-09-20)

`rphp -S host:port [-t docroot] [router.php]` — php's `cli-server` SAPI as
its own crate (`rphp-sapi-server`, ADR-033): a hand-rolled HTTP/1.x reader
over `TcpListener`, one request per connection (`Connection: close`, as
php's), each served on a request-stack thread with a fresh interpreter
(`PHP_CLI_SERVER_WORKERS=n` runs `n` accept loops). Measured against
`php -n -S` rather than read off php-src:

- **Routing** is php's `normalize_vpath` + `translate_vpath` step for step
  (`resolve.rs`): a missing path walks back a segment at a time until a
  file or a directory with `index.php`/`index.html` turns up, the stripped
  tail becoming `PATH_INFO`; a directory without an index is `404`. A
  router script runs first with the server's working directory and its
  `false` return hands the request on — to the script (same interpreter,
  same superglobals) or to the static file.
- **The response head** is php's: status line (or the script's own
  `HTTP/…` line), `Host`, `Date`, `Connection: close`, then the header list
  `header()` built (`X-Powered-By` first, `Content-type: text/html;
  charset=UTF-8` appended when nothing set one, never for a 304). The head
  goes out on the first byte that leaves the output buffers, on `flush()`
  (which is why a later `header()` then complains without a location), or
  at the end. A `HEAD` request drops the body after the head.
- **`$_SERVER`** in php's key order (environment, the SAPI's keys, the
  headers as `HTTP_*` with `CONTENT_TYPE`/`CONTENT_LENGTH` doubled,
  `PHP_AUTH_*` from `Authorization`, the request times); `$_GET`/`$_POST`
  through the stdlib's `parse_query` (`max_input_vars`), `$_COOKIE` with
  php's rules (name as sent, raw-decoded value, first of a repeated name),
  `$_FILES` from an RFC 1867 parser with php's `php……` temp names,
  `full_path`, per-attribute arrays for `f[]`, `is_uploaded_file()` /
  `move_uploaded_file()`, temp files removed at request end; `$_REQUEST`
  per `request_order`/`variables_order`; `php://input` (empty for a
  multipart POST, as php consumes it); `getallheaders()` /
  `apache_request_headers()` / `apache_response_headers()` registered only
  under the server SAPI, as php does.
- **Static files** with php's 1181-entry mime table (`; charset=UTF-8` for
  `text/*`), `Content-Length`, `405` with `Allow: GET, HEAD, POST` for
  `DELETE`/`PUT`/`PATCH`; the `400`/`404`/`405` pages byte for byte.
- **The ini** `php -n -S` reports: `html_errors=1` (errors render as
  `<br />\n<b>Warning</b>:  … in <b>file</b> on line <b>N</b><br />`, the
  message escaped for a fatal or parse error only), `implicit_flush=0`,
  `max_execution_time=30`, `register_argc_argv=0`; `output_buffering`
  honoured when set (a chunked default level flushes every `chunk_size`
  bytes). The log is php's (`[Sun Sep 20 11:49:38 2026] 127.0.0.1:50751
  [200]: GET /`, `Accepted`/`Closing`, `PHP Warning:  …` when
  `log_errors`), stamped in local time as `ctime()` does.
- **Headers everywhere:** the head is a `ResponseHead` shared between the
  interpreter and the SAPI sink; `header()` applies php's rules
  (`Content-Type: text/plain` → `Content-type: text/plain;charset=UTF-8`,
  `Location` → 302 unless a status was chosen, `HTTP/…` lines as status
  lines), the CLI keeps no list at all (php's CLI handler drops the line,
  so `headers_list()` is empty there), `setcookie()`/`setrawcookie()` build
  php's `Set-Cookie` lines (raw-url-encoded value, `deleted` + 1970 for an
  empty one, `expires`/`Max-Age`, every option and error text), and
  `session_start()` reads the id from the cookie, sends the session cookie
  from `session.cookie_*` and the `session.cache_limiter` headers.

**The L7 walk** (`GET /`, `GET /nonexistent`, a `PATH_INFO` request and a
`POST`, byte-identical to `php -S` under the fixture's allowlist for
`random_int()` hues, VarDumper ids and the request clock) found, one fatal
at a time:

- **Property hooks: backed or virtual.** A hook body that mentions
  `$this->name` makes the property backed — reads without a `get` hook and
  writes without a `set` hook go to the slot (Symfony's `Request` declares
  `public ServerBag $server { set { …; $this->server = $value; } }` and reads
  it plainly). A property none of whose hooks does is virtual: no slot at
  all, so nothing dumps, casts or serializes it, `isInitialized()` is true,
  the missing accessor is `Property C::$p is read-only`/`write-only`, and
  `getRawValue`/`setRawValue` are `Must not read from`/`write to virtual
  property`. `isset()`/`empty()` ask the `get` hook, `get_object_vars()` and
  `json_encode()` read hooked properties through it, `unset()` of a hooked
  property is `Cannot unset hooked property`, `setValue()` runs the `set`
  hook, `setRawValue()` bypasses it. Reflection's `hasHooks`/`getHooks`/
  `hasHook`/`getHook`/`isVirtual` answer (a hook is a `ReflectionMethod`
  named `$prop::get`), and `PropertyHookType` exists — declared by a php
  *prelude* every interpreter compiles, the one native enum the registry
  cannot declare, marked internal afterwards.
- **A by-reference argument bound to a nested element**
  (`krsort($this->listeners[$name])`, `byref($a['x']['y'])`) — the compiler
  sent a copy; the container is now fetched for writing with its
  write-backs. With it, php's `zend_array_dup` rule: **duplicating an
  array unwraps a reference nobody else holds**, so a copy taken after
  such a call does not share the element.
- **Autoload through a callable**: `['C', 'm']()`, `'C::m'`,
  `call_user_func`, `is_callable()` load the class first, as
  `zend_is_callable` does (VarDumper's casters name their classes as
  strings).
- **Traces**: a closure's frame carries the class it is scoped to (`::`
  without a `$this`), a trait's method the *using* class, a call in a fluent
  chain the line of the method name, string arguments are escaped C-style
  after the 15-byte cut, and an uncaught argument `TypeError` reads
  `… called in X on line N and defined in F:L`.
- **`FILTER_CALLBACK`** and the `FILTER_REQUIRE_ARRAY`/`FORCE_ARRAY`/
  `REQUIRE_SCALAR` flags in `filter_var()`, the missing `FILTER_*`
  constants, and **deprecated constants** as a registry feature
  (`FILTER_SANITIZE_STRING` raises php's deprecation on every fetch).
- **`highlight_file()` / `highlight_string()` / `show_source()`** — php's
  syntax highlighter (8.3+ `<pre><code>` form) over the workspace scanner,
  which the debug exception page excerpts source with.

**Cataloged, not done:** the parser's messages (`Unexpected token
`Semicolon`` where php says `syntax error, unexpected token ";"`), reported
through the standard error path now but in mago's words;
`get_defined_constants(true)` files every constant under `Core`; php's
`(3)` property count after a reset of an object that had dynamic
properties; FastCGI (SAPI-4) is the next SAPI.

## PDO, and the engine features the L8 walk needed (2026-09-20)

**`ext/pdo` + `pdo_sqlite`** as their own crate (`rphp-ext-pdo`, over the
`rusqlite` bundle — pure Rust apart from SQLite's own C, ADR-032's list
grows by one confined `unsafe` site: `bridge.rs`, which parks the
interpreter's address in a thread-local for the duration of a driver call
so a user-defined SQL function/aggregate/collation can re-enter it from
SQLite's synchronous callback; the invariants are stated in the module
header). Measured against php's `pdo_sqlite`: the DSN parse and `could not
find driver`, the `SQLSTATE[..]: desc: code msg` texts and the `errorInfo()`
triples, errmode exception/warning/silent (recorded on the statement, not
the handle), `rowCount()` only for a change statement, every fetch mode
(`GROUP` keeps `BOTH`'s column positions, `KEY_PAIR`/`INTO`/`CLASS` errors
through `general_error`, `LAZY` as `PDORow`), `bindParam()` through the
engine's new by-reference cell channel (`Frame.ref_cells`, `Ctx::ref_arg`),
`setFetchMode()`'s `ArgumentCountError` texts, the attribute set
(`getAttribute()` reports `IM001` for an unknown one, `setAttribute()` is
`false`), transactions and their messages, `quote()`'s NUL refusal, the
`Pdo\Sqlite` subclass with `createFunction`/`createAggregate`/
`createCollation`/`loadExtension` and the deprecated `PDO::sqliteCreate*`
wrappers, the deprecated `PDO::SQLITE_*`/`MYSQL_*`/`PGSQL_*` constants
(deprecated class constants are a registry feature now). Result sets are
buffered per `execute()` (`getIterator()` too). Not done: `openBlob()`,
`setAuthorizer()`, `getColumnMeta()` past the last column.

**The L8 walk** (`fixtures/ladder/L8-demo`, symfony/demo: `bin/console
about`, `debug:router`, `lint:container` green — the whole demo container
with Doctrine, Twig, security, translator and profiler compiled under the
`DebugClassLoader`) found, one fatal at a time:

- **Named arguments to natives** (`json_decode($s, flags: …)`): php's
  arginfo — every native's parameter names, declared types, defaults and
  return type — is a generated table (`cargo xtask native-params` →
  `rphp-runtime/src/native_params.rs`, 3846 natives) the call windows bind
  against: unknown names are php's `Error` before the frame, or the
  `ArgumentCountError` from inside it for a variadic native (whose frame
  the trace shows with the named extras); a skipped required position is
  `Argument #n ($x) not passed`, a skipped `<default>` one `must be passed
  explicitly, because the default value is not known`, a skipped optional
  one gets the stub's default evaluated (literals, constants,
  `Class::CONST`, `|`); a by-reference named parameter still receives the
  caller's cell; `call_user_func`, `ReflectionClass::newInstance`,
  `Reflection*::invoke`, `Closure::call`/`__invoke`, `Fiber::start` forward
  unknown names to the callable. The same table gives Reflection a native's
  parameters, types, defaults (`isDefaultValueConstant()`,
  `getDefaultValueConstantName()`, `<default>`) and return type, and a
  user parameter's constant default keeps its *name* (`N\PHP_INT_MAX`,
  `self::X`) for those two, with `__toString()` printing defaults as
  written and strings escaped like `smart_str_append_escaped`.
- **Fibers** (`Fiber`, `FiberError`): a fiber is a parked call chain —
  `Fiber::suspend()` reached from bytecode moves every frame above the
  fiber's boundary (register windows, pending calls, `foreach` iterators)
  into the fiber; `resume()`/`throw()` put it back at the current stack top
  and continue after (or raise at) the suspending call; nesting, transfer
  values both ways, status queries, `getReturn()`'s three refusals,
  exceptions crossing the boundary both ways, traces from inside. The one
  gap: a suspend *through a native* (a callback inside `array_map()`, a
  generator body driven by `Generator::send()`) has a Rust frame between
  boundary and suspension point and is refused with a `FiberError` — php
  switches C stacks. A suspended fiber that is never resumed does not run
  its `finally` blocks on destruction.
- **`Attribute`, `ReturnTypeWillChange`, `AllowDynamicProperties`,
  `SensitiveParameter`, `Override`, `Deprecated`** are php-written in the
  embed prelude (marked internal), so each carries its own
  `#[Attribute(...)]` with the flags php reports through
  `ReflectionAttribute::getArguments()`/`newInstance()`.
- **`readonly` and indirect modification**: a write fetch, a reference, an
  `unset()` of an element or a by-reference send of a readonly property is
  `Cannot indirectly modify readonly property` whatever the scope and
  before initialization too, unless the property holds an object (a
  handle). A nested place sent as an argument (`new X($this->list[$k])`)
  is fetched for writing only when the parameter turns out to be
  by-reference — `JmpUnlessArgByRef`, php's `FETCH_DIM_FUNC_ARG` decision —
  so a by-value call reads a readonly element (and does not autovivify a
  missing key) as php does. A write fetch of `$this->p` from an ancestor's
  method reaches the ancestor's private slot when the subclass re-declares
  the name.
- **Nested writes through `ArrayAccess`** (`$map[$k][] = 1`, `$o[$k]++`,
  `.=`, `??=`): real storage on `ArrayObject`/`ArrayIterator`/`WeakMap`
  (php's `spl_array`/`WeakMap` dimension handlers; a `W` fetch is quiet, an
  `RW` one warns), a temporary with php's `Indirect modification of
  overloaded element` notice and no `offsetSet()` for every other class,
  a by-reference `offsetGet()` followed (`WriteBackElem`, `FetchElemRW`).
- **Return by reference** (`function &f()`): a returned place hands the
  caller its cell (`$x = &f()` binds, a by-value use derefs, a native
  never sees the cell), a non-place earns php's notice and a fresh cell.
- **`ext/standard` syslog** (`openlog`/`syslog`/`closelog`, the `LOG_*`
  constants — over the logger's datagram socket, since `libc` is off
  limits), the 85 `STREAM_*` constants (`STREAM_PF_INET6` is macOS's 30),
  `stream_set_chunk_size()`/`stream_set_write_buffer()`/
  `stream_set_read_buffer()` with php's `-1`/`0` answers.
- **Class inspection autoloads** (`method_exists`, `property_exists`,
  `get_class_methods`, `get_parent_class`, `get_class_vars`; the `class_*`
  family per its `$autoload` argument with the warning text changing) and
  the loaders never see an empty or invalid name (`'\\'` does reach them as
  `''`); `class_exists()` is true for an enum; `SessionHandler` implements
  only `SessionHandlerInterface` and `SessionIdInterface`, as php's does.

**Cataloged, not done:** `ext/dom` (+ `libxml`, `simplexml` with XPath) —
the demo's XLIFF translations stop at `Extension DOM is required`, the next
extension wave; `ReflectionClass::__toString()`; the `Tentative return`
marker and the `<internal:ext>` label of internal methods; a compound
constant default (`E_ALL | E_STRICT`) prints its value in `__toString()`
where php prints the expression; `RoundingMode` (8.4) and `round()`'s enum
mode; object ids drift by the hidden objects Reflection holds
(`object-id` category).

## ext/dom, libxml, simplexml, and the demo's blog over HTTP (2026-09-20)

**`ext/dom` + `libxml` + `simplexml`** as their own crate (`rphp-ext-dom`,
`#![forbid(unsafe_code)]`, no libxml2): an arena document (`tree.rs`, one
php object per node cached in the document so identity holds, orphan
holding documents for `new DOMElement()` adopted on insertion), a
hand-written XML parser with libxml's error codes, messages, columns and
its habit of going on after most errors (`parser.rs`; the diagnostics
probe matches php line for line), an XML serializer with libxml's escaping
per context, the `formatOutput` rule and namespace reconciliation
(`serialize.rs`), an XPath 1.0 engine (`xpath.rs`: lexer, parser,
evaluator, `php:function()`/`functionString()`/`registerPhpFunctionNS()`,
the document element as the default context node, unknown prefixes refused
at compile time), `SimpleXMLElement` with its property/iterator/array
faces (`simplexml.rs`), libxml's HTML 4 tag-soup parser in outline plus its
HTML serializer (`html.rs`), and the class surface: `DOMNode` and its
family, `DOMNodeList`/`DOMNamedNodeMap` (live), `DOMImplementation`,
`DOMXPath`, `DOMException`, the `XML_*`/`DOM_*`/`LIBXML_*` constants,
`libxml_use_internal_errors()` & co. The computed properties go through the
runtime's new native-property hook (`NativeProps`: php's `prop_handler`,
`get_properties`, `get_debug_info`), which `var_dump`/`print_r`/`(array)`/
`get_object_vars`/`json_encode`/`foreach`/`isset`/`empty` all consult.

**php 8.4's `Dom\*` API** on the same tree (`classes/modern.rs`,
`selector.rs`): `Dom\HTMLDocument::createFromString/createFromFile/
createEmpty` with an HTML5 parser in outline (implied `html`/`head`/`body`
and `tbody`, the whitespace rules per insertion mode, `svg`/`math` foreign
content with the case adjustments and integration points, the adoption
agency for formatting elements, RCDATA/raw-text elements, `<?…>` as a
bogus comment, legacy named references without `;`, the C1 remap, and
lexbor's `tokenizer error …`/`tree error …` diagnostics with their
columns), `Dom\XMLDocument` (`XML fragment is not well-formed` after the
warning, the working directory as its `documentURI`), the HTML5
serialization algorithm (`<!DOCTYPE html>` alone, `&nbsp;`, void elements),
XHTML rules in `saveXml()` of an HTML document (`<br />`, `<head></head>`)
and the well-formed *subtree* serialization behind `saveXml($node)`,
`innerHTML`/`outerHTML` (declaring the namespaces in scope on the subtree's
root); `Dom\HTMLElement` (uppercase `nodeName`/`tagName`, XHTML namespace)
vs `Dom\Element`, `Dom\TokenList` (`classList`, the DOMException codes
and texts for bad tokens), `Dom\NodeList`/`HTMLCollection`/
`NamedNodeMap`/`DtdNamedNodeMap`, `Dom\Implementation`, `Dom\XPath`,
`Dom\AdjacentPosition` (prelude enum), `getElementsByClassName()`,
`querySelector()`/`querySelectorAll()`/`closest()`/`matches()` over a CSS
selector engine (type/universal/id/class/attribute selectors with the six
operators and `i`, the four combinators, selector lists, `:not()`/`:is()`/
`:where()`, `:nth-*()`, `:first/last/only-child` and `-of-type`, `:empty`,
`:root`; `Invalid selector` is a `DOMException` code 12), `insertAdjacentHTML()`,
`getInScopeNamespaces()`, `rename()`; the modern property tables in php's
dump order, `nodeValue`/`prefix`/`textContent`/`baseURI` answering `null`
where the modern API does and `""` where libxml's does. DTD declarations
are nodes now: `$doctype->entities`/`notations` list `DOMEntity`
(`nodeType` 17, ids only for an unparsed entity) and `DOMNotation`, and the
doctype's internal subset is re-serialized from its declarations the way
libxml prints it (notations first, `<!ELEMENT>` content models with
` , `/` | `, one `<!ATTLIST>` line per attribute, comments inline).

**Documented divergences:** object ids (`object-id`); Reflection sees no
properties on native-property classes and lists the modern classes'
methods in registration order; `Dom\NodeList`/`HTMLCollection`/
`NamedNodeMap` implement `ArrayAccess` here where php serves `$list[0]`
through a handler; `SimpleXMLElement` implements `ArrayAccess` likewise;
no XSD/RelaxNG validation (`schemaValidate()` & co. answer `true`), no
`xinclude()`, no `C14N` beyond the plain serialization, no external
entity loading; the HTML5 parser is an outline (no template contents,
no `<form>` pointer, no `<table>` foster parenting, no encoding sniffing —
the demo's sanitizer only needs fragments), and a `bad-doctype-token`
range column is approximated; the entity map's order is declaration order
where libxml's hash order varies between runs; `libxml_get_errors()` file
is `""` for libxml-style errors and `Entity` for lexbor's, as php reports.

**The blog over `rphp -S` (L8http)** — `GET /`, the blog and its second
page, a post, search, the RSS feed, the German blog, the login form and
two 404s, byte-identical to `php -S` after the declared normalizers (profiler
tokens and nonces, session ids, the render-time comments, cookie dates, the
JSON-escaped port, the `Install the curl extension` log row php's own
`ext/curl` avoids) — needed, one fatal at a time:

- **`DateTimeZone::getLocation()`** from a bundled `zone.tab`
  (`crates/rphp-stdlib/data`), the coordinates through timelib's integer
  encoding so the floats come out as php's (`48.21665999999999`);
- **generator traces**: a generator driven by `foreach` has no
  `Generator->rewind()`/`next()` frame (the body's frame sits at the loop,
  with its class), a generator method's frame names its class, and a
  `yield from` delegate runs with the outer body *unparked*: its frame
  shows beneath the inner's, a `try` around the `yield from` catches what
  the delegate throws (and `finally` runs), `Generator::throw()` lands in
  the innermost delegate, an exception thrown into a `try { yield }` that
  ends with the yield is caught;
- **by-reference `foreach` over a nested place** (`$this->listeners[$e]`,
  `$arr[$k]`, `$o->list[0]`) goes through the write-fetch chain `$x = &…`
  uses, so the loop mutates the container in place (the event dispatcher's
  lazy listeners resolved twice before); the container temp is released
  with the loop; a null/undefined place warns (`foreach() argument must be
  of type array|object, null given`) and stays as it is;
- **`json_encode()` of an enum case**: a backed case is its value, a pure
  one is `JSON_ERROR_NON_BACKED_ENUM` (`0` under partial output);
- `file_put_contents('php://stderr'|'php://output'|'php://stdout')`; a
  class member of a native's `Class|string` union takes its instances as
  they are (`Dom\Node|string`, `Throwable|string|null`);
  `libxml_use_internal_errors(false)` drops the buffer; `get_object_vars()`
  turns a numeric property name into an integer key.
- The ladder's working copy keeps the fixture's `var/sass` (`[fixture]
  var_keep`), the compiled stylesheet no rung command produces.

**Cataloged, not done:** `ext/curl` (the demo's `HttpClient` falls back to
the native client with a notice), `ReflectionClass::__toString()`, the
`Tentative return`/`<internal:ext>` labels, `RoundingMode`, FastCGI
(SAPI-4), and performance: the release build served `/en/blog/` in ~2.2 s
warm against `php -S`'s 75 ms; packed arrays, three accidental
copy-on-write copies and a per-worker compiled-unit cache (opcache's role:
`rphp -S` keeps one long-lived thread per worker, so the cache lives across
requests; a changed file recompiles by size and mtime) brought it to
0.48 s, which is the interpreter's own per-op cost (`examples/bench`).

## SAPI-4: FastCGI (2026-09-21)

**`rphp -b host:port`** (php-cgi's flag; `PHP_FCGI_CHILDREN` sets the
worker count) is a FastCGI responder (crate `rphp-sapi-fcgi`, no unsafe)
measured against `php-fpm -n` through a FastCGI client in
`tools/rphp/tests/fcgi.rs` — 37 request cases over `examples/http/docroot`,
byte-identical stdout, stderr and end-request status once request times,
session ids, upload temp names and clock reads are placeholders. php-fpm's
ways, as found:

- `PHP_SAPI` is `fpm-fcgi`; `$_SERVER` is `USER`, `HOME` (its cleared
  process environment), the parameters in the reverse of their arrival
  (php-fpm's hash yields them so), `FCGI_ROLE`, `PHP_SELF`
  (`SCRIPT_NAME` + `PATH_INFO`), `REQUEST_TIME_FLOAT`, `REQUEST_TIME`;
  `PHP_AUTH_*` from `HTTP_AUTHORIZATION`. `getenv()`, `putenv()` and
  `$_ENV` work on that request environment (the engine's new
  `Interp::request_env`), so `getenv('PATH')` is `false` as under php-fpm
  and `getenv()` lists the parameters; `TMPDIR` is read from it too, and
  php's temporary-directory rule is now php's (`P_tmpdir`, `/var/tmp/` on
  macOS, when `TMPDIR` is unset — `sys_get_temp_dir()` and uploads).
- The CGI head: `Status: <code> <reason>` only for a code other than 200
  (a script's own `HTTP/1.1 418 Teapot` line gives `Status: 418 Teapot`; a
  code php's table has no phrase for is `Status: 418`; `HTTP/1.1 200 OK`,
  which Symfony sends, earns nothing), `X-Powered-By` first when
  `expose_php`, the header list, the default `Content-type`. `HEAD` sends
  the head alone.
- A missing `SCRIPT_FILENAME` is `Status: 404` + `File not found.` with
  `Primary script unknown` on `FCGI_STDERR`; a directory is `Status: 403`
  + `Access denied.` with php-fpm's `security.limit_extensions` message.
- `error_log()` (and `log_errors`) go to the web server on `FCGI_STDERR`
  as `PHP message: …` while the request is open; `fastcgi_finish_request()`
  flushes every buffer and the head, ends the request (`FCGI_END_REQUEST`)
  and lets the script run on with its output dropped — php-fpm keeps only
  its own log after that. The `error_log` ini file, when set, takes
  `[20-Sep-2026 17:03:37 UTC] message` lines (`date.timezone`) from every
  SAPI now.
- `FCGI_GET_VALUES` answers only `FCGI_MPXS_CONNS: 0`, as php-fpm does;
  one request per connection (a second `FCGI_BEGIN_REQUEST` is refused with
  `FCGI_CANT_MPX_CONN`), `FCGI_KEEP_CONN` honoured, unknown record types
  answered with `FCGI_UNKNOWN_TYPE`, the end-request app status 0 whatever
  `exit()` said (php-fpm's).
- The request binding (`$_GET`/`$_POST`/`$_FILES`/`$_COOKIE`/`$_REQUEST`,
  `php://input`, `getallheaders()`, multipart uploads) moved from the `-S`
  server into `rphp-embed`'s `cgi` module, shared by both SAPIs; php's
  status-code reason table lives there too.

symfony/demo serves through it as through php-fpm (nginx-style parameters
with `SCRIPT_FILENAME=public/index.php`): the same pages, byte-identical
apart from the profiler tokens.

**Not done:** php-fpm's process management (pools, `pm.*`, request
limits, slow log, the status/ping pages), the `-C`/`--fpm-config`
configuration file (only `-d` overrides), a Unix-socket listener,
`security.limit_extensions`, `cgi.fix_pathinfo`'s re-derivation of
`PATH_INFO` when the web server passes a script that is a prefix of the
path, `clear_env = no`, `env[]`/`php_value` pool directives.

## The demo's admin walk (2026-09-21)

Walking symfony/demo's stateful flow — login, a post created with tags, a
comment, the post edited and deleted, logout, the profile, search —
against `php -S` with a cookie jar found the gaps below; every page of the
walk is now byte-identical apart from the profiler's tokens, the session
ids, the CSRF tokens and the clock, and the walk is the ladder's
`L8http-flow` rung (`cookies = true`, see `fixtures/ladder/README.md`).

- **A closure is an object.** `spl_object_id()`/`spl_object_hash()` accept
  one (every closure takes a handle from the object allocator now),
  `SplObjectStorage` and `WeakMap` key on one, `clone` makes a new closure,
  `$f::class`, `is_a()`, `is_subclass_of()`, `method_exists()`,
  `class_implements()`, `get_parent_class()`, `ReflectionObject`/
  `ReflectionClass` see the `Closure` class, `$f::fromCallable()` works,
  `(string) $f` is php's `Object of class Closure could not be converted to
  string`, `$f->x` is `Undefined property: Closure::$x` and `$f->x = 1` is
  `Cannot create dynamic property Closure::$x`. `var_dump`/`print_r` print
  php's debug table: `name`/`file`/`line`, the `static` captures, `this`,
  the `parameter` list, or `function` for a first-class callable over a
  named function — and `*RECURSION*` for a closure capturing itself.
- **`ReflectionFunctionAbstract::getStaticVariables()`**: the captured
  `use` variables (by-reference ones as the shared cell), then the `static`
  variables — the current cell once bound, else a constant-expression
  initializer's value (php 8.3 evaluates those on demand; the compiler
  keeps such an initializer as a thunk beside the inline evaluation) and
  null for any other initializer until the body first runs. Symfony's
  VarDumper casts every closure through it, so the profiler's form panel
  needed it.
- **`unset()` of a typed property** hands the slot to `__get`/`__set`/
  `__isset`/`__unset` (php's `IS_PROP_UNINIT`), with the `__get` result
  checked against the declared type; a never-initialized typed slot still
  bypasses magic and errors on read. Symfony's `Constraint::$groups` lazy
  initialization relies on this.
- **Objects meeting native parameters**: php's argument parser refuses an
  object for an `int`/`array`/`bool`… parameter with `must be of type T,
  Class given`, takes `__toString()` for a `string` one, an instance for a
  class part of a union, anything for `object`/`mixed`/`callable`, a
  Traversable for `iterable`. The engine now applies that rule at every
  native call from the generated parameter manifest (`native_params.rs`;
  the separate string-parameter table is gone), and the hand-written
  `… given` messages name objects by class and booleans as `true`/`false`
  (`zend_zval_value_name`), as php 8.3+ does. `max()`/`min()` with one
  non-array argument, `in_array()`'s and `array_merge()`'s argument
  positions, `iterator_to_array()` over a plain object, follow php's
  texts.
- **The set operations compare `(string)` casts** through the engine's
  conversion: `array_diff`/`array_intersect`/`array_unique`/
  `array_diff_assoc` and friends match a Stringable object against a
  string by its `__toString()` (and refuse an object without one with
  php's Error), and `array_keys($a, $needle)` uses the engine's loose
  comparison like `in_array()`. The demo's tag transformer
  (`array_diff($names, $tagObjects)`) inserted every tag twice before this.
- **Per-request module shutdown (`RSHUTDOWN`)**: a server's worker thread
  serves many requests, and the session module's thread-local state leaked
  from one to the next — a fresh browser saw the previous visitor's
  session as already active, with its data. `rphp_embed::request_shutdown`
  now runs after the output is flushed in every SAPI: an active session is
  written and closed, then the session, `mb_*` settings, default timezone,
  `strtok` cursor, locale and `mt_rand` seed are forgotten. The `http` and
  `fcgi` suites send two cookieless requests to a session script to keep
  it so.
- **The session cookie is sent once**: `session_regenerate_id()` drops an
  earlier `Set-Cookie: PHPSESSID=` line (php's
  `php_session_remove_cookie`), so a save handler's own deletion cookie
  (Symfony's `AbstractSessionHandler::destroy()`) does not reach the
  browser next to the new id.

**Still open from the walk:** ext/curl (the profiler's log panel lists
HttpClient's "install the curl extension" notice on every page, normalized
away in the ladder), php's reuse of freed object handles (`#N` in dumps).

## Interpreter performance, second wave (2026-09-21)

See `examples/bench/README.md` for the numbers and the techniques (a
property inline cache, the call path's per-call allocations, temporaries
released at statement end, functions shared with the unit cache). The
property cache is exercised by `examples/tier-a/lang/prop-sites.php` —
one site meeting a dynamic name, several classes, an `unset()` slot, a
reference-bound slot, a coerced type, readonly and asymmetric slots, and
a shadowed private property — and everything else by the differential
corpus and the ladder as before.

Later the same day, the algorithmic traps (`examples/bench/README.md`,
fourth wave): `.=` copying the whole string, every by-reference native
call copying its array, `unset($a[$k])` copying the array — each of them
quadratic in a loop and each fixed at the value or call boundary rather
than per function. Behaviour notes made on the way: `array_pop` steps the
append index back only when the popped key was the one before it (php's
`nNextFreeElement` rule), `array_shift` renumbers integer keys and keeps
string keys, a by-reference native's callback (`usort`) sees the array
as php's does, a negative out-of-range string offset write is php's
`Warning: Illegal string offset N` (it was an Error), `fopen(…, 'a+')`
reads from the start. Corpus: `array/pop-shift.php`,
`lang/byref-natives.php`, `lang/prop-append.php`,
`string/offset-writes.php`.

Fifth wave: the SPL containers mutate their backing arrays in place and
`SplObjectStorage`/`WeakMap` index by object handle (bench README);
`f(A::$p)` sends a static property by value unless the parameter is by
reference (`Op::SendRefStaticProp`), so an uninitialized typed static read
as an argument reports php's plain error; **php 8.4's `#[\Deprecated]`**
on user functions, methods and class constants raises php's notice at
the call site / on every fetch, with `since` and the message
(`lang/deprecated-attribute.php`). Corpus: `spl/object-storage.php`,
`lang/static-prop-sites.php`, `lang/class-const-sites.php`. Then two more php rules: `self`/`parent`/`static` inside a closure
declared outside any class compile to run-time references (php's
`Cannot access "self" when no class scope is active` unbound, the bound
scope after `Closure::bind()`; `lang/closure-scope.php`), and `&$o[$k]`
on an `ArrayAccess` object hands out an `ArrayObject`'s own storage
element, a by-reference `offsetGet()`'s cell, or a copy with php's
"Indirect modification" notice (`lang/arrayaccess-refs.php`). Known:
`SplQueue::dequeue` is O(n).
