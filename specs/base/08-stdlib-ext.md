# 08 — Standard Library & Extension Model

**Status:** stable
**Source sections:** `base-idea.md` §15 (standard library), §16 (extension model)
**Reads with:** [00-overview.md](00-overview.md) (§4 stdlib track), [02-value-model.md](02-value-model.md) (`Value` handles, conversion kernels), [03-heap-types.md](03-heap-types.md) (array/string/SPL backing), [07-jit.md](07-jit.md) (purity/effect flags drive const-fold & hoist), [09-runtime-sapi.md](09-runtime-sapi.md) (async stream/PDO/session I/O), [10-testing.md](10-testing.md) (coverage metric + differential oracle), [decisions.md](decisions.md) (ADR-004, ADR-003, ADR-012)

**Full php-src stdlib parity is a committed end-state goal (ADR-004), not best-effort.** This supersedes baseline non-goal §0.2(#2) ("not 100% coverage on day one"). The stdlib runs as a **dedicated, parallelizable track** alongside the engine milestones ([00-overview.md](00-overview.md) §4), sequenced by dependency and framework/benchmark demand but with *completeness* as the target. Coverage is a first-class, **must-not-regress CI metric** — per-extension `% functions implemented` plus `% .phpt passing` ([10-testing.md](10-testing.md)). `rphp-stdlib` is a registry of `NativeFn` descriptors organized by PHP extension namespace, each behind a `ext-<name>` Cargo feature; removing an extension is removing a flag, never editing the engine.

---

## 15.1 The `NativeFn` descriptor

Every native function and method is a static descriptor. The runtime never sees a bare function pointer; it sees metadata the interpreter, both JIT tiers, reflection, and the diagnostic pass all read from one place.

```rust
pub struct NativeFn {
    name:    SymbolId,            // interned, fully-qualified
    arity:   Arity,              // { required: u8, optional: u8, variadic: bool }
    arginfo: &'static [ParamInfo],
    flags:   FnFlags,
    handler: NativeHandler,      // fn(&mut Ctx, args: &mut [Value]) -> Result<Value, Thrown>
}

pub struct ParamInfo {
    name:     &'static str,      // for named arguments + reflection
    ty:       TypeMask,          // declared type set: int|float|string|array|…|mixed
    by_ref:   bool,             // &$param — handler may write back through the slot
    variadic: bool,             // ...$rest
    default:  Option<ConstExpr>, // const-expr default (reflection + arg-count diagnostics)
}

bitflags! {
    pub struct FnFlags: u32 {
        const DETERMINISTIC  = 1 << 0; // same args ⇒ same result; reads no observable world
        const NO_SIDE_EFFECT = 1 << 1; // writes no observable state (I/O, globals, props, statics)
        const NO_THROW       = 1 << 2; // cannot raise an exception/Error
        const READS_ENV      = 1 << 3; // depends on locale/timezone/ini — NOT const-foldable
        const NORETURN       = 1 << 4; // exit/die-shaped; terminates the frame
        const NODISCARD      = 1 << 5; // 8.5 #[\NoDiscard]-equivalent on the result
    }
}
```

`Ctx` is the per-call runtime handle: isolate, request arena, output buffer, error sink, and the capability set ([09-runtime-sapi.md](09-runtime-sapi.md)). `Thrown` is a thrown `Value` (an `Object` of a `Throwable` class), so native code participates in the same table-driven unwinding as PHP code — no separate error channel.

**Purity/effect flags feed the optimizer.** `DETERMINISTIC | NO_SIDE_EFFECT` is the contract [07-jit.md](07-jit.md) needs:

- **Constant-folding.** A `DETERMINISTIC` call whose arguments are all compile-time constants is evaluated at compile time (during HIR const-folding, [01-frontend.md](01-frontend.md) §6.3) and replaced by its result. `strlen("abc")`, `abs(-3)`, `str_repeat("=", 8)`, `dechex(255)` fold to literals. The fold calls the *same* handler the runtime calls, so a folded result is bit-identical to a runtime one.
- **Hoisting / CSE.** A `NO_SIDE_EFFECT` call that is loop-invariant (its arguments do not change across iterations) is hoisted out of the loop; two identical `NO_SIDE_EFFECT` calls common-subexpression-eliminate. A `DETERMINISTIC` call additionally needs no memory-state guard.
- **`READS_ENV` blocks the fold.** Functions whose result depends on ambient state — `setlocale`-sensitive formatting, `date()` against the default timezone, anything reading `ini` — carry `READS_ENV` and are therefore *not* `DETERMINISTIC`, so they are never folded or hoisted across a point that could change that state. This is the precise line between `strtoupper` (ASCII, deterministic) and `mb_strtoupper` (locale/ICU-dependent, env-reading).

The flags are *conservative by default*: an un-annotated function is treated as impure and effectful, so a missing flag never causes a miscompile — only a missed optimization.

---

## 15.2 arginfo generation (the modern stub system)

`arginfo`, reflection data, and the optimizer's effect flags are **generated from one declarative table** by `xtask`, the modern analog of Zend's `gen_stub.php`. The single source of truth is a per-extension table (a `native_fn!`-style macro / typed manifest), e.g.:

```
fn strlen(string $s) -> int            #[deterministic, no_side_effect, no_throw]
fn array_map(callable|null $cb, array ...$arrays) -> array   #[no_side_effect_if_cb_pure]
fn fwrite(resource $stream, string $data, int $len = -1) -> int|false   #[reads_env]
```

From this `xtask` emits, in lockstep:

1. the `&'static [ParamInfo]` arrays and `Arity` for each `NativeFn`;
2. the **reflection** surface (`ReflectionFunction`/`ReflectionParameter`/`ReflectionMethod` return these verbatim);
3. the `FnFlags` the optimizer consults;
4. a **signature-conformance test stub** wired into the differential harness ([10-testing.md](10-testing.md)).

Because all four derive from one table, signatures, reflection, and effect flags **cannot drift**. A handler whose Rust body contradicts its declared purity is caught by the deopt-stress / differential modes, not by code review.

---

## 15.3 Implementation strategy

Two implementation lanes, chosen per-function by whether reimplementation is wise.

| Lane | Where it applies | Backing |
|------|------------------|---------|
| **Pure Rust** | core value-shaped surface | `rphp-stdlib` over [02](02-value-model.md)/[03](03-heap-types.md) kernels |
| **System-library binding** | crypto, Unicode, compression, XML, regex, network | audited `-sys` crates / vendored libs behind a backend trait |

**Pure Rust** covers the surface that is just operations over the value/heap model: `array` (built directly on the dual-rep array and its SIMD bulk kernels, [03-heap-types.md](03-heap-types.md) §11.2 — including the 8.5 `array_first`/`array_last`), `string` (over the SIMD string kernels, §11.1), `math`, `ctype`, `date`/time, `json` (a simdjson-style parser/serializer), `hash` (the non-crypto and many crypto digests), `filter`, the 8.5 **`URI`** extension, and the standard **SPL** data structures (`SplStack`, `SplQueue`, `SplDoublyLinkedList`, `SplObjectStorage`, `SplPriorityQueue`, `SplFixedArray`, `ArrayObject`, the iterators). These reuse the conversion kernels ([02-value-model.md](02-value-model.md)) so loose-compare, sorting, and casting semantics match the engine exactly.

**System-library bindings** where a clean-room reimplementation would be a correctness liability:

- **PCRE2** via the `pcre2` crate. The pure-Rust `regex` crate is **rejected**: it deliberately omits backreferences and lookbehind (its linear-time guarantee depends on that), so it is **not PHP-compatible** — `preg_*` must accept the full PCRE syntax real code uses. PCRE2 is the same engine php-src links, so pattern semantics match by construction.
- **ICU** for `intl`/`mbstring` collation, `IntlListFormatter`, normalization, transliteration, and case mapping. Per **ADR-003**: native builds bind **system ICU**; the WASM/browser target uses a pure-Rust **`icu4x`** subset with a slim locale bundle. Both sit behind a **backend trait** so the surface is interchangeable and differential-tested for the locales shipped.
- **OpenSSL** (`openssl`-sys) for `openssl_*` and the crypto side of `hash`/`sodium`-adjacent needs; **libxml2** for `dom`/`simplexml`/`xml`/`xmlreader`/`xmlwriter`; **zlib**/**bzip2** for `zlib`/`bz2` and the compression stream filters; **curl** for `curl_*`. `gd`, `gmp`, `libsodium`, `gettext`, `exif`-adjacent codecs follow the same vendored-`-sys`-behind-a-trait pattern.

The trait boundary matters for more than WASM: it is where a binding can be swapped for a pure-Rust implementation later without touching the `NativeFn` surface above it.

---

## 15.4 Full-coverage plan (ADR-004) — the roadmap

Parity is reached by burning down a **dependency- and demand-ordered** tier list, but every tier targets *all* functions in its extensions, not a subset. Sequencing chooses what to do first; it does not bound what gets done.

| Tier | Extensions | Why here | Lane |
|------|-----------|----------|------|
| **A — foundation** | Core, `standard`, `SPL`, `pcre`, `date`, `json`, `hash`, `mbstring`, `ctype`, `filter` | every program and the benchmark/`.phpt` corpus depend on these; they unblock everything else | mostly pure Rust; `pcre`→PCRE2, `mbstring`→ICU |
| **B — framework-essential (ADR-012)** | **PDO + drivers**, **streams/wrappers/filters**, **session**, `reflection`, `fileinfo`, `iconv` | the request path of any framework (Symfony/Laravel-class) routes through these; first-class subsystems (§15.5) | pure Rust core + bindings (drivers, libmagic, iconv/ICU) |
| **C — breadth (the long tail)** | `gd`, `intl`, `openssl`, `curl`, `dom`/`simplexml`/`libxml`, `zip`, `bcmath`, `gmp`, `sodium`, `exif`, `gettext`, `zlib`/`bz2`, and the remaining php-src extensions | completes parity; demand-ordered within the tier (e.g. `openssl`/`curl`/`intl` ahead of `exif`/`gettext`) | predominantly system-library bindings behind backend traits |

**The coverage metric.** Two numbers per extension, tracked in CI and **must-not-regress** ([10-testing.md](10-testing.md)):

1. `% functions implemented` — declared `NativeFn`s present and non-stub, against the php-src function list for that extension at the 8.5 tag.
2. `% .phpt passing` — that extension's slice of the php-src test corpus, scored through the fuzzy + allowlisted oracle (ADR-008).

Both feed a per-extension burn-down dashboard. A merge that lowers either number for any extension fails the gate. Because the stdlib is a **dedicated parallel track** ([00-overview.md](00-overview.md) §4), extensions are staffed and scheduled independently of the M1–M5 engine milestones — the long tail is the dominant schedule risk (§25.4), and a measured burn-down is the mitigation that distinguishes this from the demand-best-effort approach that sank HippyVM, Quercus, and Tagua.

---

## 15.5 First-class subsystems (ADR-012)

These are designed here explicitly, not left implicit under "stdlib," because framework-throughput parity is unreachable without them. Their async behavior is specified jointly with [09-runtime-sapi.md](09-runtime-sapi.md).

### Streams, wrappers, and filters

The stream abstraction is a trait, with `php://`, `file://`, `http(s)://`, `php://memory`, `data://`, `compress.zlib://` etc. as built-in **wrappers**, plus runtime-registered **user wrappers** (`stream_wrapper_register`, a PHP class implementing the streamWrapper protocol).

```rust
pub trait StreamWrapper {
    fn open(&self, ctx: &mut Ctx, path: &[u8], mode: OpenMode, opts: StreamOpts)
        -> Result<Box<dyn Stream>, Thrown>;
}
pub trait Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize>;
    fn write(&mut self, buf: &[u8]) -> io::Result<usize>;
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64>;
    fn metadata(&self) -> StreamMeta;
    // async variants (below) default to wrapping the blocking ones
}
pub trait StreamFilter {            // stream_filter_register / built-in set
    fn filter(&mut self, in_: &mut Bucket, out: &mut BucketBrigade, flags: FilterFlags) -> FilterResult;
}
```

The native **filter set** mirrors php-src: `string.*` (rot13, toupper, strip_tags), `convert.*` (base64, quoted-printable, iconv), `zlib.*`/`bzip2.*`, `dechunk`, and `convert.iconv.*`. Filters compose on a bucket brigade between a stream and its consumer.

**Async.** Blocking stream calls have async variants that, under the server/FCGI reactor, **yield the current fiber instead of the OS thread** ([09-runtime-sapi.md](09-runtime-sapi.md) §17). The `file://` and socket wrappers register their fds with the reactor (`io_uring`/`epoll`/`kqueue`/IOCP); a `fread` on a not-ready socket suspends the fiber and resumes it on readiness, so one OS thread drives thousands of concurrent in-flight streams. The blocking API shape is unchanged — userland code is not colored.

### PDO core + drivers

A thin PDO core over a **driver trait**; drivers are feature-gated.

```rust
pub trait PdoDriver {
    fn connect(&self, dsn: &Dsn, auth: &Auth, opts: &PdoOpts) -> Result<Box<dyn PdoConn>, PdoError>;
}
pub trait PdoConn {
    fn prepare(&mut self, sql: &str) -> Result<Box<dyn PdoStmt>, PdoError>;
    fn begin(&mut self) -> Result<(), PdoError>;
    fn commit(&mut self) -> Result<(), PdoError>;   // + rollback, lastInsertId, quote
}
pub trait PdoStmt {
    fn bind(&mut self, p: Param, v: &Value, ty: PdoType) -> Result<(), PdoError>;
    fn execute(&mut self, ctx: &mut Ctx) -> Result<RowSet, PdoError>;   // async-capable
}
```

**PostgreSQL and MySQL are prioritized** (the framework default targets). **SQLite is added early** specifically for **hermetic tests** (see O-2) — an in-process driver needs no server, so the `.phpt` and integration suites for PDO run deterministically in CI. Drivers prefer pure-Rust protocol crates where one exists at parity, else a vetted `-sys` binding behind the same trait.

**Async query execution yields the fiber, not the OS thread.** A driver built on a non-blocking socket registers with the reactor; `PdoStmt::execute` suspends the calling fiber while the query is in flight and resumes on the result, so a worker handling many requests overlaps their database round-trips on one thread. `mysqli` shares the MySQL driver's transport. Prepared statements and server-side cursors map straight onto the trait.

### Sessions

A `session.*` core over a **handler trait** matching `SessionHandlerInterface` (`open`/`close`/`read`/`write`/`destroy`/`gc`, plus the `validate_id`/`update_timestamp` extensions), so a PHP class can be a custom backend via `session_set_save_handler`.

```rust
pub trait SessionHandler {
    fn read(&mut self, id: &SessionId) -> Result<Vec<u8>, Thrown>;
    fn write(&mut self, id: &SessionId, data: &[u8]) -> Result<(), Thrown>;
    fn destroy(&mut self, id: &SessionId) -> Result<(), Thrown>;
    fn gc(&mut self, max_lifetime: Duration) -> Result<u64, Thrown>;
}
```

Default backends: `files` (the php-src default) and `redis`/`memcached` (binding-backed) for the server SAPI. **Per-request lifecycle within the isolate/arena model** ([09-runtime-sapi.md](09-runtime-sapi.md)): the decoded `$_SESSION` array lives in the request arena and is reclaimed at arena reset; the serialized blob is written back through the handler at request end (or on explicit `session_write_close`), and the handler's I/O is reactor-driven (a `redis` write yields the fiber). No session state crosses isolates except through its backing store.

---

## 16. Extension model (most-safe first)

rPHP does **not** reproduce Zend's C ABI. It offers three layers, blessed-path first.

### 1. Safe Rust extensions — the blessed path

An extension is a crate implementing the `Extension` trait, registering functions and classes through a **typed builder**. No raw pointers and no manual refcounting in the common case; the API hands out safe `Value` handles ([02-value-model.md](02-value-model.md)) whose copy/drop obey COW and immortality automatically.

```rust
pub trait Extension {
    fn register(&self, r: &mut Registry);
}

impl Extension for MyExt {
    fn register(&self, r: &mut Registry) {
        r.function("my_hash", &MY_HASH_ARGINFO, FnFlags::DETERMINISTIC | FnFlags::NO_SIDE_EFFECT,
                   |ctx, args| Ok(Value::int(fnv1a(args[0].as_bytes(ctx)?))));
        r.class::<Widget>("Widget")
            .property("size", TypeMask::INT, Visibility::Public)
            .method("area", &AREA_ARGINFO, FnFlags::NO_SIDE_EFFECT, Widget::area);
    }
}
```

The same `arginfo`/`FnFlags` machinery (§15.1–15.2) applies, so third-party functions get reflection, const-folding, and the diagnostic pass for free. This is the supported, forward path.

### 2. Stable C ABI (`librphp`)

`rphp-ffi` exposes a C header mirroring the embedding API ([09-runtime-sapi.md](09-runtime-sapi.md)) — `librphp`. This is how non-Rust extensions and host languages bind: create/borrow `Value`s, register functions, call into the VM, all across a stable, versioned C ABI rather than Rust's unstable one.

### 3. WASM component extensions

The forward-looking, multi-tenant/edge path: extensions compiled to **WASM components described by WIT**, loaded by `rphp-ext-abi`'s **component host** into a **capability-confined sandbox**. An untrusted extension runs with **no access to host memory beyond its declared WIT interface** — values cross the boundary by the component-model ABI, not by sharing pointers. This reuses the same WASM toolchain the engine itself targets ([00-overview.md](00-overview.md) §3.3) and is the only safe way to run third-party extension code in a shared/edge deployment.

### The Zend C ABI shim — optional research track

A Zend C ABI compatibility shim is **acknowledged as the thing that would unlock the existing PECL ecosystem**, and is filed as a **hard, optional research track** (O-3), revisited after M5 — **not a v1 promise**. PECL modules assume Zend's `zval`, `HashTable`, and memory-manager internals; bridging them faithfully is a large surface with its own correctness oracle, out of scope for the parity goal that the three native layers above already serve.

---

## 15.6 8.5 discard semantics

The 8.5 `(void)` cast and the `#[\NoDiscard]` attribute are honored by the compiler's **diagnostic pass** ([01-frontend.md](01-frontend.md)). A `NativeFn` or method carrying `FnFlags::NODISCARD` (generated from a `#[\NoDiscard]` annotation in the stub table, §15.2) whose result is used in a statement context with no consumer raises the diagnostic; an explicit `(void)` cast suppresses it. Because the flag rides the same descriptor as purity, the diagnostic and the optimizer see a consistent view of every function.

---

## Deviations from base-idea.md

- **ADR-004 supersedes §0.2(#2).** The baseline non-goal "100% stdlib coverage on day one" is **replaced**: full php-src stdlib parity is a **committed end-state goal**, with per-extension `% functions` and `% .phpt` as **must-not-regress CI metrics** and a dedicated parallel track. Sequencing stays tiered/demand-ordered; completeness is the target, not best-effort. This is the headline change of this document.
- **ADR-012 makes streams/wrappers/filters, PDO + drivers, and sessions first-class subsystems** (§15.5), explicitly designed here with async reactor hooks in [09-runtime-sapi.md](09-runtime-sapi.md) — not left implicit under "stdlib."
- **ADR-003 backend trait** for `intl`/`mbstring`: system ICU natively, `icu4x` subset on WASM, interchangeable behind one trait (§15.3).

## Open questions

- **O-2 — SQLite as the default test driver.** Recommendation (per [decisions.md](decisions.md)): add SQLite early **purely for hermetic test fixtures**, so PDO/`.phpt`/integration suites run with no DB server in CI. PostgreSQL and MySQL remain the production-priority drivers; SQLite is a test-and-dev convenience, not a parity-ordering signal. Resolve the in-process driver choice (bundled `libsqlite3-sys` vs a pure-Rust engine at parity) during Tier-B work.
- **O-3 — Zend C ABI shim, ever or never** ([decisions.md](decisions.md)): kept as a research track, revisited after M5; the three native extension layers cover the parity goal without it.
- **`array_map`/`usort`-style callback purity.** Whether a higher-order function can inherit `NO_SIDE_EFFECT` from a provably-pure callback (enabling hoist/fold of `array_map($pure, …)`) — feasible once `rphp-analyze` (O-4) proves callback purity; deferred until then, conservative (impure) default in the interim.
- **`hash`/`openssl` digest overlap.** Which crypto digests live in pure-Rust `hash` vs are routed to OpenSSL — pick per-digest by audited-implementation availability during Tier-C.
