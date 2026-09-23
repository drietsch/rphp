//! The host's interrupt: one atomic the dispatch loop polls at its
//! safepoints — backward jumps and frame activation — and a deadline timer
//! that raises it (spec 06 §safepoints; the runner's WP1).
//!
//! Raising is cheap and thread-safe: a host thread, a timer or a signal
//! handler stores a flag; the interpreter notices at its next safepoint and
//! turns it into php's fatal (`Maximum execution time of N seconds
//! exceeded`, or whatever reason the host gave). The take is **one-shot**:
//! the flag clears as the fatal is raised, so shutdown functions and
//! destructors run — a host that wants a grace period raises again.
//!
//! Natives are not interrupted mid-call (a `sleep()`, a long `preg_match`):
//! cooperative, like php's own timer, which fires between opcodes.
//!
//! A *poke* ([`Interrupt::poke`]) raises the same flag without a reason:
//! what a signal handler does when `pcntl_async_signals(true)` is on. The
//! safepoint that finds a poke runs the interpreter's `poke_hook` (the
//! queued php signal handlers) and carries on, instead of raising a fatal.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// The shared flag. Clone it to hand a raiser to another thread.
#[derive(Clone, Debug, Default)]
pub struct Interrupt(Arc<Inner>);

#[derive(Debug, Default)]
struct Inner {
    raised: AtomicBool,
    /// Raised by [`Interrupt::poke`]: work for the poke hook, not a fatal.
    poked: AtomicBool,
    reason: Mutex<Option<String>>,
}

impl Interrupt {
    pub fn new() -> Interrupt {
        Interrupt::default()
    }

    /// Raise the interrupt with the message the fatal will carry.
    pub fn raise(&self, reason: impl Into<String>) {
        *self.0.reason.lock().unwrap_or_else(|p| p.into_inner()) = Some(reason.into());
        self.0.raised.store(true, Ordering::Release);
    }

    /// The safepoint check: one relaxed load.
    #[inline]
    #[must_use]
    pub fn is_raised(&self) -> bool {
        self.0.raised.load(Ordering::Relaxed)
    }

    /// Consume the interrupt: `Some(reason)` if it was raised, clearing it.
    pub fn take(&self) -> Option<String> {
        if !self.0.raised.swap(false, Ordering::AcqRel) {
            return None;
        }
        Some(
            self.0
                .reason
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .take()
                .unwrap_or_else(|| "Execution interrupted by the host".to_string()),
        )
    }

    /// Raise the flag *without* a reason: two atomic stores, no lock and no
    /// allocation, so a signal handler may call it (async-signal-safe). The
    /// safepoint that notices it consumes it with [`Interrupt::take_poke`].
    pub fn poke(&self) {
        self.0.poked.store(true, Ordering::Release);
        self.0.raised.store(true, Ordering::Release);
    }

    /// Whether a poke is pending (and no more than that is known).
    #[inline]
    #[must_use]
    pub fn is_poked(&self) -> bool {
        self.0.poked.load(Ordering::Relaxed)
    }

    /// Consume a pending poke: `true` if there was one. The flag is lowered
    /// unless a real interrupt (one with a reason) is pending as well, or a
    /// new poke raced in meanwhile.
    pub fn take_poke(&self) -> bool {
        if !self.0.poked.swap(false, Ordering::AcqRel) {
            return false;
        }
        let reason = self.0.reason.lock().unwrap_or_else(|p| p.into_inner());
        if reason.is_none() {
            self.0.raised.store(false, Ordering::Release);
            if self.0.poked.load(Ordering::Acquire) {
                self.0.raised.store(true, Ordering::Release);
            }
        }
        true
    }

    /// Lower the flag without consuming a reason.
    pub fn clear(&self) {
        self.0.poked.store(false, Ordering::Release);
        self.0.raised.store(false, Ordering::Release);
        self.0.reason.lock().unwrap_or_else(|p| p.into_inner()).take();
    }

    /// Raise at `when` unless the returned [`Deadline`] is dropped first.
    #[must_use]
    pub fn raise_at(&self, when: Instant, reason: impl Into<String>) -> Deadline {
        Deadline { id: timer().arm(when, self.clone(), reason.into()) }
    }

    /// Raise after `after` unless the returned [`Deadline`] is dropped first.
    #[must_use]
    pub fn raise_after(&self, after: Duration, reason: impl Into<String>) -> Deadline {
        self.raise_at(Instant::now() + after, reason)
    }
}

/// An armed deadline; dropping it disarms.
#[derive(Debug)]
pub struct Deadline {
    id: u64,
}

impl Drop for Deadline {
    fn drop(&mut self) {
        timer().disarm(self.id);
    }
}

/// One process-wide timer thread, started on first use, sleeping until the
/// earliest deadline.
struct Timer {
    state: Mutex<TimerState>,
    wake: Condvar,
}

#[derive(Default)]
struct TimerState {
    next_id: u64,
    /// Ordered by `(when, id)` so the earliest deadline is the first entry.
    entries: BTreeMap<(Instant, u64), (Interrupt, String)>,
    when_of: std::collections::HashMap<u64, Instant>,
}

fn timer() -> &'static Timer {
    static TIMER: OnceLock<&'static Timer> = OnceLock::new();
    TIMER.get_or_init(|| {
        let t: &'static Timer = Box::leak(Box::new(Timer { state: Mutex::new(TimerState::default()), wake: Condvar::new() }));
        std::thread::Builder::new()
            .name("rphp-deadline".into())
            .spawn(move || t.run())
            .expect("the deadline timer thread");
        t
    })
}

impl Timer {
    fn arm(&self, when: Instant, interrupt: Interrupt, reason: String) -> u64 {
        let mut s = self.state.lock().unwrap_or_else(|p| p.into_inner());
        s.next_id += 1;
        let id = s.next_id;
        s.entries.insert((when, id), (interrupt, reason));
        s.when_of.insert(id, when);
        drop(s);
        self.wake.notify_one();
        id
    }

    fn disarm(&self, id: u64) {
        let mut s = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(when) = s.when_of.remove(&id) {
            s.entries.remove(&(when, id));
        }
    }

    fn run(&self) {
        let mut s = self.state.lock().unwrap_or_else(|p| p.into_inner());
        loop {
            let now = Instant::now();
            match s.entries.first_key_value().map(|(k, _)| *k) {
                Some((when, id)) if when <= now => {
                    let (interrupt, reason) = s.entries.remove(&(when, id)).expect("the first entry");
                    s.when_of.remove(&id);
                    drop(s);
                    interrupt.raise(reason);
                    s = self.state.lock().unwrap_or_else(|p| p.into_inner());
                }
                Some((when, _)) => {
                    let (guard, _) = self.wake.wait_timeout(s, when - now).unwrap_or_else(|p| p.into_inner());
                    s = guard;
                }
                None => {
                    s = self.wake.wait(s).unwrap_or_else(|p| p.into_inner());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raise_and_take_are_one_shot() {
        let i = Interrupt::new();
        assert!(!i.is_raised());
        assert_eq!(i.take(), None);
        i.raise("stop");
        assert!(i.is_raised());
        assert_eq!(i.take().as_deref(), Some("stop"));
        assert!(!i.is_raised());
        assert_eq!(i.take(), None);
    }

    #[test]
    fn deadline_fires_unless_dropped() {
        let fires = Interrupt::new();
        let _armed = fires.raise_after(Duration::from_millis(30), "late");
        let cancelled = Interrupt::new();
        let armed = cancelled.raise_after(Duration::from_millis(30), "never");
        drop(armed);
        std::thread::sleep(Duration::from_millis(120));
        assert_eq!(fires.take().as_deref(), Some("late"));
        assert!(!cancelled.is_raised());
    }

    #[test]
    fn earlier_deadline_armed_later_fires_first() {
        let a = Interrupt::new();
        let b = Interrupt::new();
        let _late = a.raise_after(Duration::from_millis(200), "a");
        let _soon = b.raise_after(Duration::from_millis(20), "b");
        std::thread::sleep(Duration::from_millis(80));
        assert!(b.is_raised());
        assert!(!a.is_raised());
    }
}
