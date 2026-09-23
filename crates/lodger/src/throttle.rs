//! Login throttling (TAD section 7.1, ASVS 6.3.1).
//!
//! After 5 failed logins in 15 minutes, for one account or from one client
//! IP, each further attempt waits: 1 second, then 2, 4, and so on, up to 60.
//! Lodger never locks an account, because a lockout lets an attacker block
//! the admin. The counts live in memory only and reset at a restart.

use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Failures allowed in the window before attempts start to wait.
pub const FREE_FAILURES: usize = 5;
pub const WINDOW: Duration = Duration::from_secs(15 * 60);
pub const MAX_DELAY: Duration = Duration::from_secs(60);
/// Keys kept at most per map, so a flood of new names cannot fill memory.
const MAX_KEYS: usize = 10_000;

#[derive(Debug, Default)]
pub struct Throttle {
    accounts: Mutex<HashMap<String, VecDeque<Instant>>>,
    ips: Mutex<HashMap<IpAddr, VecDeque<Instant>>>,
}

/// The wait for an attempt after `failures` recent failures.
pub fn delay_for(failures: usize) -> Duration {
    if failures < FREE_FAILURES {
        return Duration::ZERO;
    }
    let exponent = u32::try_from(failures - FREE_FAILURES)
        .unwrap_or(u32::MAX)
        .min(6);
    Duration::from_secs(1u64 << exponent).min(MAX_DELAY)
}

fn recent<K: std::hash::Hash + Eq>(
    map: &Mutex<HashMap<K, VecDeque<Instant>>>,
    key: &K,
    now: Instant,
) -> usize {
    let mut map = map
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(times) = map.get_mut(key) else {
        return 0;
    };
    while times
        .front()
        .is_some_and(|t| now.duration_since(*t) >= WINDOW)
    {
        times.pop_front();
    }
    times.len()
}

fn record<K: std::hash::Hash + Eq + Clone>(
    map: &Mutex<HashMap<K, VecDeque<Instant>>>,
    key: &K,
    now: Instant,
) {
    let mut map = map
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if map.len() >= MAX_KEYS && !map.contains_key(key) {
        // Drop the keys whose failures all left the window.
        map.retain(|_, times| {
            times
                .back()
                .is_some_and(|t| now.duration_since(*t) < WINDOW)
        });
        if map.len() >= MAX_KEYS {
            return;
        }
    }
    map.entry(key.clone()).or_default().push_back(now);
}

impl Throttle {
    /// How long this attempt must wait: the larger of the account's and the
    /// IP's delay. `account` is compared without regard to case.
    pub fn delay(&self, account: &str, ip: IpAddr, now: Instant) -> (Duration, usize) {
        let failures =
            recent(&self.accounts, &account.to_lowercase(), now).max(recent(&self.ips, &ip, now));
        (delay_for(failures), failures)
    }

    pub fn fail(&self, account: &str, ip: IpAddr, now: Instant) {
        record(&self.accounts, &account.to_lowercase(), now);
        record(&self.ips, &ip, now);
    }

    /// A correct login clears the account's failures. The IP keeps its
    /// count, so one good account does not reset guesses at other accounts.
    pub fn succeed(&self, account: &str) {
        self.accounts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&account.to_lowercase());
    }
}

#[cfg(test)]
mod tests {
    use std::net::IpAddr;
    use std::time::{Duration, Instant};

    use super::{MAX_DELAY, Throttle, WINDOW, delay_for};

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn the_first_five_failures_cost_nothing_then_the_wait_grows() {
        let waits: Vec<u64> = (0..13).map(|n| delay_for(n).as_secs()).collect();
        assert_eq!(waits, [0, 0, 0, 0, 0, 1, 2, 4, 8, 16, 32, 60, 60]);
        assert_eq!(delay_for(usize::MAX), MAX_DELAY);
    }

    #[test]
    fn the_sixth_attempt_for_an_account_waits() {
        let t = Throttle::default();
        let now = Instant::now();
        for n in 0..5 {
            // Different IPs: the account count alone must trigger the wait.
            assert_eq!(
                t.delay("admin", ip(&format!("10.0.0.{n}")), now).0,
                Duration::ZERO
            );
            t.fail("Admin", ip(&format!("10.0.0.{n}")), now);
        }
        assert_eq!(
            t.delay("ADMIN", ip("10.0.0.99"), now),
            (Duration::from_secs(1), 5)
        );
    }

    #[test]
    fn one_ip_guessing_many_accounts_waits_too() {
        let t = Throttle::default();
        let now = Instant::now();
        for n in 0..5 {
            t.fail(&format!("user{n}"), ip("10.0.0.1"), now);
        }
        assert_eq!(
            t.delay("someone-else", ip("10.0.0.1"), now).0,
            Duration::from_secs(1)
        );
        assert_eq!(
            t.delay("someone-else", ip("10.0.0.2"), now).0,
            Duration::ZERO
        );
    }

    #[test]
    fn failures_leave_the_window_after_15_minutes() {
        let t = Throttle::default();
        let then = Instant::now();
        for _ in 0..5 {
            t.fail("admin", ip("10.0.0.1"), then);
        }
        assert_eq!(
            t.delay("admin", ip("10.0.0.1"), then + WINDOW).0,
            Duration::ZERO
        );
    }

    #[test]
    fn a_correct_login_clears_the_account_but_not_the_ip() {
        let t = Throttle::default();
        let now = Instant::now();
        for _ in 0..5 {
            t.fail("admin", ip("10.0.0.1"), now);
        }
        t.succeed("ADMIN");
        assert_eq!(t.delay("admin", ip("10.0.0.2"), now).0, Duration::ZERO);
        assert_eq!(
            t.delay("admin", ip("10.0.0.1"), now).0,
            Duration::from_secs(1)
        );
    }
}
