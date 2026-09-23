//! Login throttling (TAD section 7.1, ASVS 6.3.1).
//!
//! After 5 failed logins in 15 minutes, for one account or from one client
//! IP, each further attempt waits: 1 second, then 2, 4, and so on, up to 60.
//! Lodger never locks an account, because a lockout lets an attacker block
//! the admin. The counts live in memory only and reset at a restart.
//!
//! An attempt counts as a failure from the moment it starts, and a correct
//! login takes its count back. So parallel attempts see each other. Attempts
//! from one IP also queue: each starts only after the wait of the one before
//! it. An attempt that would wait more than [`MAX_DELAY`] in that queue gets
//! no turn at all, and the caller answers 429.

use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, Ipv6Addr};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use lodger_core::validate::NAME_MAX_LEN;

/// Failures allowed in the window before attempts start to wait.
pub const FREE_FAILURES: usize = 5;
pub const WINDOW: Duration = Duration::from_secs(15 * 60);
pub const MAX_DELAY: Duration = Duration::from_secs(60);
/// Keys kept at most per map. With keys of bounded size, this bounds memory.
const MAX_KEYS: usize = 10_000;

#[derive(Debug, Default)]
struct Entry {
    /// The start times of the attempts in the window that did not succeed.
    times: VecDeque<Instant>,
    /// For an IP: the earliest start for the next attempt.
    next: Option<Instant>,
}

#[derive(Debug)]
struct Counts<K> {
    map: HashMap<K, Entry>,
}

// A derive would require `K: Default`, which `IpAddr` is not.
impl<K> Default for Counts<K> {
    fn default() -> Self {
        Self {
            map: HashMap::new(),
        }
    }
}

impl<K: std::hash::Hash + Eq + Clone> Counts<K> {
    /// The entry for `key` with only the times in the window. `None` when the
    /// map is full and has no room for a new key.
    fn entry(&mut self, key: &K, now: Instant) -> Option<&mut Entry> {
        if self.map.len() >= MAX_KEYS && !self.map.contains_key(key) {
            // Drop the keys whose attempts all left the window.
            self.map.retain(|_, e| {
                e.times
                    .back()
                    .is_some_and(|t| now.duration_since(*t) < WINDOW)
                    || e.next.is_some_and(|t| t > now)
            });
            if self.map.len() >= MAX_KEYS {
                return None;
            }
        }
        let entry = self.map.entry(key.clone()).or_default();
        while entry
            .times
            .front()
            .is_some_and(|t| now.duration_since(*t) >= WINDOW)
        {
            entry.times.pop_front();
        }
        Some(entry)
    }
}

#[derive(Debug, Default)]
struct Counters {
    accounts: Counts<String>,
    ips: Counts<IpAddr>,
}

#[derive(Debug, Default)]
pub struct Throttle {
    counters: Mutex<Counters>,
}

/// A login attempt that got a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Attempt {
    /// How long to wait before checking the password.
    pub wait: Duration,
    /// The recent failures that caused the wait, for the log.
    pub failures: usize,
    /// The attempt's start, which [`Throttle::succeed`] takes back.
    pub at: Instant,
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

/// Accounts compare without regard to case. The key is cut to the longest
/// name that setup accepts, so a huge username cannot fill memory.
fn account_key(account: &str) -> String {
    account
        .chars()
        .take(NAME_MAX_LEN)
        .collect::<String>()
        .to_lowercase()
}

/// One IPv6 client usually controls a whole /64, so the /64 is the key.
fn ip_key(ip: IpAddr) -> IpAddr {
    match ip.to_canonical() {
        IpAddr::V6(v6) => IpAddr::V6(Ipv6Addr::from(u128::from(v6) & !u128::from(u64::MAX))),
        v4 => v4,
    }
}

impl Throttle {
    /// Starts a login attempt and counts it as a failure. Returns the wait
    /// before the password check, or `Err` with the time after which the IP
    /// gets a turn again.
    pub fn begin(&self, account: &str, ip: IpAddr, now: Instant) -> Result<Attempt, Duration> {
        let (account, ip) = (account_key(account), ip_key(ip));
        let mut counters = self
            .counters
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Counters { accounts, ips } = &mut *counters;
        // A full map fails closed: a key that it cannot count gets the
        // longest wait, so junk keys cannot switch the throttle off.
        let account_failures = accounts.entry(&account, now).map(|e| e.times.len());
        let ip_entry = ips.entry(&ip, now);
        let ip_failures = ip_entry.as_ref().map(|e| e.times.len());
        let failures = match (account_failures, ip_failures) {
            (Some(a), Some(i)) => a.max(i),
            _ => usize::MAX,
        };
        let mut start = now + delay_for(failures);
        if let Some(entry) = ip_entry {
            if let Some(next) = entry.next {
                start = start.max(next);
            }
            if start - now > MAX_DELAY {
                return Err(start - now);
            }
            entry.times.push_back(now);
            entry.next = Some(start + delay_for(entry.times.len()));
        }
        if let Some(entry) = accounts.map.get_mut(&account) {
            entry.times.push_back(now);
        }
        Ok(Attempt {
            wait: start - now,
            failures: failures.min(MAX_KEYS),
            at: now,
        })
    }

    /// A correct login clears the account's failures and takes back the
    /// attempt from the IP's count. The IP keeps its other failures, so one
    /// good account does not reset guesses at other accounts.
    pub fn succeed(&self, account: &str, ip: IpAddr, attempt: &Attempt) {
        let mut counters = self
            .counters
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        counters.accounts.map.remove(&account_key(account));
        if let Some(entry) = counters.ips.map.get_mut(&ip_key(ip))
            && let Some(i) = entry.times.iter().position(|t| *t == attempt.at)
        {
            entry.times.remove(i);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::IpAddr;
    use std::time::{Duration, Instant};

    use super::{MAX_DELAY, MAX_KEYS, Throttle, WINDOW, delay_for};

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    fn wait(t: &Throttle, account: &str, from: &str, now: Instant) -> Duration {
        t.begin(account, ip(from), now).unwrap().wait
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
            // Different IPs: the account count alone must cause the wait.
            assert_eq!(
                wait(&t, "Admin", &format!("10.0.0.{n}"), now),
                Duration::ZERO
            );
        }
        let sixth = t.begin("ADMIN", ip("10.0.0.99"), now).unwrap();
        assert_eq!((sixth.wait, sixth.failures), (Duration::from_secs(1), 5));
    }

    #[test]
    fn one_ip_guessing_many_accounts_waits_too() {
        let t = Throttle::default();
        let now = Instant::now();
        for n in 0..5 {
            wait(&t, &format!("user{n}"), "10.0.0.1", now);
        }
        assert_eq!(
            wait(&t, "someone-else", "10.0.0.1", now),
            Duration::from_secs(1)
        );
        assert_eq!(wait(&t, "someone-else", "10.0.0.2", now), Duration::ZERO);
    }

    #[test]
    fn parallel_attempts_from_one_ip_queue_and_the_excess_gets_no_turn() {
        let t = Throttle::default();
        let now = Instant::now();
        // All at the same instant, as parallel requests arrive, and before
        // any of them has a result.
        let results: Vec<_> = (0..12)
            .map(|n| t.begin(&format!("user{n}"), ip("10.0.0.1"), now))
            .collect();
        let waits: Vec<u64> = results
            .iter()
            .map_while(|r| r.ok().map(|a| a.wait.as_secs()))
            .collect();
        // Each waits for the one before it: at most one guess per turn.
        assert_eq!(waits, [0, 0, 0, 0, 0, 1, 3, 7, 15, 31]);
        assert_eq!(results[10], Err(Duration::from_secs(63)));
        assert!(results[11].is_err());
    }

    #[test]
    fn failures_leave_the_window_after_15_minutes() {
        let t = Throttle::default();
        let then = Instant::now();
        for _ in 0..5 {
            wait(&t, "admin", "10.0.0.1", then);
        }
        assert_eq!(wait(&t, "admin", "10.0.0.1", then + WINDOW), Duration::ZERO);
    }

    #[test]
    fn a_correct_login_clears_the_account_and_its_own_attempt_only() {
        let t = Throttle::default();
        let now = Instant::now();
        for _ in 0..4 {
            wait(&t, "admin", "10.0.0.1", now);
        }
        let good = t.begin("admin", ip("10.0.0.1"), now).unwrap();
        t.succeed("ADMIN", ip("10.0.0.1"), &good);
        // The account starts over. The IP keeps its 4 failures.
        assert_eq!(wait(&t, "admin", "10.0.0.2", now), Duration::ZERO);
        assert_eq!(t.begin("other", ip("10.0.0.1"), now).unwrap().failures, 4);
    }

    #[test]
    fn an_ipv6_client_is_counted_by_its_64() {
        let t = Throttle::default();
        let now = Instant::now();
        for n in 0..5 {
            wait(&t, &format!("user{n}"), &format!("2001:db8:1:2::{n}"), now);
        }
        assert_eq!(
            wait(&t, "x", "2001:db8:1:2:ffff::1", now),
            Duration::from_secs(1)
        );
        assert_eq!(wait(&t, "x", "2001:db8:1:3::1", now), Duration::ZERO);
        // An IPv4 client in IPv6 form is the same client.
        for _ in 0..5 {
            wait(&t, "y", "10.0.0.7", now);
        }
        assert_eq!(
            wait(&t, "z", "::ffff:10.0.0.7", now),
            Duration::from_secs(1)
        );
    }

    #[test]
    fn a_full_map_gives_new_keys_the_longest_wait() {
        let t = Throttle::default();
        let now = Instant::now();
        {
            let mut c = t.counters.lock().unwrap();
            for n in 0..MAX_KEYS {
                let e = c.accounts.map.entry(format!("junk{n}")).or_default();
                e.times.push_back(now);
            }
        }
        let attempt = t.begin("admin", ip("10.0.0.1"), now).unwrap();
        assert_eq!(attempt.wait, MAX_DELAY);
        // Keys whose attempts left the window make room again.
        assert_eq!(wait(&t, "admin", "10.0.0.2", now + WINDOW), Duration::ZERO);
    }

    #[test]
    fn a_huge_username_keeps_a_small_key() {
        let t = Throttle::default();
        let huge = "x".repeat(1 << 20);
        wait(&t, &huge, "10.0.0.1", Instant::now());
        let c = t.counters.lock().unwrap();
        assert!(c.accounts.map.keys().all(|k| k.len() <= 64));
    }
}
