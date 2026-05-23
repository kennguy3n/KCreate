//! Lamport clock for total ordering of remote operations.
//!
//! Each peer maintains a monotonically increasing 64-bit counter:
//!
//! * On **send**: increment, attach the new value to the outgoing
//!   envelope.
//! * On **receive**: set the local counter to
//!   `max(local, received) + 1`.
//!
//! This is the classic Lamport algorithm. It guarantees that if
//! message *A* causally precedes message *B*, then `A.clock < B.clock`;
//! the converse does not hold (concurrent messages from different
//! peers can produce equal clocks). The [`crate::conflict`] module
//! breaks ties using peer ids when that happens.
//!
//! 2^64 events at one event per nanosecond is ~584 years, so wrap-around
//! is a non-issue in practice; we panic on overflow rather than wrap
//! silently because a wrap would silently break ordering.

use serde::{Deserialize, Serialize};

/// A 64-bit Lamport timestamp. The default value is `0`, which is
/// also the value of a session that has not sent or received anything
/// yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LamportClock(u64);

impl LamportClock {
    /// Construct a clock at the given value. Mostly useful for tests
    /// and for restoring a persisted session.
    #[must_use]
    pub const fn from_raw(v: u64) -> Self {
        Self(v)
    }

    /// Borrow the current value as a `u64`.
    #[must_use]
    pub const fn as_u64(self) -> u64 {
        self.0
    }

    /// Increment in-place by 1 and return the **new** value. Use this
    /// at the moment of *send*.
    ///
    /// # Panics
    ///
    /// Panics on overflow rather than wrap (we'd rather crash than
    /// silently break ordering).
    #[must_use = "the returned value is the Lamport clock to attach to the outgoing message"]
    pub fn tick(&mut self) -> Self {
        self.0 = self.0.checked_add(1).expect("Lamport clock overflow");
        *self
    }

    /// Observe a remote clock value and advance the local clock to
    /// `max(local, remote) + 1`. Return the new local value.
    ///
    /// # Panics
    ///
    /// Panics on overflow (see [`Self::tick`]).
    #[must_use = "the returned value is the new local Lamport clock after observing the remote"]
    pub fn observe(&mut self, remote: Self) -> Self {
        let new = self.0.max(remote.0);
        self.0 = new.checked_add(1).expect("Lamport clock overflow");
        *self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_zero() {
        assert_eq!(LamportClock::default().as_u64(), 0);
    }

    #[test]
    fn tick_increments_by_one() {
        let mut c = LamportClock::default();
        assert_eq!(c.tick().as_u64(), 1);
        assert_eq!(c.tick().as_u64(), 2);
        assert_eq!(c.as_u64(), 2);
    }

    #[test]
    fn observe_takes_max_plus_one() {
        let mut c = LamportClock::from_raw(3);
        // Observing a smaller remote still bumps local by 1.
        assert_eq!(c.observe(LamportClock::from_raw(1)).as_u64(), 4);
        // Observing a larger remote jumps to remote + 1.
        assert_eq!(c.observe(LamportClock::from_raw(10)).as_u64(), 11);
    }

    #[test]
    fn ordering_preserves_happens_before() {
        // Peer A sends op_a (clock 1). Peer B observes it (clock 2),
        // then sends op_b (clock 3). op_a must order before op_b.
        let mut a = LamportClock::default();
        let mut b = LamportClock::default();
        let a_clock = a.tick();
        let _ = b.observe(a_clock);
        let b_clock = b.tick();
        assert!(a_clock < b_clock);
    }

    #[test]
    #[should_panic(expected = "Lamport clock overflow")]
    fn tick_panics_on_overflow() {
        let mut c = LamportClock::from_raw(u64::MAX);
        let _ = c.tick();
    }

    #[test]
    fn json_round_trip() {
        let c = LamportClock::from_raw(42);
        let j = serde_json::to_string(&c).unwrap();
        // The transparent attribute means we get a bare number.
        assert_eq!(j, "42");
        let back: LamportClock = serde_json::from_str(&j).unwrap();
        assert_eq!(back, c);
    }
}
