//! The `unsafe` corner of the extension: the libc calls neither `rustix`
//! nor `nix` wraps safely, and the signal handler itself.
//!
//! Everything else in the crate is safe code. What lives here, and why each
//! is sound:
//!
//! * **`fork()`** — sound in a *single-threaded* process, which is what the
//!   CLI is while it runs a script. In a process with other threads the
//!   child gets only the calling thread, and a lock another thread held at
//!   the fork (an allocator's, the deadline timer's) would stay locked in
//!   the child forever. That is why `pcntl` is registered for the CLI and
//!   embed SAPIs only — php builds it for the CLI alone — and why a script
//!   that armed `set_time_limit()` (which starts the timer thread) forks at
//!   its own risk, exactly as a threaded php would.
//! * **`sigaction()`** — installs [`on_signal`], a handler that only does
//!   async-signal-safe things: atomic loads and stores into a fixed static
//!   queue, and [`rphp_runtime::Interrupt::poke`] (two atomic stores).
//!   It allocates nothing, locks nothing and leaves `errno` alone.
//! * The **plain libc calls** (`times`, `getlogin`, `waitid`, `wait4`,
//!   `sysconf`, `pathconf`, `isatty`, `ttyname_r`, `kill`, `alarm`,
//!   `sigprocmask`, `getpriority`, `initgroups`, …) take integers or
//!   pointers to locals/`CString`s that outlive the call; a raw descriptor
//!   number that is not open is the kernel's `EBADF`, never memory
//!   unsafety, since no Rust object owns or borrows it here.
//!
//! See `COVERAGE.md`, "posix / pcntl": this is the one module outside the
//! PCRE2 binding and PDO's bridge that is allowed `unsafe`.
#![allow(unsafe_code)]

use std::ffi::{c_int, c_void, CStr};
use std::io;
use std::mem::MaybeUninit;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, AtomicPtr, AtomicU64, AtomicU8, Ordering};

use rphp_runtime::Interrupt;

fn cvt(r: c_int) -> io::Result<c_int> {
    if r == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(r)
    }
}

fn clear_errno() {
    nix::errno::Errno::clear();
}

fn errno() -> i32 {
    nix::errno::Errno::last_raw()
}

// ---- process ----------------------------------------------------------------

/// Set in a child `fork()` made.
static FORKED: AtomicBool = AtomicBool::new(false);

/// `fork(2)`: the child's `0`, the parent's child pid.
pub fn fork() -> io::Result<i32> {
    // SAFETY: see the module doc — the script's thread is the only one that
    // does anything while it runs (the host's thread waits in `join` with
    // signals blocked), and the child continues on it alone.
    let pid = cvt(unsafe { libc::fork() })?;
    if pid == 0 {
        FORKED.store(true, Ordering::Release);
    }
    Ok(pid)
}

/// Whether this process is a child `pcntl_fork()` made.
pub fn forked() -> bool {
    FORKED.load(Ordering::Acquire)
}

/// A thread's signal mask, saved to be restored.
#[derive(Clone, Copy)]
pub struct Mask(libc::sigset_t);

/// Block every signal on the calling thread except the synchronous faults
/// (which must reach the thread that caused them): the previous mask.
pub fn block_async_signals() -> Mask {
    let mut set = MaybeUninit::<libc::sigset_t>::zeroed();
    let mut old = MaybeUninit::<libc::sigset_t>::zeroed();
    // SAFETY: both sets are locals, initialised by `sigfillset` /
    // `sigemptyset` before use; `pthread_sigmask` only changes this
    // thread's mask.
    unsafe {
        libc::sigfillset(set.as_mut_ptr());
        libc::sigemptyset(old.as_mut_ptr());
        for s in [libc::SIGSEGV, libc::SIGBUS, libc::SIGILL, libc::SIGFPE, libc::SIGTRAP, libc::SIGABRT] {
            libc::sigdelset(set.as_mut_ptr(), s);
        }
        libc::pthread_sigmask(libc::SIG_BLOCK, set.as_ptr(), old.as_mut_ptr());
        Mask(old.assume_init())
    }
}

/// Put a saved mask back on the calling thread.
pub fn restore_mask(m: &Mask) {
    // SAFETY: `m` holds a mask `pthread_sigmask` produced.
    unsafe {
        libc::pthread_sigmask(libc::SIG_SETMASK, &m.0, ptr::null_mut());
    }
}

/// `kill(2)` with any signal number (`0` tests for the process).
pub fn kill(pid: i64, sig: i64) -> io::Result<()> {
    let (Ok(pid), Ok(sig)) = (libc::pid_t::try_from(pid), c_int::try_from(sig)) else {
        return Err(io::Error::from_raw_os_error(libc::EINVAL));
    };
    // A signal the script sends itself goes to the script's own thread: in
    // php's single thread it is pending there while blocked and delivered
    // the moment `pcntl_sigprocmask()` unblocks it. Process-directed, darwin
    // would park it on another thread (the first) and the script would never
    // see it. (Linux keeps such a signal pending on the process, and takes
    // the sender's thread first, so plain `kill()` is already right there;
    // its `tgkill` would also report `SI_TKILL` where php reports `SI_USER`.)
    #[cfg(target_vendor = "apple")]
    if sig > 0 && sig < NSIG as c_int && pid == std::process::id() as libc::pid_t {
        // SAFETY: `pthread_self()` is the calling, live thread.
        let r = unsafe { libc::pthread_kill(libc::pthread_self(), sig) };
        return if r == 0 { Ok(()) } else { Err(io::Error::from_raw_os_error(r)) };
    }
    // SAFETY: integers only.
    cvt(unsafe { libc::kill(pid, sig) }).map(drop)
}

/// `getpgid(2)`, with php's C-cast of the pid (a negative one is the
/// kernel's `ESRCH`, which `rustix`'s positive-only `Pid` cannot express).
pub fn getpgid(pid: i64) -> io::Result<i32> {
    // SAFETY: an integer only.
    cvt(unsafe { libc::getpgid(pid as libc::pid_t) })
}

/// `getsid(2)`.
pub fn getsid(pid: i64) -> io::Result<i32> {
    // SAFETY: an integer only.
    cvt(unsafe { libc::getsid(pid as libc::pid_t) })
}

/// `setpgid(2)`.
pub fn setpgid(pid: i64, pgid: i64) -> io::Result<()> {
    // SAFETY: integers only.
    cvt(unsafe { libc::setpgid(pid as libc::pid_t, pgid as libc::pid_t) }).map(drop)
}

/// `setsid(2)`: the new session id, `-1` on failure (php reports no errno).
pub fn setsid() -> i32 {
    // SAFETY: no arguments.
    unsafe { libc::setsid() }
}

/// `alarm(3)`: the seconds left on the previous alarm.
pub fn alarm(secs: u32) -> u32 {
    // SAFETY: an integer only.
    unsafe { libc::alarm(secs) }
}

/// The resource usage `wait4(2)` reports, in php's key order.
pub struct Rusage(pub Vec<(&'static str, i64)>);

fn rusage_rows(ru: &libc::rusage) -> Rusage {
    Rusage(vec![
        ("ru_oublock", ru.ru_oublock as i64),
        ("ru_inblock", ru.ru_inblock as i64),
        ("ru_msgsnd", ru.ru_msgsnd as i64),
        ("ru_msgrcv", ru.ru_msgrcv as i64),
        ("ru_maxrss", ru.ru_maxrss as i64),
        ("ru_ixrss", ru.ru_ixrss as i64),
        ("ru_idrss", ru.ru_idrss as i64),
        ("ru_minflt", ru.ru_minflt as i64),
        ("ru_majflt", ru.ru_majflt as i64),
        ("ru_nsignals", ru.ru_nsignals as i64),
        ("ru_nvcsw", ru.ru_nvcsw as i64),
        ("ru_nivcsw", ru.ru_nivcsw as i64),
        ("ru_nswap", ru.ru_nswap as i64),
        ("ru_utime.tv_usec", ru.ru_utime.tv_usec as i64),
        ("ru_utime.tv_sec", ru.ru_utime.tv_sec as i64),
        ("ru_stime.tv_usec", ru.ru_stime.tv_usec as i64),
        ("ru_stime.tv_sec", ru.ru_stime.tv_sec as i64),
    ])
}

/// `wait4(2)`: `(pid, status, usage)`; pid `0` under `WNOHANG` with nothing
/// to reap.
///
/// `status` starts as the caller's value, which a failed wait leaves as it
/// was (php reads the by-reference argument in first).
pub fn wait4(pid: i64, flags: i64, status: i32) -> io::Result<(i32, i32, Rusage)> {
    // php's C cast: an out-of-range pid wraps, as it does there.
    let pid = pid as libc::pid_t;
    let mut status: c_int = status;
    let mut ru = MaybeUninit::<libc::rusage>::zeroed();
    // SAFETY: both out-pointers are locals that outlive the call.
    let r = cvt(unsafe { libc::wait4(pid, &mut status, flags as c_int, ru.as_mut_ptr()) })?;
    // SAFETY: zero-initialised, and filled in by the kernel on success.
    let ru = unsafe { ru.assume_init() };
    Ok((r, status, rusage_rows(&ru)))
}

/// `waitid(2)`: the child's siginfo, or `None` under `WNOHANG` with nothing
/// to report.
pub fn waitid(idtype: i64, id: i64, flags: i64) -> io::Result<Option<SigInfo>> {
    let idtype = match idtype {
        0 => libc::P_ALL,
        1 => libc::P_PID,
        2 => libc::P_PGID,
        _ => return Err(io::Error::from_raw_os_error(libc::EINVAL)),
    };
    let mut info = MaybeUninit::<libc::siginfo_t>::zeroed();
    // SAFETY: the out-pointer is a zeroed local that outlives the call.
    cvt(unsafe { libc::waitid(idtype, id as libc::id_t, info.as_mut_ptr(), flags as c_int) })?;
    // SAFETY: zeroed, and filled in by the kernel on success.
    let info = unsafe { info.assume_init() };
    if info.si_signo == 0 {
        return Ok(None);
    }
    Ok(Some(SigInfo::read(&info)))
}

/// `getpriority(2)`; `errno` decides whether `-1` is a priority.
pub fn getpriority(which: i64, who: i64) -> io::Result<i32> {
    clear_errno();
    // SAFETY: integers only.
    let r = unsafe { libc::getpriority(which as _, who as _) };
    if r == -1 && errno() != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(r)
}

/// `setpriority(2)`.
pub fn setpriority(which: i64, who: i64, prio: i64) -> io::Result<()> {
    // SAFETY: integers only.
    cvt(unsafe { libc::setpriority(which as _, who as _, prio as c_int) }).map(drop)
}

/// `times(3)`: `(ticks, utime, stime, cutime, cstime)`.
pub fn times() -> io::Result<[i64; 5]> {
    let mut t = MaybeUninit::<libc::tms>::zeroed();
    // SAFETY: the out-pointer is a zeroed local.
    let ticks = unsafe { libc::times(t.as_mut_ptr()) };
    if ticks == (-1i64) as libc::clock_t {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: zeroed, and filled in by libc.
    let t = unsafe { t.assume_init() };
    Ok([ticks as i64, t.tms_utime as i64, t.tms_stime as i64, t.tms_cutime as i64, t.tms_cstime as i64])
}

/// `getlogin(2)`, copied out of libc's static buffer at once.
pub fn getlogin() -> io::Result<Vec<u8>> {
    // SAFETY: no arguments; the result is null or a NUL-terminated string
    // that stays valid until the next call, and is copied before returning.
    let p = unsafe { libc::getlogin() };
    if p.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: non-null and NUL-terminated (above).
    Ok(unsafe { CStr::from_ptr(p) }.to_bytes().to_vec())
}

extern "C" {
    /// POSIX `ctermid(3)`, which the `libc` crate does not declare.
    fn ctermid(s: *mut std::ffi::c_char) -> *mut std::ffi::c_char;
}

/// `ctermid(3)`: the controlling terminal's path.
pub fn controlling_terminal() -> Vec<u8> {
    // Larger than any platform's `L_ctermid` (1024 on darwin, 9 on glibc).
    let mut buf = [0 as std::ffi::c_char; 1025];
    // SAFETY: the buffer is a live local of at least `L_ctermid` bytes,
    // which is all `ctermid` writes, NUL included.
    unsafe { ctermid(buf.as_mut_ptr()) };
    let bytes: Vec<u8> = buf.iter().take_while(|&&c| c != 0).map(|&c| c as u8).collect();
    bytes
}

/// `initgroups(3)`.
pub fn initgroups(name: &CStr, gid: i64) -> bool {
    // SAFETY: a NUL-terminated string the caller owns, and an integer.
    unsafe { libc::initgroups(name.as_ptr(), gid as _) == 0 }
}

// ---- files and descriptors ---------------------------------------------------

/// `sysconf(3)`, as php returns it: `-1` for an unknown name.
pub fn sysconf(name: i64) -> i64 {
    // SAFETY: an integer only.
    unsafe { libc::sysconf(name as c_int) as i64 }
}

fn conf_result(r: libc::c_long) -> io::Result<i64> {
    if r < 0 && errno() != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(r as i64)
}

/// `pathconf(2)`; `-1` with no `errno` is "no limit".
pub fn pathconf(path: &CStr, name: i64) -> io::Result<i64> {
    clear_errno();
    // SAFETY: a NUL-terminated string the caller owns, and an integer.
    conf_result(unsafe { libc::pathconf(path.as_ptr(), name as c_int) })
}

/// `fpathconf(2)` on a descriptor number.
pub fn fpathconf(fd: i32, name: i64) -> io::Result<i64> {
    clear_errno();
    // SAFETY: integers only.
    conf_result(unsafe { libc::fpathconf(fd, name as c_int) })
}

/// `isatty(3)` on a descriptor number: `Err` carries why not.
pub fn isatty(fd: i32) -> io::Result<()> {
    // SAFETY: an integer only.
    if unsafe { libc::isatty(fd) } == 1 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// `ttyname_r(3)` on a descriptor number.
pub fn ttyname(fd: i32) -> io::Result<Vec<u8>> {
    let mut buf = vec![0u8; 256];
    // SAFETY: the buffer is a live local of the length passed.
    let r = unsafe { libc::ttyname_r(fd, buf.as_mut_ptr().cast(), buf.len()) };
    if r != 0 {
        return Err(io::Error::from_raw_os_error(r));
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    buf.truncate(end);
    Ok(buf)
}

/// `access(2)` with php's mode bits passed through unmasked.
pub fn access(path: &CStr, mode: i64) -> io::Result<()> {
    // SAFETY: a NUL-terminated string the caller owns, and an integer.
    cvt(unsafe { libc::access(path.as_ptr(), mode as c_int) }).map(drop)
}

/// `mkfifo(2)`.
pub fn mkfifo(path: &CStr, mode: i64) -> io::Result<()> {
    // SAFETY: a NUL-terminated string the caller owns, and an integer.
    cvt(unsafe { libc::mkfifo(path.as_ptr(), mode as libc::mode_t) }).map(drop)
}

/// `mknod(2)` with `makedev(major, minor)`.
pub fn mknod(path: &CStr, mode: i64, major: i64, minor: i64) -> io::Result<()> {
    let dev = libc::makedev(major as _, minor as _);
    // SAFETY: a NUL-terminated string the caller owns, and integers.
    cvt(unsafe { libc::mknod(path.as_ptr(), mode as libc::mode_t, dev) }).map(drop)
}

/// `getrlimit(2)` on a raw resource number: `(soft, hard)`.
pub fn getrlimit(resource: i64) -> io::Result<(u64, u64)> {
    let mut rl = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
    // SAFETY: the out-pointer is a local.
    cvt(unsafe { libc::getrlimit(resource as _, &mut rl) })?;
    Ok((rl.rlim_cur as u64, rl.rlim_max as u64))
}

/// `setrlimit(2)` on a raw resource number.
pub fn setrlimit(resource: i64, soft: i64, hard: i64) -> io::Result<()> {
    let rl = libc::rlimit { rlim_cur: soft as libc::rlim_t, rlim_max: hard as libc::rlim_t };
    // SAFETY: the in-pointer is a local.
    cvt(unsafe { libc::setrlimit(resource as _, &rl) }).map(drop)
}

/// The process's `RLIM_INFINITY`.
pub const RLIM_INFINITY: u64 = libc::RLIM_INFINITY as u64;

// ---- signals -------------------------------------------------------------------

/// php's `num_signals`: signal numbers run from 1 to `NSIG - 1`.
#[cfg(target_os = "linux")]
pub const NSIG: i64 = 65;
#[cfg(not(target_os = "linux"))]
pub const NSIG: i64 = 32;

/// What php's `pcntl_siginfo_to_zval` reads out of a `siginfo_t`.
#[derive(Clone, Copy, Debug, Default)]
pub struct SigInfo {
    pub signo: i32,
    pub errno: i32,
    pub code: i32,
    pub pid: i32,
    pub uid: i64,
    pub status: i32,
    pub addr: i64,
}

impl SigInfo {
    fn read(si: &libc::siginfo_t) -> SigInfo {
        // SAFETY: the accessors read plain fields of a `siginfo_t` the
        // kernel filled in; which union member is meaningful is decided by
        // the caller from `signo`, as php does.
        #[cfg(target_os = "linux")]
        let (pid, uid, status, addr) = unsafe { (si.si_pid(), si.si_uid(), si.si_status(), si.si_addr() as i64) };
        #[cfg(not(target_os = "linux"))]
        let (pid, uid, status, addr) = (si.si_pid, si.si_uid, si.si_status, si.si_addr as i64);
        SigInfo {
            signo: si.si_signo,
            errno: si.si_errno,
            code: si.si_code,
            pid,
            uid: uid as i64,
            status,
            addr,
        }
    }
}

/// One queued delivery. `state`: 0 free, 1 being written, 2 ready.
struct Slot {
    state: AtomicU8,
    seq: AtomicU64,
    signo: AtomicI32,
    errno: AtomicI32,
    code: AtomicI32,
    pid: AtomicI32,
    uid: AtomicI64,
    status: AtomicI32,
    addr: AtomicI64,
}

impl Slot {
    const fn new() -> Slot {
        Slot {
            state: AtomicU8::new(0),
            seq: AtomicU64::new(0),
            signo: AtomicI32::new(0),
            errno: AtomicI32::new(0),
            code: AtomicI32::new(0),
            pid: AtomicI32::new(0),
            uid: AtomicI64::new(0),
            status: AtomicI32::new(0),
            addr: AtomicI64::new(0),
        }
    }
}

const QUEUE: usize = 64;

/// The deliveries not dispatched yet; a delivery that finds every slot
/// taken is dropped (php's queue is bounded too).
static SLOTS: [Slot; QUEUE] = [const { Slot::new() }; QUEUE];
static SEQ: AtomicU64 = AtomicU64::new(0);
/// `pcntl_async_signals(true)`: the handler pokes [`TARGET`].
static ASYNC: AtomicBool = AtomicBool::new(false);
/// The interrupt of the interpreter that asked for async signals — a
/// leaked clone, so the handler may read it at any time.
static TARGET: AtomicPtr<Interrupt> = AtomicPtr::new(ptr::null_mut());

extern "C" fn on_signal(sig: c_int, info: *mut libc::siginfo_t, _uctx: *mut c_void) {
    let si = if info.is_null() {
        SigInfo { signo: sig, ..SigInfo::default() }
    } else {
        // SAFETY: with `SA_SIGINFO` the kernel passes a valid `siginfo_t`
        // for the duration of the handler.
        SigInfo::read(unsafe { &*info })
    };
    push(si);
    if ASYNC.load(Ordering::Acquire) {
        let p = TARGET.load(Ordering::Acquire);
        if !p.is_null() {
            // SAFETY: TARGET only ever holds a leaked, never-freed box.
            unsafe { &*p }.poke();
        }
    }
}

fn push(si: SigInfo) {
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    for slot in &SLOTS {
        if slot.state.compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed).is_ok() {
            slot.seq.store(seq, Ordering::Relaxed);
            slot.signo.store(si.signo, Ordering::Relaxed);
            slot.errno.store(si.errno, Ordering::Relaxed);
            slot.code.store(si.code, Ordering::Relaxed);
            slot.pid.store(si.pid, Ordering::Relaxed);
            slot.uid.store(si.uid, Ordering::Relaxed);
            slot.status.store(si.status, Ordering::Relaxed);
            slot.addr.store(si.addr, Ordering::Relaxed);
            slot.state.store(2, Ordering::Release);
            return;
        }
    }
}

/// Take every queued delivery, oldest first.
pub fn drain() -> Vec<SigInfo> {
    let mut out: Vec<(u64, SigInfo)> = Vec::new();
    for slot in &SLOTS {
        if slot.state.load(Ordering::Acquire) == 2 {
            let si = SigInfo {
                signo: slot.signo.load(Ordering::Relaxed),
                errno: slot.errno.load(Ordering::Relaxed),
                code: slot.code.load(Ordering::Relaxed),
                pid: slot.pid.load(Ordering::Relaxed),
                uid: slot.uid.load(Ordering::Relaxed),
                status: slot.status.load(Ordering::Relaxed),
                addr: slot.addr.load(Ordering::Relaxed),
            };
            out.push((slot.seq.load(Ordering::Relaxed), si));
            slot.state.store(0, Ordering::Release);
        }
    }
    out.sort_by_key(|(seq, _)| *seq);
    out.into_iter().map(|(_, si)| si).collect()
}

/// Turn async delivery on or off; `target` is the interrupt to poke.
pub fn set_async(on: bool, target: Option<&Interrupt>) {
    if let Some(t) = target {
        let p = Box::into_raw(Box::new(t.clone()));
        // The previous target is leaked, not freed: a handler may be
        // reading it right now.
        TARGET.store(p, Ordering::Release);
    }
    ASYNC.store(on, Ordering::Release);
}

/// A signal's disposition.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Disposition {
    Default,
    Ignore,
    Catch { restart: bool },
}

/// `sigaction(2)`: every signal blocked while [`on_signal`] runs, as php's
/// `php_signal4` does.
pub fn set_disposition(sig: i64, d: Disposition) -> io::Result<()> {
    // SAFETY: a zeroed `sigaction` is a valid "default, no flags" value.
    let mut act: libc::sigaction = unsafe { std::mem::zeroed() };
    let handler: extern "C" fn(c_int, *mut libc::siginfo_t, *mut c_void) = on_signal;
    act.sa_sigaction = match d {
        Disposition::Default => libc::SIG_DFL,
        Disposition::Ignore => libc::SIG_IGN,
        Disposition::Catch { .. } => handler as libc::sighandler_t,
    };
    act.sa_flags = match d {
        Disposition::Catch { restart: true } => libc::SA_SIGINFO | libc::SA_RESTART,
        Disposition::Catch { restart: false } => libc::SA_SIGINFO,
        _ => 0,
    };
    // SAFETY: `act` is a local; `sigfillset` only writes its mask, and
    // `sigaction` reads it and installs a handler that is async-signal-safe
    // (module doc).
    unsafe {
        libc::sigfillset(&mut act.sa_mask);
        cvt(libc::sigaction(sig as c_int, &act, ptr::null_mut()))?;
    }
    Ok(())
}

/// `sigprocmask(2)` with a list of signal numbers: the previous mask, as a
/// list.
pub fn sigprocmask(how: i64, signals: &[i64]) -> io::Result<Vec<i64>> {
    let how = match how {
        1 => libc::SIG_BLOCK,
        2 => libc::SIG_UNBLOCK,
        3 => libc::SIG_SETMASK,
        _ => return Err(io::Error::from_raw_os_error(libc::EINVAL)),
    };
    let mut set = MaybeUninit::<libc::sigset_t>::zeroed();
    let mut old = MaybeUninit::<libc::sigset_t>::zeroed();
    // SAFETY: both sets are locals, initialised by `sigemptyset` before use.
    unsafe {
        libc::sigemptyset(set.as_mut_ptr());
        libc::sigemptyset(old.as_mut_ptr());
        for &s in signals {
            cvt(libc::sigaddset(set.as_mut_ptr(), s as c_int))?;
        }
        cvt(libc::sigprocmask(how, set.as_ptr(), old.as_mut_ptr()))?;
    }
    let mut out = Vec::new();
    for s in 1..NSIG {
        // SAFETY: `old` was filled in by `sigprocmask`.
        if unsafe { libc::sigismember(old.as_ptr(), s as c_int) } == 1 {
            out.push(s);
        }
    }
    Ok(out)
}
