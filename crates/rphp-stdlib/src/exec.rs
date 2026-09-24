//! php-src `ext/standard/exec.c` and `proc_open.c`: the shell escapes, the
//! four shell-out functions and `proc_open` with its pipes.
//!
//! A string command runs through `/bin/sh -c`, as php's does; an array is
//! executed directly (php ≥ 7.4). The pipes are [`crate::file`] streams
//! over the child's descriptors, so `fread`/`fwrite`/`feof`/
//! `stream_set_blocking`/`stream_select` work on them the way Symfony's
//! `Process` expects.
//!
//! ## The host's say
//!
//! An embedding host may refuse a spawn: it stores a [`SpawnPolicy`] under
//! [`POLICY_SLOT`] in `Interp::ext.slots`, and every function here asks it
//! before forking. A refusal is php's warning for a failed spawn and
//! `false`, so a script sees an ordinary failure, not an engine error. With
//! no policy installed nothing is refused — the CLI behaves as php does.

use std::process::{Child, Command, Stdio};

use rphp_runtime::{nf, nf_ref, Ctx, NativeFn, NativeResult, Unwind};
use rphp_value::{Array, ArrayKey, Str, Value};

/// This extension's registry contribution (see `lib.rs`).
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf!("escapeshellarg", 1, Some(1), escapeshellarg),
    nf!("escapeshellcmd", 1, Some(1), escapeshellcmd),
    nf_ref!("exec", 1, Some(3), 0b110, exec),
    nf!("shell_exec", 1, Some(1), shell_exec),
    nf_ref!("system", 1, Some(2), 0b10, system),
    nf_ref!("passthru", 1, Some(2), 0b10, passthru),
    nf_ref!("proc_open", 3, Some(6), 0b100, proc_open),
    nf!("proc_close", 1, Some(1), proc_close),
    nf!("proc_get_status", 1, Some(1), proc_get_status),
    nf!("proc_terminate", 1, Some(2), proc_terminate),
];

/// What a spawn is about to run, for the host's policy.
#[derive(Debug)]
pub struct SpawnRequest<'a> {
    /// The function asking (`proc_open`, `exec`, …).
    pub function: &'static str,
    /// The program and its arguments; `["/bin/sh", "-c", command]` for a
    /// string command.
    pub argv: &'a [Vec<u8>],
    /// The working directory the child starts in, when the script named one.
    pub cwd: Option<&'a std::path::Path>,
}

/// The host's spawn policy: `Err(reason)` refuses.
pub type SpawnPolicy = Box<dyn Fn(&SpawnRequest<'_>) -> Result<(), String>>;

/// The `Interp::ext.slots` key a [`SpawnPolicy`] is installed under.
pub const POLICY_SLOT: &str = "exec.spawn-policy";

/// Ask the host; `Ok(false)` after php's warning when it refused.
pub(crate) fn permitted(ctx: &mut Ctx, req: &SpawnRequest<'_>) -> Result<bool, Unwind> {
    let verdict = match ctx.ext.slots.get(POLICY_SLOT).and_then(|p| p.downcast_ref::<SpawnPolicy>()) {
        Some(policy) => policy(req),
        None => Ok(()),
    };
    match verdict {
        Ok(()) => Ok(true),
        Err(reason) => {
            ctx.warn(&format!("{}(): Unable to fork [{}]: {reason}", req.function, String::from_utf8_lossy(&req.argv.join(&b' '))))?;
            Ok(false)
        }
    }
}

/// `/bin/sh -c command`, as php runs a string.
pub(crate) fn shell_argv(command: &[u8]) -> Vec<Vec<u8>> {
    vec![b"/bin/sh".to_vec(), b"-c".to_vec(), command.to_vec()]
}

fn os(bytes: &[u8]) -> std::ffi::OsString {
    use std::os::unix::ffi::OsStringExt;
    std::ffi::OsString::from_vec(bytes.to_vec())
}

pub(crate) fn command_for(argv: &[Vec<u8>]) -> Command {
    let mut c = Command::new(os(&argv[0]));
    c.args(argv[1..].iter().map(|a| os(a)));
    c
}

/// Run a shell command to completion with the parent's stdin and stderr,
/// capturing stdout; `None` when it could not be started.
fn run_shell(ctx: &mut Ctx, function: &'static str, command: &[u8]) -> Result<Option<(Vec<u8>, i32)>, Unwind> {
    let argv = shell_argv(command);
    if !permitted(ctx, &SpawnRequest { function, argv: &argv, cwd: None })? {
        return Ok(None);
    }
    let mut c = command_for(&argv);
    c.stdin(Stdio::inherit()).stdout(Stdio::piped()).stderr(Stdio::inherit()).current_dir(&ctx.cwd);
    let out = match c.output() {
        Ok(o) => o,
        Err(e) => {
            ctx.warn(&format!("{function}(): Unable to fork [{}]: {e}", String::from_utf8_lossy(command)))?;
            return Ok(None);
        }
    };
    Ok(Some((out.stdout, out.status.code().unwrap_or(-1))))
}

/// php's `exec()` line split: trailing whitespace off every line, and the
/// output array holds the lines (a trailing newline makes no empty entry).
fn lines(out: &[u8]) -> Vec<Vec<u8>> {
    if out.is_empty() {
        return Vec::new();
    }
    let mut v: Vec<Vec<u8>> = out
        .split(|&b| b == b'\n')
        .map(|l| {
            let mut l = l.to_vec();
            while l.last().is_some_and(|b| b.is_ascii_whitespace()) {
                l.pop();
            }
            l
        })
        .collect();
    if out.last() == Some(&b'\n') {
        v.pop();
    }
    v
}

/// `exec(string $command, array &$output = null, int &$result_code = null): string|false`
fn exec(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let command = args[0].to_php_bytes();
    if command.is_empty() {
        return Err(Unwind::value_error("exec(): Argument #1 ($command) cannot be empty"));
    }
    let Some((out, code)) = run_shell(ctx, "exec", &command)? else { return Ok(Value::Bool(false)) };
    let lines = lines(&out);
    if args.len() > 1 {
        // php appends to an existing array.
        let mut arr = match &*args[1].deref() {
            Value::Array(a) => a.clone(),
            _ => Array::new(),
        };
        for l in &lines {
            arr.push(Value::Str(Str::from_vec(l.clone())));
        }
        args[1] = Value::Array(arr);
    }
    if args.len() > 2 {
        args[2] = Value::Int(i64::from(code));
    }
    Ok(Value::Str(Str::from_vec(lines.last().cloned().unwrap_or_default())))
}

/// `shell_exec(string $command): string|false|null` — the whole output;
/// `null` when there was none.
fn shell_exec(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let command = args[0].to_php_bytes();
    if command.is_empty() {
        return Err(Unwind::value_error("shell_exec(): Argument #1 ($command) cannot be empty"));
    }
    let Some((out, _)) = run_shell(ctx, "shell_exec", &command)? else { return Ok(Value::Bool(false)) };
    Ok(if out.is_empty() { Value::Null } else { Value::Str(Str::from_vec(out)) })
}

/// `system(string $command, int &$result_code = null): string|false` — the
/// output is echoed as it comes; the last line is returned.
fn system(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let command = args[0].to_php_bytes();
    if command.is_empty() {
        return Err(Unwind::value_error("system(): Argument #1 ($command) cannot be empty"));
    }
    let Some((out, code)) = run_shell(ctx, "system", &command)? else { return Ok(Value::Bool(false)) };
    ctx.echo(&out);
    if args.len() > 1 {
        args[1] = Value::Int(i64::from(code));
    }
    Ok(Value::Str(Str::from_vec(lines(&out).last().cloned().unwrap_or_default())))
}

/// `passthru(string $command, int &$result_code = null): ?false` — raw
/// output straight through.
fn passthru(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let command = args[0].to_php_bytes();
    if command.is_empty() {
        return Err(Unwind::value_error("passthru(): Argument #1 ($command) cannot be empty"));
    }
    let Some((out, code)) = run_shell(ctx, "passthru", &command)? else { return Ok(Value::Bool(false)) };
    ctx.echo(&out);
    if args.len() > 1 {
        args[1] = Value::Int(i64::from(code));
    }
    Ok(Value::Null)
}

// ---- proc_open ----------------------------------------------------------------

/// A `process` resource.
struct Proc {
    child: Child,
    /// What `proc_get_status()['command']` reports.
    command: Vec<u8>,
    /// The exit status once collected: php ≥ 8.3 keeps reporting it.
    status: Option<std::process::ExitStatus>,
}

impl Proc {
    fn poll(&mut self) -> Option<std::process::ExitStatus> {
        if self.status.is_none() {
            self.status = self.child.try_wait().ok().flatten();
        }
        self.status
    }
}

fn proc_arg(ctx: &mut Ctx, v: &Value, func: &str) -> Result<rphp_value::Resource, Unwind> {
    match &*v.deref() {
        Value::Resource(r) if r.kind() == "process" => Ok(r.clone()),
        other => {
            let _ = ctx;
            Err(Unwind::type_error(format!(
                "{func}(): Argument #1 ($process) must be of type resource, {} given",
                rphp_runtime::value_name(&other)
            )))
        }
    }
}

fn with_proc<R>(ctx: &mut Ctx, v: &Value, func: &str, f: impl FnOnce(&mut Proc) -> R) -> Result<R, Unwind> {
    let r = proc_arg(ctx, v, func)?;
    let mut payload = r.payload_mut();
    let Some(p) = payload.as_mut().and_then(|a| a.downcast_mut::<Proc>()) else {
        return Err(Unwind::type_error(format!("{func}(): supplied resource is not a valid process resource")));
    };
    Ok(f(p))
}

/// What one descriptor of the spec becomes.
enum Desc {
    Pipe { readable_for_parent: bool },
    Stdio(Stdio),
}

/// `proc_open(array|string $command, array $descriptor_spec, array &$pipes, ?string $cwd = null, ?array $env_vars = null, ?array $options = null): resource|false`
fn proc_open(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    // The command.
    let (argv, shown): (Vec<Vec<u8>>, Vec<u8>) = match &*args[0].deref() {
        Value::Array(a) => {
            let argv: Vec<Vec<u8>> = a.iter().map(|(_, v)| v.deref().to_php_bytes()).collect();
            if argv.is_empty() {
                return Err(Unwind::value_error("proc_open(): Argument #1 ($command) must have at least one element"));
            }
            // php reports the program alone for an array command.
            let shown = argv[0].clone();
            (argv, shown)
        }
        other => {
            let c = other.to_php_bytes();
            if c.is_empty() {
                return Err(Unwind::value_error("proc_open(): Argument #1 ($command) cannot be empty"));
            }
            (shell_argv(&c), c)
        }
    };
    let cwd = match args.get(3) {
        Some(v) if !matches!(*v.deref(), Value::Null) => Some(std::path::PathBuf::from(os(&v.to_php_bytes()))),
        _ => None,
    };
    if !permitted(ctx, &SpawnRequest { function: "proc_open", argv: &argv, cwd: cwd.as_deref() })? {
        return Ok(Value::Bool(false));
    }

    // The descriptors: php's `['pipe', mode]`, `['file', path, mode]`, a
    // stream resource, or (unnamed) the parent's own.
    let spec = match &*args[1].deref() {
        Value::Array(a) => a.clone(),
        _ => Array::new(),
    };
    let mut descs: Vec<(i64, Desc)> = Vec::new();
    for (k, v) in spec.iter() {
        let ArrayKey::Int(n) = k else { continue };
        let v = v.deref().into_owned();
        let d = match &v {
            Value::Array(d) => {
                let kind = d.get(&ArrayKey::Int(0)).map(|v| v.to_php_bytes()).unwrap_or_default();
                match kind.as_slice() {
                    b"pipe" => {
                        let mode = d.get(&ArrayKey::Int(1)).map(|v| v.to_php_bytes()).unwrap_or_default();
                        // 'r' is the child's reading end: the parent writes.
                        Desc::Pipe { readable_for_parent: !mode.starts_with(b"r") }
                    }
                    b"file" => {
                        let path = std::path::PathBuf::from(os(&d.get(&ArrayKey::Int(1)).map(|v| v.to_php_bytes()).unwrap_or_default()));
                        let mode = d.get(&ArrayKey::Int(2)).map(|v| v.to_php_bytes()).unwrap_or_else(|| b"r".to_vec());
                        let write = mode.iter().any(|b| matches!(b, b'w' | b'a' | b'+' | b'c' | b'x'));
                        let mut o = std::fs::OpenOptions::new();
                        o.read(!write || mode.contains(&b'+')).write(write).create(write).append(mode.starts_with(b"a")).truncate(mode.starts_with(b"w"));
                        match o.open(&path) {
                            Ok(f) => Desc::Stdio(Stdio::from(f)),
                            Err(e) => {
                                ctx.warn(&format!("proc_open(): Unable to open specified file: {} (mode {}): {e}", path.display(), String::from_utf8_lossy(&mode)))?;
                                return Ok(Value::Bool(false));
                            }
                        }
                    }
                    b"pty" => {
                        ctx.warn("proc_open(): PTY (pseudoterminal) not supported on this system")?;
                        return Ok(Value::Bool(false));
                    }
                    other => {
                        ctx.warn(&format!("proc_open(): {} is not a valid descriptor spec/mode", String::from_utf8_lossy(other)))?;
                        return Ok(Value::Bool(false));
                    }
                }
            }
            Value::Resource(_) => {
                if let Some(f) = crate::file::pipe_dup(ctx, &v, "proc_open")? {
                    Desc::Stdio(Stdio::from(f))
                } else if crate::file::std_sink(ctx, &v, "proc_open")?.is_some() {
                    Desc::Stdio(Stdio::inherit())
                } else if let Some(p) = crate::file::stream_path(ctx, &v, "proc_open")? {
                    match std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&p).or_else(|_| std::fs::File::open(&p)) {
                        Ok(f) => Desc::Stdio(Stdio::from(f)),
                        Err(_) => Desc::Stdio(Stdio::inherit()),
                    }
                } else {
                    Desc::Stdio(Stdio::inherit())
                }
            }
            _ => {
                ctx.warn(&format!("proc_open(): Descriptor item must be either an array or a File-Handle (index {n})"))?;
                return Ok(Value::Bool(false));
            }
        };
        descs.push((*n, d));
    }

    let mut cmd = command_for(&argv);
    cmd.stdin(Stdio::inherit()).stdout(Stdio::inherit()).stderr(Stdio::inherit());
    let mut wants: [Option<bool>; 3] = [None, None, None];
    for (n, d) in descs {
        if !(0..=2).contains(&n) {
            ctx.warn(&format!("proc_open(): descriptor {n} beyond the standard three is not supported"))?;
            return Ok(Value::Bool(false));
        }
        let stdio = match d {
            Desc::Pipe { readable_for_parent } => {
                wants[n as usize] = Some(readable_for_parent);
                Stdio::piped()
            }
            Desc::Stdio(s) => s,
        };
        match n {
            0 => cmd.stdin(stdio),
            1 => cmd.stdout(stdio),
            _ => cmd.stderr(stdio),
        };
    }
    cmd.current_dir(cwd.as_deref().unwrap_or(&ctx.cwd));
    // `env_vars` replaces the environment; `K => V` pairs or `"K=V"` items.
    if let Some(Value::Array(env)) = args.get(4).map(|v| v.deref().into_owned()) {
        cmd.env_clear();
        for (k, v) in env.iter() {
            match k {
                ArrayKey::Str(name) => {
                    cmd.env(os(name.as_bytes()), os(&v.deref().to_php_bytes()));
                }
                ArrayKey::Int(_) => {
                    let pair = v.deref().to_php_bytes();
                    if let Some(eq) = pair.iter().position(|&b| b == b'=') {
                        cmd.env(os(&pair[..eq]), os(&pair[eq + 1..]));
                    }
                }
            }
        }
    }

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            ctx.warn(&format!("proc_open(): posix_spawn() failed: {}", e.to_string().split(" (os error").next().unwrap_or("")))?;
            return Ok(Value::Bool(false));
        }
    };
    // The parent's ends, as `$pipes`.
    let mut pipes = Array::new();
    if let Some(readable) = wants[0] {
        if let Some(stdin) = child.stdin.take() {
            pipes.set(ArrayKey::Int(0), crate::file::pipe_resource(ctx, std::fs::File::from(std::os::fd::OwnedFd::from(stdin)), readable));
        }
    }
    if let Some(readable) = wants[1] {
        if let Some(stdout) = child.stdout.take() {
            pipes.set(ArrayKey::Int(1), crate::file::pipe_resource(ctx, std::fs::File::from(std::os::fd::OwnedFd::from(stdout)), readable));
        }
    }
    if let Some(readable) = wants[2] {
        if let Some(stderr) = child.stderr.take() {
            pipes.set(ArrayKey::Int(2), crate::file::pipe_resource(ctx, std::fs::File::from(std::os::fd::OwnedFd::from(stderr)), readable));
        }
    }
    args[2] = Value::Array(pipes);
    Ok(ctx.resources.add("process", Box::new(Proc { child, command: shown, status: None })))
}

/// `proc_close(resource $process): int` — wait for the child; its exit
/// code, `-1` when a signal ended it.
fn proc_close(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let v = args[0].clone();
    let code = with_proc(ctx, &v, "proc_close", |p| {
        let status = match p.status {
            Some(s) => Some(s),
            None => p.child.wait().ok(),
        };
        p.status = status;
        status.and_then(|s| s.code()).unwrap_or(-1)
    })?;
    ctx.resources.close_value(&v);
    Ok(Value::Int(i64::from(code)))
}

/// `proc_get_status(resource $process): array` — php's eight keys.
fn proc_get_status(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    use std::os::unix::process::ExitStatusExt;
    let v = args[0].clone();
    let (command, pid, status) = with_proc(ctx, &v, "proc_get_status", |p| (p.command.clone(), p.child.id(), p.poll()))?;
    let mut out = Array::new();
    let mut set = |k: &str, v: Value| out.set(ArrayKey::str(k.as_bytes()), v);
    set("command", Value::Str(Str::from_vec(command)));
    set("pid", Value::Int(i64::from(pid)));
    set("running", Value::Bool(status.is_none()));
    set("signaled", Value::Bool(status.is_some_and(|s| s.signal().is_some())));
    set("stopped", Value::Bool(status.is_some_and(|s| s.stopped_signal().is_some())));
    set("exitcode", Value::Int(status.and_then(|s| s.code()).map_or(-1, i64::from)));
    set("termsig", Value::Int(status.and_then(|s| s.signal()).map_or(0, i64::from)));
    set("stopsig", Value::Int(status.and_then(|s| s.stopped_signal()).map_or(0, i64::from)));
    Ok(Value::Array(out))
}

/// `proc_terminate(resource $process, int $signal = 15): bool`
fn proc_terminate(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let v = args[0].clone();
    let signal = args.get(1).map_or(15, Value::to_int);
    let pid = with_proc(ctx, &v, "proc_terminate", |p| if p.poll().is_some() { None } else { Some(p.child.id()) })?;
    let Some(pid) = pid else { return Ok(Value::Bool(false)) };
    let (Some(pid), Some(sig)) = (rustix::process::Pid::from_raw(pid as i32), rustix::process::Signal::from_named_raw(signal as i32)) else {
        return Ok(Value::Bool(false));
    };
    Ok(Value::Bool(rustix::process::kill_process(pid, sig).is_ok()))
}

/// `escapeshellarg(string $arg): string` — one shell word: the whole string
/// in single quotes, with each `'` closed, escaped and reopened.
fn escapeshellarg(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let s = args[0].to_php_bytes();
    let mut out = Vec::with_capacity(s.len() + 2);
    out.push(b'\'');
    for &b in s.iter() {
        if b == b'\'' {
            out.extend_from_slice(b"'\\''");
        } else {
            out.push(b);
        }
    }
    out.push(b'\'');
    Ok(Value::Str(Str::from_vec(out)))
}

/// `escapeshellcmd(string $command): string` — escape the metacharacters of a
/// *command line*, which is a weaker guarantee than quoting an argument.
///
/// Two rules are easy to miss. A quote is escaped only when it has no partner
/// later in the string, so `"a&b"` keeps its quotes (and still escapes the
/// `&` between them) while a lone `"` is escaped. And a `0xFF` byte is
/// **dropped**, not escaped — php removes it outright.
fn escapeshellcmd(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let s = args[0].to_php_bytes();
    let mut out = Vec::with_capacity(s.len());
    for (i, &b) in s.iter().enumerate() {
        match b {
            0xFF => {}
            b'"' | b'\'' => {
                // Paired: leave both alone. php looks for the next identical
                // quote anywhere in the rest of the string.
                if s[i + 1..].contains(&b) || out.contains(&b) && paired_before(&s[..i], b) {
                    out.push(b);
                } else {
                    out.push(b'\\');
                    out.push(b);
                }
            }
            b'#' | b'&' | b';' | b'`' | b'|' | b'*' | b'?' | b'~' | b'<' | b'>' | b'^' | b'('
            | b')' | b'[' | b']' | b'{' | b'}' | b'$' | b'\\' | 0x0A => {
                out.push(b'\\');
                out.push(b);
            }
            _ => out.push(b),
        }
    }
    Ok(Value::Str(Str::from_vec(out)))
}

/// Whether `q` already opened a pair in the part of the string behind us, so
/// this one closes it rather than dangling.
fn paired_before(before: &[u8], q: u8) -> bool {
    before.iter().filter(|&&b| b == q).count() % 2 == 1
}

/// No constants: `escapeshellcmd`'s character set is not exposed.
pub(crate) fn register_constants(_r: &mut rphp_runtime::Registry) {}
