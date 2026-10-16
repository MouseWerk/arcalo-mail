//! Noticing that the computer woke up or changed networks, so the accounts reconnect at once
//! instead of waiting for a dead connection to time out or for the next backoff step.
//!
//! The shell samples every few seconds: the wall clock, the monotonic clock and the
//! addresses of the network interfaces. [`Watch::sample`] compares with the last sample and
//! says what happened; the shell then sends `engine::Command::Reconnect` to every worker. A
//! sleep shows as the wall clock running ahead of the monotonic one (which stands still while
//! suspended on Linux and macOS) or as a gap far longer than the sampling interval; a network
//! change as a different set of addresses. No connection is opened for this.

use std::time::{Duration, Instant};

/// How often the shell samples.
pub const INTERVAL: Duration = Duration::from_secs(5);

/// A wall-clock jump larger than this (beyond the monotonic time) is a sleep.
const SLEEP_GAP: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// The computer slept and woke up.
    Woke,
    /// The addresses changed (another Wi-Fi, a VPN, a cable).
    NetworkChanged,
    /// No address before, one now.
    CameOnline,
}

#[derive(Debug, Default)]
pub struct Watch {
    last: Option<(i64, Instant)>,
    addresses: Option<Vec<String>>,
}

impl Watch {
    /// One sample: the wall clock in milliseconds, the monotonic clock, and the addresses of the
    /// interfaces other than loopback.
    pub fn sample(&mut self, wall_ms: i64, mono: Instant, mut addresses: Vec<String>) -> Vec<Change> {
        let mut out = vec![];
        if let Some((wall0, mono0)) = self.last {
            let wall = Duration::from_millis((wall_ms - wall0).max(0) as u64);
            let mono_gap = mono.saturating_duration_since(mono0);
            if wall.saturating_sub(mono_gap) > SLEEP_GAP || mono_gap > INTERVAL * 6 {
                out.push(Change::Woke);
            }
        }
        self.last = Some((wall_ms, mono));
        addresses.sort();
        addresses.dedup();
        if let Some(before) = &self.addresses
            && *before != addresses
        {
            out.push(if before.is_empty() && !addresses.is_empty() {
                Change::CameOnline
            } else {
                Change::NetworkChanged
            });
        }
        self.addresses = Some(addresses);
        out
    }
}

/// Whether the workers should reconnect now (an address went away without a new one does not
/// help a connection; the next attempt fails and backs off as before).
pub fn reconnect(changes: &[Change]) -> bool {
    !changes.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sleep_and_network_changes() {
        let mut w = Watch::default();
        let t0 = Instant::now();
        let a = || vec!["192.168.1.20".to_string()];
        assert!(w.sample(1_000_000, t0, a()).is_empty(), "the first sample only sets the stage");
        assert!(w.sample(1_005_000, t0 + INTERVAL, a()).is_empty());
        // Suspended for an hour: the wall clock moved, the monotonic one did not.
        assert_eq!(w.sample(4_605_000, t0 + INTERVAL * 2, a()), [Change::Woke]);
        // A long stall of the sampling thread counts as well.
        assert_eq!(w.sample(4_705_000, t0 + INTERVAL * 2 + Duration::from_secs(100), a()), [Change::Woke]);
        let later = t0 + INTERVAL * 30;
        w.sample(4_800_000, later, a());
        assert_eq!(w.sample(4_805_000, later + INTERVAL, vec!["10.0.0.7".into()]), [Change::NetworkChanged]);
        assert_eq!(w.sample(4_810_000, later + INTERVAL * 2, vec![]), [Change::NetworkChanged]);
        assert_eq!(w.sample(4_815_000, later + INTERVAL * 3, vec!["10.0.0.7".into()]), [Change::CameOnline]);
        assert!(w.sample(4_820_000, later + INTERVAL * 4, vec!["10.0.0.7".into()]).is_empty());
        assert!(reconnect(&[Change::Woke]) && !reconnect(&[]));
    }
}
