//! php-src `ext/pcntl/pcntl.c`: signals, `fork`, the `wait` family,
//! `exec`, priorities, and the `SIG*` / `W*` / `PCNTL_E*` constants.
//!
//! ## Signals
//!
//! The OS handler ([`sys`]) only queues a delivery. The php handlers run
//! when the script calls `pcntl_signal_dispatch()` — or, after
//! `pcntl_async_signals(true)`, at the interpreter's next safepoint: the
//! handler pokes the interpreter's [`rphp_runtime::Interrupt`] and the
//! safepoint (a loop's back edge, a function call) runs
//! [`Interp::poke_hook`], which is [`dispatch`]. Either way a handler is
//! called as php calls it, `handler(int $signo, array $siginfo)`.
//!
//! The handlers a script installed live in the interpreter's slots; what is
//! process-wide is the OS disposition, which [`request_shutdown`] resets.

use std::collections::BTreeMap;
use std::ffi::CString;
use std::io;

use rphp_runtime::{nf, nf_ref, value_name, Ctx, Interp, NativeFn, NativeResult, Registry, Unwind};
use rphp_value::{Array, ArrayKey, Value};

use crate::posix::strerror;
use crate::sys::{self, Disposition, SigInfo, NSIG};

static FUNCTIONS: &[NativeFn] = &[
    nf!("pcntl_fork", 0, Some(0), pcntl_fork),
    nf_ref!("pcntl_waitpid", 2, Some(4), 0b1010, pcntl_waitpid),
    nf_ref!("pcntl_waitid", 0, Some(5), 0b10100, pcntl_waitid),
    nf_ref!("pcntl_wait", 1, Some(3), 0b101, pcntl_wait),
    nf!("pcntl_signal", 2, Some(3), pcntl_signal),
    nf!("pcntl_signal_get_handler", 1, Some(1), pcntl_signal_get_handler),
    nf!("pcntl_signal_dispatch", 0, Some(0), pcntl_signal_dispatch),
    nf_ref!("pcntl_sigprocmask", 2, Some(3), 0b100, pcntl_sigprocmask),
    nf!("pcntl_wifexited", 1, Some(1), pcntl_wifexited),
    nf!("pcntl_wifstopped", 1, Some(1), pcntl_wifstopped),
    nf!("pcntl_wifcontinued", 1, Some(1), pcntl_wifcontinued),
    nf!("pcntl_wifsignaled", 1, Some(1), pcntl_wifsignaled),
    nf!("pcntl_wexitstatus", 1, Some(1), pcntl_wexitstatus),
    nf!("pcntl_wtermsig", 1, Some(1), pcntl_wtermsig),
    nf!("pcntl_wstopsig", 1, Some(1), pcntl_wstopsig),
    nf!("pcntl_exec", 1, Some(3), pcntl_exec),
    nf!("pcntl_alarm", 1, Some(1), pcntl_alarm),
    nf!("pcntl_get_last_error", 0, Some(0), pcntl_get_last_error),
    nf!("pcntl_errno", 0, Some(0), pcntl_get_last_error),
    nf!("pcntl_getpriority", 0, Some(2), pcntl_getpriority),
    nf!("pcntl_setpriority", 1, Some(3), pcntl_setpriority),
    nf!("pcntl_strerror", 1, Some(1), pcntl_strerror),
    nf!("pcntl_async_signals", 0, Some(1), pcntl_async_signals),
];

const SIG_DFL: i64 = 0;
const SIG_IGN: i64 = 1;

/// The constants, with this platform's values, in php's order.
fn constants() -> Vec<(&'static str, i64)> {
    let mut c: Vec<(&'static str, i64)> = vec![
        ("WNOHANG", libc::WNOHANG as i64),
        ("WUNTRACED", libc::WUNTRACED as i64),
        ("WCONTINUED", libc::WCONTINUED as i64),
        ("WEXITED", libc::WEXITED as i64),
        ("WSTOPPED", libc::WSTOPPED as i64),
        ("WNOWAIT", libc::WNOWAIT as i64),
        ("P_ALL", 0),
        ("P_PID", 1),
        ("P_PGID", 2),
        ("SIG_IGN", SIG_IGN),
        ("SIG_DFL", SIG_DFL),
        ("SIG_ERR", -1),
        ("SIGHUP", libc::SIGHUP as i64),
        ("SIGINT", libc::SIGINT as i64),
        ("SIGQUIT", libc::SIGQUIT as i64),
        ("SIGILL", libc::SIGILL as i64),
        ("SIGTRAP", libc::SIGTRAP as i64),
        ("SIGABRT", libc::SIGABRT as i64),
        ("SIGIOT", libc::SIGIOT as i64),
        ("SIGBUS", libc::SIGBUS as i64),
        ("SIGFPE", libc::SIGFPE as i64),
        ("SIGKILL", libc::SIGKILL as i64),
        ("SIGUSR1", libc::SIGUSR1 as i64),
        ("SIGSEGV", libc::SIGSEGV as i64),
        ("SIGUSR2", libc::SIGUSR2 as i64),
        ("SIGPIPE", libc::SIGPIPE as i64),
        ("SIGALRM", libc::SIGALRM as i64),
        ("SIGTERM", libc::SIGTERM as i64),
    ];
    #[cfg(target_os = "linux")]
    c.push(("SIGSTKFLT", libc::SIGSTKFLT as i64));
    c.extend([
        ("SIGCHLD", libc::SIGCHLD as i64),
        ("SIGCONT", libc::SIGCONT as i64),
        ("SIGSTOP", libc::SIGSTOP as i64),
        ("SIGTSTP", libc::SIGTSTP as i64),
        ("SIGTTIN", libc::SIGTTIN as i64),
        ("SIGTTOU", libc::SIGTTOU as i64),
        ("SIGURG", libc::SIGURG as i64),
        ("SIGXCPU", libc::SIGXCPU as i64),
        ("SIGXFSZ", libc::SIGXFSZ as i64),
        ("SIGVTALRM", libc::SIGVTALRM as i64),
        ("SIGPROF", libc::SIGPROF as i64),
        ("SIGWINCH", libc::SIGWINCH as i64),
    ]);
    #[cfg(target_os = "linux")]
    c.extend([("SIGPOLL", libc::SIGPOLL as i64)]);
    c.push(("SIGIO", libc::SIGIO as i64));
    #[cfg(target_os = "linux")]
    c.push(("SIGPWR", libc::SIGPWR as i64));
    #[cfg(target_vendor = "apple")]
    c.push(("SIGINFO", libc::SIGINFO as i64));
    c.extend([
        ("SIGSYS", libc::SIGSYS as i64),
        ("SIGBABY", libc::SIGSYS as i64),
    ]);
    #[cfg(target_os = "linux")]
    c.extend([("SIGRTMIN", 34), ("SIGRTMAX", 64)]);
    c.extend([
        ("PRIO_PGRP", libc::PRIO_PGRP as i64),
        ("PRIO_USER", libc::PRIO_USER as i64),
        ("PRIO_PROCESS", libc::PRIO_PROCESS as i64),
    ]);
    #[cfg(target_vendor = "apple")]
    c.extend([("PRIO_DARWIN_BG", 0x1000), ("PRIO_DARWIN_THREAD", 3)]);
    c.extend([
        ("SIG_BLOCK", libc::SIG_BLOCK as i64),
        ("SIG_UNBLOCK", libc::SIG_UNBLOCK as i64),
        ("SIG_SETMASK", libc::SIG_SETMASK as i64),
        ("PCNTL_EINTR", libc::EINTR as i64),
        ("PCNTL_ECHILD", libc::ECHILD as i64),
        ("PCNTL_EINVAL", libc::EINVAL as i64),
        ("PCNTL_EAGAIN", libc::EAGAIN as i64),
        ("PCNTL_ESRCH", libc::ESRCH as i64),
        ("PCNTL_EACCES", libc::EACCES as i64),
        ("PCNTL_EPERM", libc::EPERM as i64),
        ("PCNTL_ENOMEM", libc::ENOMEM as i64),
        ("PCNTL_E2BIG", libc::E2BIG as i64),
        ("PCNTL_EFAULT", libc::EFAULT as i64),
        ("PCNTL_EIO", libc::EIO as i64),
        ("PCNTL_EISDIR", libc::EISDIR as i64),
        ("PCNTL_ELOOP", libc::ELOOP as i64),
        ("PCNTL_EMFILE", libc::EMFILE as i64),
        ("PCNTL_ENAMETOOLONG", libc::ENAMETOOLONG as i64),
        ("PCNTL_ENFILE", libc::ENFILE as i64),
        ("PCNTL_ENOENT", libc::ENOENT as i64),
        ("PCNTL_ENOEXEC", libc::ENOEXEC as i64),
        ("PCNTL_ENOTDIR", libc::ENOTDIR as i64),
        ("PCNTL_ETXTBSY", libc::ETXTBSY as i64),
        ("PCNTL_ENOSPC", libc::ENOSPC as i64),
        ("PCNTL_EUSERS", libc::EUSERS as i64),
    ]);
    c
}

pub(crate) fn register(r: &mut Registry) {
    r.functions(FUNCTIONS);
    for (name, v) in constants() {
        r.constant(name, Value::Int(v));
    }
}

// ---- state ---------------------------------------------------------------------------

#[derive(Default)]
struct State {
    /// `pcntl_get_last_error()`.
    last_error: i32,
    /// What `pcntl_signal()` stored per signal: a callable, or `SIG_DFL` /
    /// `SIG_IGN`.
    handlers: BTreeMap<i64, Value>,
    /// `pcntl_async_signals()`.
    async_on: bool,
    /// Whether the OS handler's poke target is this interpreter.
    armed: bool,
    /// Set while [`dispatch`] runs, so a poke inside a handler does not
    /// dispatch recursively (php's `processing_signal_queue`).
    dispatching: bool,
}

const SLOT: &str = "pcntl.state";

fn state(it: &mut Interp) -> &mut State {
    let slot = it.ext.slots.entry(SLOT).or_insert_with(|| Box::new(State::default()));
    if !slot.is::<State>() {
        *slot = Box::new(State::default());
    }
    slot.downcast_mut::<State>().expect("the slot was just made a State")
}

fn errno_of(e: &io::Error) -> i32 {
    e.raw_os_error().unwrap_or(libc::EINVAL)
}

fn int(args: &[Value], i: usize) -> i64 {
    args.get(i).map_or(0, Value::to_int)
}

fn set(a: &mut Array, key: &str, v: Value) {
    a.set(ArrayKey::str(key.as_bytes()), v);
}

/// `request_shutdown`: the dispositions this request installed go back to
/// the default, and async delivery is switched off.
pub(crate) fn request_shutdown(it: &mut Interp) {
    let Some(st) = it.ext.slots.get_mut(SLOT).and_then(|s| s.downcast_mut::<State>()) else { return };
    for &sig in st.handlers.keys() {
        let _ = sys::set_disposition(sig, Disposition::Default);
    }
    st.handlers.clear();
    if st.armed {
        sys::set_async(false, None);
        st.armed = false;
    }
    it.poke_hook = None;
}

// ---- signals ----------------------------------------------------------------------------

/// php's `pcntl_siginfo_to_zval`: the fields every signal has, then the
/// ones its kind carries.
fn siginfo_array(si: &SigInfo) -> Array {
    let mut a = Array::new();
    set(&mut a, "signo", Value::Int(i64::from(si.signo)));
    set(&mut a, "errno", Value::Int(i64::from(si.errno)));
    set(&mut a, "code", Value::Int(i64::from(si.code)));
    match si.signo {
        libc::SIGCHLD => {
            set(&mut a, "status", Value::Int(i64::from(si.status)));
            set(&mut a, "pid", Value::Int(i64::from(si.pid)));
            set(&mut a, "uid", Value::Int(si.uid));
        }
        libc::SIGUSR1 | libc::SIGUSR2 => {
            set(&mut a, "pid", Value::Int(i64::from(si.pid)));
            set(&mut a, "uid", Value::Int(si.uid));
        }
        libc::SIGILL | libc::SIGFPE | libc::SIGSEGV | libc::SIGBUS => {
            set(&mut a, "addr", Value::Float(si.addr as f64));
        }
        _ => {}
    }
    a
}

/// Run the php handlers of every queued delivery, oldest first — what
/// `pcntl_signal_dispatch()` does, and the interpreter's poke hook.
fn dispatch(it: &mut Interp) -> Result<(), Unwind> {
    if state(it).dispatching {
        return Ok(());
    }
    state(it).dispatching = true;
    let r = run_queue(it);
    state(it).dispatching = false;
    r
}

fn run_queue(it: &mut Interp) -> Result<(), Unwind> {
    loop {
        let queue = sys::drain();
        if queue.is_empty() {
            return Ok(());
        }
        for si in queue {
            let signo = i64::from(si.signo);
            let Some(handler) = state(it).handlers.get(&signo).cloned() else { continue };
            if matches!(handler, Value::Int(_)) {
                continue;
            }
            it.call_value(&handler, &[Value::Int(signo), Value::Array(siginfo_array(&si))])?;
        }
    }
}

/// Point the OS handler's poke at this interpreter (once per request).
fn arm(ctx: &mut Ctx) {
    let on = state(ctx).async_on;
    if !state(ctx).armed {
        let target = ctx.interrupt.clone();
        sys::set_async(on, Some(&target));
        state(ctx).armed = true;
        ctx.poke_hook = Some(dispatch);
    } else {
        sys::set_async(on, None);
    }
}

fn check_signo(func: &str, signo: i64) -> Result<(), Unwind> {
    if signo < 1 {
        return Err(Unwind::value_error(format!("{func}(): Argument #1 ($signal) must be greater than or equal to 1")));
    }
    if signo >= NSIG {
        return Err(Unwind::value_error(format!("{func}(): Argument #1 ($signal) must be less than {NSIG}")));
    }
    Ok(())
}

/// `pcntl_signal(int $signal, callable|int $handler, bool $restart_syscalls = true): bool`
fn pcntl_signal(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let signo = int(args, 0);
    check_signo("pcntl_signal", signo)?;
    let handler = args[1].deref().into_owned();
    let restart = args.get(2).is_none_or(Value::to_bool);
    let disposition = match handler {
        Value::Int(SIG_DFL) => Disposition::Default,
        Value::Int(SIG_IGN) => Disposition::Ignore,
        Value::Int(_) => {
            return Err(Unwind::value_error(
                "pcntl_signal(): Argument #2 ($handler) must be either SIG_DFL or SIG_IGN when an integer value is given",
            ))
        }
        ref h if ctx.is_callable(h) => Disposition::Catch { restart },
        ref h => {
            state(ctx).last_error = libc::EINVAL;
            return Err(Unwind::type_error(format!(
                "pcntl_signal(): Argument #2 ($handler) must be of type callable|int, {} given",
                value_name(h)
            )));
        }
    };
    if signo == i64::from(libc::SIGKILL) || signo == i64::from(libc::SIGSTOP) {
        // The engine's signal layer refuses these outright (a fatal, not
        // pcntl's warning).
        return Err(ctx.fatal(&format!("Error installing signal handler for {signo}")));
    }
    if let Err(e) = sys::set_disposition(signo, disposition) {
        state(ctx).last_error = errno_of(&e);
        ctx.warn("pcntl_signal(): Error assigning signal")?;
        return Ok(Value::Bool(false));
    }
    state(ctx).handlers.insert(signo, handler);
    if matches!(disposition, Disposition::Catch { .. }) {
        arm(ctx);
    }
    Ok(Value::Bool(true))
}

/// `pcntl_signal_get_handler(int $signal): callable|int`
fn pcntl_signal_get_handler(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let signo = int(args, 0);
    if !(1..NSIG).contains(&signo) {
        return Err(Unwind::value_error(format!(
            "pcntl_signal_get_handler(): Argument #1 ($signal) must be between 1 and {}",
            NSIG - 1
        )));
    }
    Ok(state(ctx).handlers.get(&signo).cloned().unwrap_or(Value::Int(SIG_DFL)))
}

/// `pcntl_signal_dispatch(): bool`
fn pcntl_signal_dispatch(ctx: &mut Ctx, _: &mut [Value]) -> NativeResult {
    dispatch(ctx)?;
    Ok(Value::Bool(true))
}

/// `pcntl_async_signals(?bool $enable = null): bool` — the previous setting.
fn pcntl_async_signals(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let old = state(ctx).async_on;
    match args.first().map(|v| v.deref().into_owned()) {
        None | Some(Value::Null) => {}
        Some(v) => {
            state(ctx).async_on = v.to_bool();
            arm(ctx);
        }
    }
    Ok(Value::Bool(old))
}

/// A signal number out of `pcntl_sigprocmask()`'s list: `zval_try_get_long`.
fn list_signo(ctx: &mut Ctx, v: &Value) -> Result<i64, Unwind> {
    let v = v.deref().into_owned();
    let n = match &v {
        Value::Int(i) => Some(*i),
        Value::Null => Some(0),
        Value::Bool(b) => Some(i64::from(*b)),
        Value::Float(f) if f.is_finite() => {
            if f.fract() != 0.0 {
                ctx.deprecated(&format!(
                    "Implicit conversion from float {} to int loses precision",
                    v.to_php_string()
                ))?;
            }
            Some(*f as i64)
        }
        Value::Str(s) => match rphp_value::numeric_string(s.as_bytes()) {
            Some(Value::Int(i)) => Some(i),
            Some(Value::Float(f)) => Some(f as i64),
            _ => None,
        },
        _ => None,
    };
    let Some(n) = n else {
        return Err(Unwind::type_error(format!(
            "pcntl_sigprocmask(): Argument #2 ($signals) signals must be of type int, {} given",
            value_name(&v)
        )));
    };
    if !(1..NSIG).contains(&n) {
        return Err(Unwind::value_error(format!(
            "pcntl_sigprocmask(): Argument #2 ($signals) signals must be between 1 and {}",
            NSIG - 1
        )));
    }
    Ok(n)
}

/// `pcntl_sigprocmask(int $mode, array $signals, &$old_signals = null): bool`
fn pcntl_sigprocmask(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let how = int(args, 0);
    if !matches!(how, 1..=3) {
        return Err(Unwind::value_error(
            "pcntl_sigprocmask(): Argument #1 ($mode) must be one of SIG_BLOCK, SIG_UNBLOCK, or SIG_SETMASK",
        ));
    }
    let list = match &*args[1].deref() {
        Value::Array(a) => a.clone(),
        _ => Array::new(),
    };
    if list.is_empty() {
        return Err(Unwind::value_error("pcntl_sigprocmask(): Argument #2 ($signals) must not be empty"));
    }
    let mut signals = Vec::new();
    for (_, v) in list.iter() {
        signals.push(list_signo(ctx, v)?);
    }
    match sys::sigprocmask(how, &signals) {
        Ok(old) => {
            if args.len() > 2 {
                let mut a = Array::new();
                for s in old {
                    a.push(Value::Int(s));
                }
                args[2] = Value::Array(a);
            }
            Ok(Value::Bool(true))
        }
        Err(e) => {
            state(ctx).last_error = errno_of(&e);
            ctx.warn(&format!("pcntl_sigprocmask(): {}", strerror(i64::from(errno_of(&e)))))?;
            Ok(Value::Bool(false))
        }
    }
}

/// `pcntl_alarm(int $seconds): int` — the C cast to `unsigned` included.
fn pcntl_alarm(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    Ok(Value::Int(i64::from(sys::alarm(int(args, 0) as u32))))
}

// ---- processes -----------------------------------------------------------------------------

/// `pcntl_fork(): int`
///
/// Output the engine has staged is flushed first, so it is not written
/// twice — once by each process.
fn pcntl_fork(ctx: &mut Ctx, _: &mut [Value]) -> NativeResult {
    ctx.out.flush_sink();
    match sys::fork() {
        Ok(pid) => Ok(Value::Int(i64::from(pid))),
        Err(e) => {
            let n = errno_of(&e);
            state(ctx).last_error = n;
            let msg = match n {
                libc::EAGAIN => format!("pcntl_fork(): Error {n}: Reached the maximum limit of number of processes"),
                libc::ENOMEM => format!("pcntl_fork(): Error {n}: Insufficient memory"),
                _ => format!("pcntl_fork(): Error {n}"),
            };
            ctx.warn(&msg)?;
            Ok(Value::Int(-1))
        }
    }
}

fn rusage_array(ru: &sys::Rusage) -> Value {
    let mut a = Array::new();
    for &(k, v) in &ru.0 {
        set(&mut a, k, Value::Int(v));
    }
    Value::Array(a)
}

/// `wait4()` for `pcntl_waitpid()` / `pcntl_wait()`: the status and usage
/// go to the by-reference slots given.
fn wait_common(ctx: &mut Ctx, args: &mut [Value], pid: i64, flags: i64, st: usize, ru: usize) -> NativeResult {
    let before = args[st].to_int() as i32;
    match sys::wait4(pid, flags, before) {
        Ok((child, status, usage)) => {
            args[st] = Value::Int(i64::from(status));
            if args.len() > ru {
                args[ru] = if child > 0 { rusage_array(&usage) } else { Value::Array(Array::new()) };
            }
            Ok(Value::Int(i64::from(child)))
        }
        Err(e) => {
            state(ctx).last_error = errno_of(&e);
            args[st] = Value::Int(i64::from(before));
            if args.len() > ru {
                args[ru] = Value::Array(Array::new());
            }
            Ok(Value::Int(-1))
        }
    }
}

/// `pcntl_waitpid(int $process_id, &$status, int $flags = 0, &$resource_usage = []): int`
fn pcntl_waitpid(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let (pid, flags) = (int(args, 0), int(args, 2));
    wait_common(ctx, args, pid, flags, 1, 3)
}

/// `pcntl_wait(&$status, int $flags = 0, &$resource_usage = []): int`
fn pcntl_wait(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let flags = int(args, 1);
    wait_common(ctx, args, -1, flags, 0, 2)
}

/// `pcntl_waitid(int $idtype = P_ALL, ?int $id = null, &$info = [], int $flags = WEXITED, &$resource_usage = []): bool`
///
/// `$resource_usage` is only filled where the platform has `wait6(2)`
/// (FreeBSD); elsewhere php leaves it alone, and so does this.
fn pcntl_waitid(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let idtype = if args.is_empty() { 0 } else { int(args, 0) };
    let id = int(args, 1);
    let flags = if args.len() > 3 { int(args, 3) } else { libc::WEXITED as i64 };
    match sys::waitid(idtype, id, flags) {
        Ok(info) => {
            if args.len() > 2 {
                let si = info.unwrap_or_default();
                args[2] = Value::Array(siginfo_array(&si));
            }
            Ok(Value::Bool(true))
        }
        Err(e) => {
            state(ctx).last_error = errno_of(&e);
            Ok(Value::Bool(false))
        }
    }
}

fn status(args: &[Value]) -> i32 {
    int(args, 0) as i32
}

fn pcntl_wifexited(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    Ok(Value::Bool(libc::WIFEXITED(status(args))))
}

fn pcntl_wifstopped(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    Ok(Value::Bool(libc::WIFSTOPPED(status(args))))
}

fn pcntl_wifcontinued(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    Ok(Value::Bool(libc::WIFCONTINUED(status(args))))
}

fn pcntl_wifsignaled(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    Ok(Value::Bool(libc::WIFSIGNALED(status(args))))
}

fn pcntl_wexitstatus(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    Ok(Value::Int(i64::from(libc::WEXITSTATUS(status(args)))))
}

fn pcntl_wtermsig(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    Ok(Value::Int(i64::from(libc::WTERMSIG(status(args)))))
}

fn pcntl_wstopsig(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    Ok(Value::Int(i64::from(libc::WSTOPSIG(status(args)))))
}

/// A value as `zval_try_get_string` makes it, with the array warning.
fn arg_string(ctx: &mut Ctx, v: &Value) -> Result<Vec<u8>, Unwind> {
    if matches!(&*v.deref(), Value::Array(_)) {
        ctx.warn("Array to string conversion")?;
        return Ok(b"Array".to_vec());
    }
    Ok(v.to_php_bytes().to_vec())
}

/// `pcntl_exec(string $path, array $args = [], array $env_vars = []): false`
fn pcntl_exec(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let path = args[0].to_php_bytes().to_vec();
    let Ok(cpath) = CString::new(path.clone()) else {
        return Err(Unwind::value_error("pcntl_exec(): Argument #1 ($path) must not contain any null bytes"));
    };
    let mut argv = vec![cpath.clone()];
    if let Some(Value::Array(a)) = args.get(1).map(|v| v.deref().into_owned()) {
        for (_, v) in a.iter() {
            let bytes = arg_string(ctx, v)?;
            let Ok(c) = CString::new(bytes) else {
                return Err(Unwind::value_error(
                    "pcntl_exec(): Argument #2 ($args) individual argument must not contain null bytes",
                ));
            };
            argv.push(c);
        }
    }
    let env = match args.get(2).map(|v| v.deref().into_owned()) {
        Some(Value::Array(a)) => {
            let mut env = Vec::new();
            for (k, v) in a.iter() {
                let mut entry = match k {
                    ArrayKey::Int(i) => i.to_string().into_bytes(),
                    ArrayKey::Str(s) => s.as_bytes().to_vec(),
                };
                if entry.contains(&0) {
                    return Err(Unwind::value_error(
                        "pcntl_exec(): Argument #3 ($env_vars) name for environment variable must not contain null bytes",
                    ));
                }
                let value = arg_string(ctx, v)?;
                if value.contains(&0) {
                    return Err(Unwind::value_error(
                        "pcntl_exec(): Argument #3 ($env_vars) value for environment variable must not contain null bytes",
                    ));
                }
                entry.push(b'=');
                entry.extend_from_slice(&value);
                env.push(CString::new(entry).expect("checked for NUL above"));
            }
            Some(env)
        }
        _ => None,
    };
    ctx.out.flush_sink();
    let err = match env {
        Some(env) => nix::unistd::execve(&cpath, &argv, &env),
        None => nix::unistd::execv(&cpath, &argv),
    };
    let n = match err {
        Err(e) => e as i32,
        Ok(never) => match never {},
    };
    state(ctx).last_error = n;
    ctx.warn(&format!("pcntl_exec(): Error has occurred: (errno {n}) {}", strerror(i64::from(n))))?;
    Ok(Value::Bool(false))
}

// ---- priorities -------------------------------------------------------------------------------

/// The modes php names when the kernel refuses a call with `EINVAL`: a
/// refusal under one of these blames the id, under any other the mode.
/// (php asks the kernel first; its `errno` is recorded either way.)
fn valid_mode(mode: i64) -> bool {
    let base = [libc::PRIO_PGRP as i64, libc::PRIO_USER as i64, libc::PRIO_PROCESS as i64];
    base.contains(&mode) || (cfg!(target_vendor = "apple") && mode == 3)
}

#[cfg(target_vendor = "apple")]
const MODES: &str = "PRIO_PGRP, PRIO_USER, PRIO_PROCESS or PRIO_DARWIN_THREAD";
#[cfg(not(target_vendor = "apple"))]
const MODES: &str = "PRIO_PGRP, PRIO_USER, or PRIO_PROCESS";

/// php's report of a failed `getpriority` / `setpriority`: `pid_arg` and
/// `mode_arg` are the positions its `ValueError`s name.
fn priority_error(ctx: &mut Ctx, func: &str, e: &io::Error, pid_arg: usize, mode_arg: usize, mode: i64) -> NativeResult {
    let n = errno_of(e);
    state(ctx).last_error = n;
    let msg = match n {
        libc::ESRCH => format!("{func}(): Error {n}: No process was located using the given parameters"),
        libc::EINVAL if valid_mode(mode) => {
            return Err(Unwind::value_error(format!(
                "{func}(): Argument #{pid_arg} ($process_id) is not a valid process, process group, or user ID"
            )))
        }
        libc::EINVAL => {
            return Err(Unwind::value_error(format!("{func}(): Argument #{mode_arg} ($mode) must be one of {MODES}")))
        }
        libc::EPERM => format!(
            "{func}(): Error {n}: A process was located, but neither its effective nor real user ID matched the effective user ID of the caller"
        ),
        libc::EACCES => format!("{func}(): Error {n}: Only a super user may attempt to increase the process priority"),
        _ => format!("{func}(): Unknown error {n} has occurred"),
    };
    ctx.warn(&msg)?;
    Ok(Value::Bool(false))
}

/// `pcntl_getpriority(?int $process_id = null, int $mode = PRIO_PROCESS): int|false`
fn pcntl_getpriority(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let pid = int(args, 0);
    let mode = if args.len() > 1 { int(args, 1) } else { libc::PRIO_PROCESS as i64 };
    match sys::getpriority(mode, pid) {
        Ok(p) => Ok(Value::Int(i64::from(p))),
        Err(e) => priority_error(ctx, "pcntl_getpriority", &e, 1, 2, mode),
    }
}

/// `pcntl_setpriority(int $priority, ?int $process_id = null, int $mode = PRIO_PROCESS): bool`
fn pcntl_setpriority(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let prio = int(args, 0);
    let pid = int(args, 1);
    let mode = if args.len() > 2 { int(args, 2) } else { libc::PRIO_PROCESS as i64 };
    match sys::setpriority(mode, pid, prio) {
        Ok(()) => Ok(Value::Bool(true)),
        Err(e) => priority_error(ctx, "pcntl_setpriority", &e, 2, 3, mode),
    }
}

// ---- errors ----------------------------------------------------------------------------------------

/// `pcntl_get_last_error(): int` (and its alias `pcntl_errno()`).
fn pcntl_get_last_error(ctx: &mut Ctx, _: &mut [Value]) -> NativeResult {
    Ok(Value::Int(i64::from(state(ctx).last_error)))
}

/// `pcntl_strerror(int $error_code): string`
fn pcntl_strerror(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    Ok(Value::string(strerror(int(args, 0)).as_bytes()))
}
