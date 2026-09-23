//! php-src `ext/posix/posix.c`: the `posix_*` functions and `POSIX_*`
//! constants.
//!
//! php's error convention throughout: a failing call returns `false` and
//! records `errno` for `posix_get_last_error()`; a success leaves the
//! recorded error alone, and so does a lookup that finds nobody.

use std::ffi::CString;
use std::io;
use std::os::unix::ffi::OsStrExt;

use nix::unistd::{Gid, Group, Uid, User};
use rphp_runtime::{nf, value_name, Ctx, Interp, NativeFn, NativeResult, Registry, Unwind};
use rphp_stdlib::StreamFd;
use rphp_value::{numeric_string, Array, ArrayKey, Value};

use crate::sys;

static FUNCTIONS: &[NativeFn] = &[
    nf!("posix_kill", 2, Some(2), posix_kill),
    nf!("posix_getpid", 0, Some(0), posix_getpid),
    nf!("posix_getppid", 0, Some(0), posix_getppid),
    nf!("posix_getuid", 0, Some(0), posix_getuid),
    nf!("posix_setuid", 1, Some(1), posix_setuid),
    nf!("posix_geteuid", 0, Some(0), posix_geteuid),
    nf!("posix_seteuid", 1, Some(1), posix_seteuid),
    nf!("posix_getgid", 0, Some(0), posix_getgid),
    nf!("posix_setgid", 1, Some(1), posix_setgid),
    nf!("posix_getegid", 0, Some(0), posix_getegid),
    nf!("posix_setegid", 1, Some(1), posix_setegid),
    nf!("posix_getgroups", 0, Some(0), posix_getgroups),
    nf!("posix_getlogin", 0, Some(0), posix_getlogin),
    nf!("posix_getpgrp", 0, Some(0), posix_getpgrp),
    nf!("posix_setsid", 0, Some(0), posix_setsid),
    nf!("posix_setpgid", 2, Some(2), posix_setpgid),
    nf!("posix_getpgid", 1, Some(1), posix_getpgid),
    nf!("posix_getsid", 1, Some(1), posix_getsid),
    nf!("posix_uname", 0, Some(0), posix_uname),
    nf!("posix_times", 0, Some(0), posix_times),
    nf!("posix_ctermid", 0, Some(0), posix_ctermid),
    nf!("posix_ttyname", 1, Some(1), posix_ttyname),
    nf!("posix_isatty", 1, Some(1), posix_isatty),
    nf!("posix_getcwd", 0, Some(0), posix_getcwd),
    nf!("posix_mkfifo", 2, Some(2), posix_mkfifo),
    nf!("posix_mknod", 2, Some(4), posix_mknod),
    nf!("posix_access", 1, Some(2), posix_access),
    nf!("posix_getgrnam", 1, Some(1), posix_getgrnam),
    nf!("posix_getgrgid", 1, Some(1), posix_getgrgid),
    nf!("posix_getpwnam", 1, Some(1), posix_getpwnam),
    nf!("posix_getpwuid", 1, Some(1), posix_getpwuid),
    nf!("posix_getrlimit", 0, Some(1), posix_getrlimit),
    nf!("posix_setrlimit", 3, Some(3), posix_setrlimit),
    nf!("posix_get_last_error", 0, Some(0), posix_get_last_error),
    nf!("posix_errno", 0, Some(0), posix_get_last_error),
    nf!("posix_strerror", 1, Some(1), posix_strerror),
    nf!("posix_initgroups", 2, Some(2), posix_initgroups),
    nf!("posix_sysconf", 1, Some(1), posix_sysconf),
    nf!("posix_pathconf", 2, Some(2), posix_pathconf),
    nf!("posix_fpathconf", 2, Some(2), posix_fpathconf),
];

/// The `POSIX_*` constants, with this platform's values, in php's order.
fn constants() -> Vec<(&'static str, i64)> {
    let mut c: Vec<(&'static str, i64)> = vec![
        ("POSIX_F_OK", libc::F_OK as i64),
        ("POSIX_X_OK", libc::X_OK as i64),
        ("POSIX_W_OK", libc::W_OK as i64),
        ("POSIX_R_OK", libc::R_OK as i64),
        ("POSIX_S_IFREG", libc::S_IFREG as i64),
        ("POSIX_S_IFCHR", libc::S_IFCHR as i64),
        ("POSIX_S_IFBLK", libc::S_IFBLK as i64),
        ("POSIX_S_IFIFO", libc::S_IFIFO as i64),
        ("POSIX_S_IFSOCK", libc::S_IFSOCK as i64),
        ("POSIX_RLIMIT_AS", libc::RLIMIT_AS as i64),
        ("POSIX_RLIMIT_CORE", libc::RLIMIT_CORE as i64),
        ("POSIX_RLIMIT_CPU", libc::RLIMIT_CPU as i64),
        ("POSIX_RLIMIT_DATA", libc::RLIMIT_DATA as i64),
        ("POSIX_RLIMIT_FSIZE", libc::RLIMIT_FSIZE as i64),
    ];
    #[cfg(target_os = "linux")]
    c.extend([
        ("POSIX_RLIMIT_LOCKS", libc::RLIMIT_LOCKS as i64),
        ("POSIX_RLIMIT_MSGQUEUE", libc::RLIMIT_MSGQUEUE as i64),
    ]);
    c.push(("POSIX_RLIMIT_MEMLOCK", libc::RLIMIT_MEMLOCK as i64));
    #[cfg(target_os = "linux")]
    c.push(("POSIX_RLIMIT_NICE", libc::RLIMIT_NICE as i64));
    c.extend([
        ("POSIX_RLIMIT_NOFILE", libc::RLIMIT_NOFILE as i64),
        ("POSIX_RLIMIT_NPROC", libc::RLIMIT_NPROC as i64),
        ("POSIX_RLIMIT_RSS", libc::RLIMIT_RSS as i64),
    ]);
    #[cfg(target_os = "linux")]
    c.extend([
        ("POSIX_RLIMIT_RTPRIO", libc::RLIMIT_RTPRIO as i64),
        ("POSIX_RLIMIT_RTTIME", libc::RLIMIT_RTTIME as i64),
        ("POSIX_RLIMIT_SIGPENDING", libc::RLIMIT_SIGPENDING as i64),
    ]);
    c.extend([
        ("POSIX_RLIMIT_STACK", libc::RLIMIT_STACK as i64),
        ("POSIX_RLIMIT_INFINITY", sys::RLIM_INFINITY as i64),
        ("POSIX_SC_ARG_MAX", libc::_SC_ARG_MAX as i64),
        ("POSIX_SC_CHILD_MAX", libc::_SC_CHILD_MAX as i64),
        ("POSIX_SC_CLK_TCK", libc::_SC_CLK_TCK as i64),
        ("POSIX_SC_PAGESIZE", libc::_SC_PAGESIZE as i64),
        ("POSIX_SC_NPROCESSORS_CONF", libc::_SC_NPROCESSORS_CONF as i64),
        ("POSIX_SC_NPROCESSORS_ONLN", libc::_SC_NPROCESSORS_ONLN as i64),
        ("POSIX_PC_LINK_MAX", libc::_PC_LINK_MAX as i64),
        ("POSIX_PC_MAX_CANON", libc::_PC_MAX_CANON as i64),
        ("POSIX_PC_MAX_INPUT", libc::_PC_MAX_INPUT as i64),
        ("POSIX_PC_NAME_MAX", libc::_PC_NAME_MAX as i64),
        ("POSIX_PC_PATH_MAX", libc::_PC_PATH_MAX as i64),
        ("POSIX_PC_PIPE_BUF", libc::_PC_PIPE_BUF as i64),
        ("POSIX_PC_CHOWN_RESTRICTED", libc::_PC_CHOWN_RESTRICTED as i64),
        ("POSIX_PC_NO_TRUNC", libc::_PC_NO_TRUNC as i64),
        ("POSIX_PC_ALLOC_SIZE_MIN", libc::_PC_ALLOC_SIZE_MIN as i64),
        ("POSIX_PC_SYMLINK_MAX", libc::_PC_SYMLINK_MAX as i64),
        ("POSIX_SC_OPEN_MAX", libc::_SC_OPEN_MAX as i64),
    ]);
    c
}

pub(crate) fn register(r: &mut Registry) {
    r.functions(FUNCTIONS);
    for (name, v) in constants() {
        r.constant(name, Value::Int(v));
    }
}

// ---- state and helpers ----------------------------------------------------------

const SLOT: &str = "posix.last_error";

fn last_error(it: &mut Interp) -> &mut i32 {
    let slot = it.ext.slots.entry(SLOT).or_insert_with(|| Box::new(0i32));
    if !slot.is::<i32>() {
        *slot = Box::new(0i32);
    }
    slot.downcast_mut::<i32>().expect("the slot was just made an i32")
}

fn errno_of(e: &io::Error) -> i32 {
    e.raw_os_error().unwrap_or(libc::EINVAL)
}

/// Record `e` and answer php's `false`.
fn fail(ctx: &mut Ctx, e: &io::Error) -> NativeResult {
    *last_error(ctx) = errno_of(e);
    Ok(Value::Bool(false))
}

fn fail_nix(ctx: &mut Ctx, e: nix::Error) -> NativeResult {
    *last_error(ctx) = e as i32;
    Ok(Value::Bool(false))
}

fn bool_result(ctx: &mut Ctx, r: io::Result<()>) -> NativeResult {
    match r {
        Ok(()) => Ok(Value::Bool(true)),
        Err(e) => fail(ctx, &e),
    }
}

fn int(args: &[Value], i: usize) -> i64 {
    args.get(i).map_or(0, Value::to_int)
}

fn set(a: &mut Array, key: &str, v: Value) {
    a.set(ArrayKey::str(key.as_bytes()), v);
}

/// The text `strerror(3)` gives, as std renders it without its suffix.
pub(crate) fn strerror(code: i64) -> String {
    let msg = io::Error::from_raw_os_error(code as i32).to_string();
    match msg.rfind(" (os error ") {
        Some(at) => msg[..at].to_string(),
        None => msg,
    }
}

/// A path argument as a C string, resolved against the interpreter's
/// working directory the way php's virtual cwd resolves it; `None` when it
/// carries a NUL byte.
fn c_path(ctx: &Ctx, v: &Value) -> Option<CString> {
    let bytes = v.to_php_bytes();
    if bytes.contains(&0) {
        return None;
    }
    let path = if bytes.is_empty() || bytes.starts_with(b"/") {
        bytes.to_vec()
    } else {
        let mut p = ctx.cwd.as_os_str().as_bytes().to_vec();
        if !p.ends_with(b"/") {
            p.push(b'/');
        }
        p.extend_from_slice(&bytes);
        p
    };
    CString::new(path).ok()
}

fn null_bytes(func: &str, n: usize, name: &str) -> Unwind {
    Unwind::value_error(format!("{func}(): Argument #{n} (${name}) must not contain any null bytes"))
}

/// `zend_parse_arg_long_weak` for an untyped parameter: `None` when the
/// value is no int.
fn weak_int(ctx: &mut Ctx, func: &str, v: &Value) -> Result<Option<i64>, Unwind> {
    let float = |ctx: &mut Ctx, f: f64, text: Option<String>| -> Result<Option<i64>, Unwind> {
        if !f.is_finite() || !(-9.223_372_036_854_776e18..9.223_372_036_854_776e18).contains(&f) {
            return Ok(None);
        }
        let i = f as i64;
        if i as f64 != f {
            ctx.deprecated(&match text {
                Some(s) => format!("Implicit conversion from float-string \"{s}\" to int loses precision"),
                None => format!("Implicit conversion from float {} to int loses precision", Value::Float(f).to_php_string()),
            })?;
        }
        Ok(Some(i))
    };
    Ok(match v {
        Value::Int(i) => Some(*i),
        Value::Bool(b) => Some(i64::from(*b)),
        Value::Null => {
            ctx.deprecated(&format!(
                "{func}(): Passing null to parameter #1 ($file_descriptor) of type int is deprecated"
            ))?;
            Some(0)
        }
        Value::Float(f) => float(ctx, *f, None)?,
        Value::Str(s) => match numeric_string(s.as_bytes()) {
            Some(Value::Int(i)) => Some(i),
            Some(Value::Float(f)) => float(ctx, f, Some(s.to_string_lossy().into_owned()))?,
            _ => None,
        },
        _ => None,
    })
}

/// A descriptor, and the file kept open to lend it (a plain-file stream,
/// which the stream layer keeps buffered, is reopened by its path).
struct Fd {
    fd: i32,
    _file: Option<std::fs::File>,
}

/// How a function treats a `file_descriptor` of the wrong type.
#[derive(Clone, Copy, PartialEq)]
enum WrongType {
    /// `posix_isatty()`: php's warning, and `false`.
    Refuse,
    /// `posix_ttyname()`: the warning, then `zval_get_long()` of it anyway.
    Convert,
    /// `posix_fpathconf()`: the `TypeError`.
    Throw,
}

/// php's `file_descriptor` parameter: an int, or a stream resource cast to
/// its descriptor. `Ok(None)` (after php's warning, where it gives one)
/// when there is none.
fn fd_arg(ctx: &mut Ctx, func: &str, v: &Value, wrong: WrongType) -> Result<Option<Fd>, Unwind> {
    use std::os::fd::AsRawFd;
    let v = v.deref().into_owned();
    if let Value::Resource(_) = v {
        return match rphp_stdlib::stream_fd(ctx, &v, func)? {
            StreamFd::Fd(fd) => Ok(Some(Fd { fd, _file: None })),
            StreamFd::Path(p) => match std::fs::File::open(&p) {
                Ok(f) => Ok(Some(Fd { fd: f.as_raw_fd(), _file: Some(f) })),
                Err(e) => {
                    *last_error(ctx) = errno_of(&e);
                    Ok(None)
                }
            },
            StreamFd::Unusable(kind) => {
                ctx.warn(&format!("{func}(): Could not use stream of type '{kind}'"))?;
                Ok(None)
            }
        };
    }
    let n = match weak_int(ctx, func, &v)? {
        Some(n) => n,
        None => {
            let msg = format!("{func}(): Argument #1 ($file_descriptor) must be of type int|resource, {} given", value_name(&v));
            match wrong {
                WrongType::Throw => return Err(Unwind::type_error(msg)),
                WrongType::Refuse => {
                    ctx.warn(&msg)?;
                    return Ok(None);
                }
                WrongType::Convert => {
                    ctx.warn(&msg)?;
                    if let Value::Object(_) = &v {
                        ctx.warn(&format!("Object of class {} could not be converted to int", value_name(&v)))?;
                        1
                    } else {
                        v.to_int()
                    }
                }
            }
        }
    };
    if !(0..=i64::from(i32::MAX)).contains(&n) {
        *last_error(ctx) = libc::EBADF;
        ctx.warn(&format!("{func}(): Argument #1 ($file_descriptor) must be between 0 and {}", i32::MAX))?;
        return Ok(None);
    }
    Ok(Some(Fd { fd: n as i32, _file: None }))
}

// ---- process identity ---------------------------------------------------------------

/// `posix_kill(int $process_id, int $signal): bool`
fn posix_kill(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let pid = int(args, 0);
    if i32::try_from(pid).is_err() {
        // php prints `INT_MIN` through an unsigned format.
        return Err(Unwind::value_error(format!(
            "posix_kill(): Argument #1 ($process_id) must be between {} and {}",
            i32::MIN.unsigned_abs(),
            i32::MAX
        )));
    }
    let r = sys::kill(pid, int(args, 1));
    bool_result(ctx, r)
}

fn posix_getpid(_: &mut Ctx, _: &mut [Value]) -> NativeResult {
    Ok(Value::Int(i64::from(rustix::process::getpid().as_raw_nonzero().get())))
}

fn posix_getppid(_: &mut Ctx, _: &mut [Value]) -> NativeResult {
    Ok(Value::Int(rustix::process::getppid().map_or(0, |p| i64::from(p.as_raw_nonzero().get()))))
}

fn posix_getuid(_: &mut Ctx, _: &mut [Value]) -> NativeResult {
    Ok(Value::Int(i64::from(rustix::process::getuid().as_raw())))
}

fn posix_geteuid(_: &mut Ctx, _: &mut [Value]) -> NativeResult {
    Ok(Value::Int(i64::from(rustix::process::geteuid().as_raw())))
}

fn posix_getgid(_: &mut Ctx, _: &mut [Value]) -> NativeResult {
    Ok(Value::Int(i64::from(rustix::process::getgid().as_raw())))
}

fn posix_getegid(_: &mut Ctx, _: &mut [Value]) -> NativeResult {
    Ok(Value::Int(i64::from(rustix::process::getegid().as_raw())))
}

fn id_result(ctx: &mut Ctx, r: nix::Result<()>) -> NativeResult {
    match r {
        Ok(()) => Ok(Value::Bool(true)),
        Err(e) => fail_nix(ctx, e),
    }
}

/// `posix_setuid(int $user_id): bool`
fn posix_setuid(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let r = nix::unistd::setuid(Uid::from_raw(int(args, 0) as u32));
    id_result(ctx, r)
}

/// `posix_seteuid(int $user_id): bool`
fn posix_seteuid(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let r = nix::unistd::seteuid(Uid::from_raw(int(args, 0) as u32));
    id_result(ctx, r)
}

/// `posix_setgid(int $group_id): bool`
fn posix_setgid(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let r = nix::unistd::setgid(Gid::from_raw(int(args, 0) as u32));
    id_result(ctx, r)
}

/// `posix_setegid(int $group_id): bool`
fn posix_setegid(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let r = nix::unistd::setegid(Gid::from_raw(int(args, 0) as u32));
    id_result(ctx, r)
}

/// `posix_getgroups(): array|false`
fn posix_getgroups(ctx: &mut Ctx, _: &mut [Value]) -> NativeResult {
    match rustix::process::getgroups() {
        Ok(groups) => {
            let mut a = Array::new();
            for g in groups {
                a.push(Value::Int(i64::from(g.as_raw())));
            }
            Ok(Value::Array(a))
        }
        Err(e) => fail(ctx, &e.into()),
    }
}

/// `posix_getlogin(): string|false`
fn posix_getlogin(ctx: &mut Ctx, _: &mut [Value]) -> NativeResult {
    match sys::getlogin() {
        Ok(name) => Ok(Value::string(&name)),
        Err(e) => fail(ctx, &e),
    }
}

fn posix_getpgrp(_: &mut Ctx, _: &mut [Value]) -> NativeResult {
    Ok(Value::Int(i64::from(rustix::process::getpgrp().as_raw_nonzero().get())))
}

/// `posix_setsid(): int` — `-1` on failure, with no error recorded.
fn posix_setsid(_: &mut Ctx, _: &mut [Value]) -> NativeResult {
    Ok(Value::Int(i64::from(sys::setsid())))
}

fn pid_range(func: &str, n: usize, name: &str, v: i64) -> Result<(), Unwind> {
    if !(0..=i64::from(i32::MAX)).contains(&v) {
        return Err(Unwind::value_error(format!(
            "{func}(): Argument #{n} (${name}) must be between 0 and {}",
            i32::MAX
        )));
    }
    Ok(())
}

/// `posix_setpgid(int $process_id, int $process_group_id): bool`
fn posix_setpgid(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let (pid, pgid) = (int(args, 0), int(args, 1));
    pid_range("posix_setpgid", 1, "process_id", pid)?;
    pid_range("posix_setpgid", 2, "process_group_id", pgid)?;
    let r = sys::setpgid(pid, pgid);
    bool_result(ctx, r)
}

/// `posix_getpgid(int $process_id): int|false`
fn posix_getpgid(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    match sys::getpgid(int(args, 0)) {
        Ok(g) => Ok(Value::Int(i64::from(g))),
        Err(e) => fail(ctx, &e),
    }
}

/// `posix_getsid(int $process_id): int|false`
fn posix_getsid(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let pid = int(args, 0);
    pid_range("posix_getsid", 1, "process_id", pid)?;
    match sys::getsid(pid) {
        Ok(s) => Ok(Value::Int(i64::from(s))),
        Err(e) => fail(ctx, &e),
    }
}

/// `posix_uname(): array|false`
fn posix_uname(_: &mut Ctx, _: &mut [Value]) -> NativeResult {
    let u = rustix::system::uname();
    let mut a = Array::new();
    set(&mut a, "sysname", Value::string(u.sysname().to_bytes()));
    set(&mut a, "nodename", Value::string(u.nodename().to_bytes()));
    set(&mut a, "release", Value::string(u.release().to_bytes()));
    set(&mut a, "version", Value::string(u.version().to_bytes()));
    set(&mut a, "machine", Value::string(u.machine().to_bytes()));
    #[cfg(target_os = "linux")]
    set(&mut a, "domainname", Value::string(u.domainname().to_bytes()));
    Ok(Value::Array(a))
}

/// `posix_times(): array|false` — clock ticks.
fn posix_times(ctx: &mut Ctx, _: &mut [Value]) -> NativeResult {
    match sys::times() {
        Ok(t) => {
            let mut a = Array::new();
            for (k, v) in ["ticks", "utime", "stime", "cutime", "cstime"].into_iter().zip(t) {
                set(&mut a, k, Value::Int(v));
            }
            Ok(Value::Array(a))
        }
        Err(e) => fail(ctx, &e),
    }
}

/// `posix_ctermid(): string|false`
fn posix_ctermid(_: &mut Ctx, _: &mut [Value]) -> NativeResult {
    Ok(Value::string(&sys::controlling_terminal()))
}

/// `posix_ttyname(resource|int $file_descriptor): string|false`
fn posix_ttyname(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let Some(fd) = fd_arg(ctx, "posix_ttyname", &args[0], WrongType::Convert)? else { return Ok(Value::Bool(false)) };
    match sys::ttyname(fd.fd) {
        Ok(name) => Ok(Value::string(&name)),
        Err(e) => fail(ctx, &e),
    }
}

/// `posix_isatty(resource|int $file_descriptor): bool`
fn posix_isatty(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let Some(fd) = fd_arg(ctx, "posix_isatty", &args[0], WrongType::Refuse)? else { return Ok(Value::Bool(false)) };
    let r = sys::isatty(fd.fd);
    bool_result(ctx, r)
}

/// `posix_getcwd(): string|false` — the interpreter's working directory,
/// which is what `chdir()` moved.
fn posix_getcwd(ctx: &mut Ctx, _: &mut [Value]) -> NativeResult {
    Ok(Value::string(ctx.cwd.as_os_str().as_bytes()))
}

// ---- files ----------------------------------------------------------------------------

/// `posix_mkfifo(string $filename, int $permissions): bool`
fn posix_mkfifo(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let Some(path) = c_path(ctx, &args[0]) else { return Err(null_bytes("posix_mkfifo", 1, "filename")) };
    let r = sys::mkfifo(&path, int(args, 1));
    ctx.ext.stat_cache = None;
    bool_result(ctx, r)
}

/// `posix_mknod(string $filename, int $flags, int $major = 0, int $minor = 0): bool`
fn posix_mknod(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let Some(path) = c_path(ctx, &args[0]) else { return Err(null_bytes("posix_mknod", 1, "filename")) };
    let mode = int(args, 1);
    let (major, minor) = (int(args, 2), int(args, 3));
    let kind = mode & libc::S_IFMT as i64;
    let (major, minor) = if kind == libc::S_IFCHR as i64 || kind == libc::S_IFBLK as i64 {
        if major == 0 {
            return Err(Unwind::value_error(
                "posix_mknod(): Argument #3 ($major) cannot be 0 for the POSIX_S_IFCHR and POSIX_S_IFBLK modes",
            ));
        }
        (major, minor)
    } else {
        (0, 0)
    };
    let r = sys::mknod(&path, mode, major, minor);
    ctx.ext.stat_cache = None;
    bool_result(ctx, r)
}

/// `posix_access(string $filename, int $flags = 0): bool`
fn posix_access(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let Some(path) = c_path(ctx, &args[0]) else { return Err(null_bytes("posix_access", 1, "filename")) };
    if path.as_bytes().is_empty() {
        // php's `expand_filepath()` refuses an empty path, and it reports EIO.
        *last_error(ctx) = libc::EIO;
        return Ok(Value::Bool(false));
    }
    let r = sys::access(&path, int(args, 1));
    bool_result(ctx, r)
}

// ---- users and groups ---------------------------------------------------------------

fn group_array(g: &Group) -> Value {
    let mut a = Array::new();
    set(&mut a, "name", Value::string(g.name.as_bytes()));
    set(&mut a, "passwd", Value::string(g.passwd.as_bytes()));
    let mut members = Array::new();
    for m in &g.mem {
        members.push(Value::string(m.as_bytes()));
    }
    set(&mut a, "members", Value::Array(members));
    set(&mut a, "gid", Value::Int(i64::from(g.gid.as_raw())));
    Value::Array(a)
}

fn user_array(u: &User) -> Value {
    let mut a = Array::new();
    set(&mut a, "name", Value::string(u.name.as_bytes()));
    set(&mut a, "passwd", Value::string(u.passwd.as_bytes()));
    set(&mut a, "uid", Value::Int(i64::from(u.uid.as_raw())));
    set(&mut a, "gid", Value::Int(i64::from(u.gid.as_raw())));
    set(&mut a, "gecos", Value::string(u.gecos.as_bytes()));
    set(&mut a, "dir", Value::string(u.dir.as_os_str().as_bytes()));
    set(&mut a, "shell", Value::string(u.shell.as_os_str().as_bytes()));
    Value::Array(a)
}

/// A lookup's answer: the entry, `false` for nobody (error untouched), or
/// `false` with the error recorded.
fn lookup<T>(ctx: &mut Ctx, r: nix::Result<Option<T>>, shape: fn(&T) -> Value) -> NativeResult {
    match r {
        Ok(Some(entry)) => Ok(shape(&entry)),
        Ok(None) => Ok(Value::Bool(false)),
        Err(e) => fail_nix(ctx, e),
    }
}

/// The name argument as a C-ready `&str`: a NUL byte or a name that is not
/// UTF-8 names nobody.
fn name_arg(v: &Value) -> Option<String> {
    let b = v.to_php_bytes();
    if b.is_empty() || b.contains(&0) {
        return None;
    }
    String::from_utf8(b.to_vec()).ok()
}

/// `posix_getgrnam(string $name): array|false`
fn posix_getgrnam(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let Some(name) = name_arg(&args[0]) else { return Ok(Value::Bool(false)) };
    lookup(ctx, Group::from_name(&name), group_array)
}

/// `posix_getgrgid(int $group_id): array|false`
fn posix_getgrgid(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let gid = Gid::from_raw(int(args, 0) as u32);
    lookup(ctx, Group::from_gid(gid), group_array)
}

/// `posix_getpwnam(string $username): array|false`
fn posix_getpwnam(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let Some(name) = name_arg(&args[0]) else { return Ok(Value::Bool(false)) };
    lookup(ctx, User::from_name(&name), user_array)
}

/// `posix_getpwuid(int $user_id): array|false`
fn posix_getpwuid(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let uid = Uid::from_raw(int(args, 0) as u32);
    lookup(ctx, User::from_uid(uid), user_array)
}

/// `posix_initgroups(string $username, int $group_id): bool` — php records
/// no error here.
fn posix_initgroups(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let name = args[0].to_php_bytes();
    if name.is_empty() {
        return Ok(Value::Bool(false));
    }
    let Ok(name) = CString::new(name.to_vec()) else { return Ok(Value::Bool(false)) };
    Ok(Value::Bool(sys::initgroups(&name, int(args, 1))))
}

// ---- limits ----------------------------------------------------------------------------

/// php's `posix_getrlimit()` table, in its order, as this platform has it.
const LIMITS: &[(i64, &str)] = &[
    (libc::RLIMIT_CORE as i64, "core"),
    (libc::RLIMIT_DATA as i64, "data"),
    (libc::RLIMIT_STACK as i64, "stack"),
    (libc::RLIMIT_AS as i64, "totalmem"),
    (libc::RLIMIT_RSS as i64, "rss"),
    (libc::RLIMIT_NPROC as i64, "maxproc"),
    (libc::RLIMIT_MEMLOCK as i64, "memlock"),
    (libc::RLIMIT_CPU as i64, "cpu"),
    (libc::RLIMIT_FSIZE as i64, "filesize"),
    (libc::RLIMIT_NOFILE as i64, "openfiles"),
];

fn limit_value(v: u64) -> Value {
    if v == sys::RLIM_INFINITY {
        Value::string(b"unlimited")
    } else {
        Value::Int(v as i64)
    }
}

/// `posix_getrlimit(?int $resource = null): array|false`
fn posix_getrlimit(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    if let Some(Value::Int(res)) = args.first().map(|v| v.deref().into_owned()) {
        return match sys::getrlimit(res) {
            Ok((soft, hard)) => {
                let mut a = Array::new();
                a.push(limit_value(soft));
                a.push(limit_value(hard));
                Ok(Value::Array(a))
            }
            Err(e) => fail(ctx, &e),
        };
    }
    let mut a = Array::new();
    for &(res, name) in LIMITS {
        match sys::getrlimit(res) {
            Ok((soft, hard)) => {
                set(&mut a, &format!("soft {name}"), limit_value(soft));
                set(&mut a, &format!("hard {name}"), limit_value(hard));
            }
            Err(e) => return fail(ctx, &e),
        }
    }
    Ok(Value::Array(a))
}

/// `posix_setrlimit(int $resource, int $soft_limit, int $hard_limit): bool`
/// — `-1` passes through as the C cast makes it.
fn posix_setrlimit(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let (res, soft, hard) = (int(args, 0), int(args, 1), int(args, 2));
    if soft < -1 {
        return Err(Unwind::value_error("posix_setrlimit(): Argument #2 ($soft_limit) must be greater or equal to -1"));
    }
    if hard < -1 {
        return Err(Unwind::value_error("posix_setrlimit(): Argument #3 ($hard_limit) must be greater or equal to -1"));
    }
    if hard > -1 && soft > hard {
        return Err(Unwind::value_error(format!(
            "posix_setrlimit(): Argument #2 ($soft_limit) must be lower or equal to {hard}"
        )));
    }
    let r = sys::setrlimit(res, soft, hard);
    bool_result(ctx, r)
}

/// `posix_sysconf(int $conf_id): int` — `-1` for a name the system does
/// not know, with no error recorded.
fn posix_sysconf(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    Ok(Value::Int(sys::sysconf(int(args, 0))))
}

/// `posix_pathconf(string $path, int $name): int|false`
fn posix_pathconf(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    if args[0].to_php_bytes().is_empty() {
        return Err(Unwind::value_error("posix_pathconf(): Argument #1 ($path) must not be empty"));
    }
    let Some(path) = c_path(ctx, &args[0]) else { return Err(null_bytes("posix_pathconf", 1, "path")) };
    match sys::pathconf(&path, int(args, 1)) {
        Ok(v) => Ok(Value::Int(v)),
        Err(e) => fail(ctx, &e),
    }
}

/// `posix_fpathconf(resource|int $file_descriptor, int $name): int|false`
fn posix_fpathconf(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let Some(fd) = fd_arg(ctx, "posix_fpathconf", &args[0], WrongType::Throw)? else { return Ok(Value::Bool(false)) };
    match sys::fpathconf(fd.fd, int(args, 1)) {
        Ok(v) => Ok(Value::Int(v)),
        Err(e) => fail(ctx, &e),
    }
}

// ---- errors ----------------------------------------------------------------------------

/// `posix_get_last_error(): int` (and its alias `posix_errno()`).
fn posix_get_last_error(ctx: &mut Ctx, _: &mut [Value]) -> NativeResult {
    Ok(Value::Int(i64::from(*last_error(ctx))))
}

/// `posix_strerror(int $error_code): string`
fn posix_strerror(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    Ok(Value::string(strerror(int(args, 0)).as_bytes()))
}
