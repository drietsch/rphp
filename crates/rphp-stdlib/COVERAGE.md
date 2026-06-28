# stdlib coverage — burn-down dashboard

Tracks the per-extension parity burn-down (`specs/base/08-stdlib-ext.md`, ADR-004).
The oracle is differential testing against stock **PHP 8.5**: each extension ships a
`examples/tier-a/<ext>.php` snippet that exercises its functions, run through both rphp
and `php` by `crates/rphp-sapi-cli/tests/differential.rs` and required to match
byte-for-byte (must-not-regress).

**Registry size: 180 rows** (wave 1: ~103 distinct functions; wave 2: +11 by-reference
builtins; wave 3: +9 higher-order/callable builtins). Plus aliases and the initial slice.

## Engine gaps that bound the burn-down

Functions needing a missing language feature are **cataloged, not faked** (decision: skip
& catalog). They unblock as the language slice grows:

- **by-reference parameters** — ✅ *native* by-ref done (the ABI copies a builtin's
  mutated arg slots back into the caller's variable; `lib.rs` `Handler::ByRef` +
  `NativeFn::by_ref`). User-defined `function f(&$x)` is still pending (needs parser/AST).
- **callables** — ✅ *function-name-string* callables done: native functions re-enter the
  engine via `Host::call` (`lib.rs` `Host` trait, implemented by `rphp-runtime`'s `VmHost`),
  resolving a callable string to a user function (`Module::func_by_name`) or a builtin.
  **Closures / arrow fns / `[$obj, 'method']`** still pending (parser + closure/object values).
- **objects** — class-returning APIs (also `json_decode`'s default object form, SPL, DateTime).

## Implemented (Tier-A wave 1)

| Extension | Implemented | Notes |
|-----------|-------------|-------|
| ctype     | 11 | all `ctype_*`; ASCII (C-locale) classification; integer special-case matched |
| math      | 33 | trig/exp/log, `pow`, `hypot`, `fmod`, `fdiv`, `is_nan/finite/infinite`, base conversions (`dechex`…`octdec`) |
| string    | 29 | `sprintf`/`printf`/`vsprintf`, `number_format`, `str_pad`, `str_split`, `strtr`, `strcmp` family, `bin2hex`/`hex2bin`, … |
| array     | 18 + 11 + 6 | value-returning: `array_slice/flip/unique/diff/intersect/combine/chunk/column/fill/pad/search/product/…`, `array_is_list`. **By-ref:** `sort/rsort/asort/arsort/ksort/krsort`, `array_push/pop/shift/unshift`, `array_splice`. **Higher-order:** `array_map`, `array_filter`, `array_reduce`, `usort/uasort/uksort` |
| funcs     | 2  | `call_user_func`, `call_user_func_array` |
| json      | 2  | `json_encode` (incl. `JSON_PRETTY_PRINT`/`UNESCAPED_SLASHES`/`UNESCAPED_UNICODE`), `json_decode` |
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

## Known divergences (documented)

- `json_decode` returns an **array** for JSON objects regardless of `$assoc` (no object type yet).
- ctype on bytes ≥ 128 is ASCII/C-locale only (stock php here is C.UTF-8 and classifies some high bytes differently); bytes 0–127 match exactly.
- PHP 8.4+ `E_DEPRECATED`/warning notices (non-string ctype args, invalid base chars, NAN-to-string) are not emitted — the engine has no warning channel yet.
