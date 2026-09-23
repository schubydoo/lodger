//! Single-use WebSocket tickets (TAD section 7.4, ASVS 4.4.2).
//!
//! A page asks `POST /api/ws-tickets` for a ticket, with its session and its
//! CSRF token, and puts the ticket in the WebSocket URL. The upgrade redeems
//! the ticket: it works once, for 30 seconds, and only with the session that
//! asked for it. A ticket leaked from a log or a history therefore opens
//! nothing. Only the ticket's SHA-256 stays in memory.
//!
//! The ticket also keeps the `Origin` of the request that asked for it. That
//! request passed the Origin rule for state-changing requests, so it came
//! from a Lodger page. The upgrade must send exactly the same `Origin`
//! (TAD 7.4). Browsers send no `Sec-Fetch-Site` on a WebSocket upgrade, so
//! this exact match is the upgrade's Origin check, and it never uses `Host`.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

/// How long a ticket works.
pub const LIFETIME: Duration = Duration::from_secs(30);
/// Tickets kept at most. Each expires in 30 s, so only a flood fills this.
const MAX_TICKETS: usize = 10_000;

#[derive(Debug)]
struct Ticket {
    /// The SHA-256 of the session token that asked for the ticket.
    session: [u8; 32],
    /// The `Origin` of the request that asked for it. `None` for a script,
    /// which sends none.
    origin: Option<String>,
    expires: Instant,
}

#[derive(Debug, Default)]
pub struct Tickets {
    by_hash: Mutex<HashMap<[u8; 32], Ticket>>,
}

fn hash(ticket: &str) -> [u8; 32] {
    Sha256::digest(ticket.as_bytes()).into()
}

impl Tickets {
    /// Makes a ticket for `session`. `None` when too many tickets are live.
    pub fn issue(
        &self,
        session: [u8; 32],
        origin: Option<String>,
        now: Instant,
    ) -> Result<Option<String>, String> {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).map_err(|e| format!("cannot make a ticket: {e}"))?;
        let ticket = hex::encode(bytes);
        let mut by_hash = self
            .by_hash
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if by_hash.len() >= MAX_TICKETS {
            by_hash.retain(|_, t| now < t.expires);
            if by_hash.len() >= MAX_TICKETS {
                return Ok(None);
            }
        }
        by_hash.insert(
            hash(&ticket),
            Ticket {
                session,
                origin,
                expires: now + LIFETIME,
            },
        );
        Ok(Some(ticket))
    }

    /// Uses up `ticket`. True only if it is live, belongs to `session`, and
    /// `origin` is exactly the one that asked for it. A ticket is gone after
    /// one attempt, even a failed one.
    pub fn redeem(
        &self,
        ticket: &str,
        session: [u8; 32],
        origin: Option<&str>,
        now: Instant,
    ) -> bool {
        self.by_hash
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&hash(ticket))
            .is_some_and(|t| {
                t.session == session && t.origin.as_deref() == origin && now < t.expires
            })
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::{LIFETIME, MAX_TICKETS, Tickets};

    const ALICE: [u8; 32] = [1; 32];
    const BOB: [u8; 32] = [2; 32];
    const PAGE: &str = "https://lodger.lan";

    fn issue(t: &Tickets, session: [u8; 32], now: Instant) -> String {
        t.issue(session, Some(PAGE.into()), now).unwrap().unwrap()
    }

    #[test]
    fn a_ticket_works_once() {
        let (t, now) = (Tickets::default(), Instant::now());
        let ticket = issue(&t, ALICE, now);
        assert_eq!(ticket.len(), 64);
        assert!(t.redeem(&ticket, ALICE, Some(PAGE), now));
        assert!(!t.redeem(&ticket, ALICE, Some(PAGE), now));
    }

    #[test]
    fn a_ticket_expires_after_30_seconds() {
        let (t, now) = (Tickets::default(), Instant::now());
        let ticket = issue(&t, ALICE, now);
        assert!(!t.redeem(&ticket, ALICE, Some(PAGE), now + LIFETIME));
        let ticket = issue(&t, ALICE, now);
        assert!(t.redeem(
            &ticket,
            ALICE,
            Some(PAGE),
            now + LIFETIME - std::time::Duration::from_millis(1)
        ));
    }

    #[test]
    fn a_ticket_works_only_for_its_session_and_a_wrong_try_uses_it_up() {
        let (t, now) = (Tickets::default(), Instant::now());
        let ticket = issue(&t, ALICE, now);
        assert!(!t.redeem(&ticket, BOB, Some(PAGE), now));
        assert!(!t.redeem(&ticket, ALICE, Some(PAGE), now));
        assert!(!t.redeem("not a ticket", ALICE, Some(PAGE), now));
    }

    #[test]
    fn a_ticket_needs_exactly_the_origin_that_asked_for_it() {
        let (t, now) = (Tickets::default(), Instant::now());
        for other in [
            Some("https://evil.lodger.lan"),
            Some("http://lodger.lan"),
            Some("https://lodger.lan:8443"),
            None,
        ] {
            let ticket = issue(&t, ALICE, now);
            assert!(!t.redeem(&ticket, ALICE, other, now), "{other:?}");
        }
        // A script that sends no Origin gets a ticket that needs none.
        let ticket = t.issue(ALICE, None, now).unwrap().unwrap();
        assert!(!t.redeem(&ticket, ALICE, Some(PAGE), now));
        let ticket = t.issue(ALICE, None, now).unwrap().unwrap();
        assert!(t.redeem(&ticket, ALICE, None, now));
    }

    #[test]
    fn a_flood_of_tickets_is_refused_until_they_expire() {
        let (t, now) = (Tickets::default(), Instant::now());
        for _ in 0..MAX_TICKETS {
            issue(&t, ALICE, now);
        }
        assert_eq!(t.issue(ALICE, None, now).unwrap(), None);
        assert!(t.issue(ALICE, None, now + LIFETIME).unwrap().is_some());
    }
}
