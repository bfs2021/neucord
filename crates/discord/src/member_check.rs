//! Checking that the members the `@` suggestions list are still members, as
//! the client does (579974, fed by the suggestions in 152210 through
//! 462446). A member we hold may have left without our being told: the
//! suggestions ask for them again by id, with a `nonce` on op 8, and drop
//! whom the answer's `not_found` names.
//!
//! Each listed member that isn't us or a guest is asked for at most once
//! every [`AGAIN_AFTER`]; one checked more recently is checked again once
//! that runs out. What is to be asked goes [`WAIT`] after the first of it,
//! [`crate::members::REQUEST_LIMIT`] users to a request, and not while the
//! connection is down. A request unanswered after [`ANSWER_TIMEOUT`] is
//! asked again, at most [`TRIES`] times for a user, behind a backoff for its
//! server. A new session asks again for everything still waiting.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use crate::Snowflake;
use crate::members::{Known, Members, REQUEST_LIMIT};

/// How long a member counts as checked (`3e5`).
pub const AGAIN_AFTER: Duration = Duration::from_secs(300);
/// How long what is to be asked waits for more (`M`, 500 ms).
pub const WAIT: Duration = Duration::from_millis(500);
/// How long the suggestions wait for the list to settle (462446's debounce).
pub const SUGGESTIONS_WAIT: Duration = Duration::from_millis(500);
/// How long a request waits for its answer (`3e4`).
pub const ANSWER_TIMEOUT: Duration = Duration::from_secs(30);
/// How many times a user is asked for before giving up (`t < 3`).
pub const TRIES: u32 = 3;
/// A server's backoff after a request went unanswered (158390, `1e3, 6e4`).
const BACKOFF_MIN: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(60);

/// Op 8 for `users` of `guild`, carrying `nonce`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub guild: Snowflake,
    pub users: Vec<Snowflake>,
    pub nonce: String,
}

/// A request waiting for its answer: the members as we held them.
#[derive(Debug)]
struct Asked {
    guild: Snowflake,
    members: Vec<(Snowflake, Known)>,
    deadline: Instant,
}

/// 158390's backoff: each failure waits longer, by up to twice the last
/// wait, at most [`BACKOFF_MAX`].
#[derive(Debug)]
struct Backoff {
    current: Duration,
    /// While set, the server's requests wait.
    until: Option<Instant>,
}

impl Backoff {
    fn fail(&mut self, now: Instant, random: f64) {
        let grown = self.current + self.current.mul_f64(2.0 * random);
        self.current = grown.min(BACKOFF_MAX);
        self.until = Some(now + self.current);
    }
}

#[derive(Debug, Default)]
pub struct MemberCheck {
    /// When each member was last found a member (`f`).
    checked: HashMap<Snowflake, HashMap<Snowflake, Instant>>,
    /// To ask for, by server, in order (`p`).
    queued: Vec<(Snowflake, Vec<Snowflake>)>,
    /// Checked too recently, asked for again at the time given (`y`).
    later: HashMap<Snowflake, HashMap<Snowflake, Instant>>,
    /// The request each member is in (`x`).
    asking: HashMap<Snowflake, HashMap<Snowflake, String>>,
    /// Requests waiting for their answer, by nonce (`g`).
    asked: HashMap<String, Asked>,
    /// How often each member went unanswered (`E`).
    tries: HashMap<Snowflake, HashMap<Snowflake, u32>>,
    backoffs: HashMap<Snowflake, Backoff>,
    /// When what is queued goes (`M`).
    send_at: Option<Instant>,
    /// Numbers the nonces (`b`).
    count: u64,
}

/// A member the suggestions may check: one we hold, not a guest (`I`).
fn checkable(members: &Members, guild: Snowflake, user: Snowflake) -> Option<&Known> {
    members.known(guild, user).filter(|k| !k.guest)
}

/// `n.toString(36)`.
fn base36(mut n: u64) -> String {
    let mut digits = Vec::new();
    loop {
        digits.push(char::from_digit((n % 36) as u32, 36).unwrap_or('0'));
        n /= 36;
        if n == 0 {
            break;
        }
    }
    digits.iter().rev().collect()
}

impl MemberCheck {
    /// The suggestions list `users` in `guild` (`m`).
    pub fn suggested(&mut self, guild: Snowflake, users: &[Snowflake], me: Snowflake, members: &Members, now: Instant) {
        let mut queued = false;
        for &user in users {
            if user == me || self.is_asking(guild, user) || checkable(members, guild, user).is_none() {
                continue;
            }
            if let Some(&at) = self.checked.get(&guild).and_then(|c| c.get(&user))
                && now.duration_since(at) < AGAIN_AFTER
            {
                self.later.entry(guild).or_default().insert(user, at + AGAIN_AFTER);
                continue;
            }
            self.queue(guild, user);
            queued = true;
        }
        if queued {
            self.send_soon(now);
        }
    }

    /// A new session (`CONNECTION_OPEN`): what was waiting for an answer or
    /// for its time is asked for again, and what was checked is forgotten.
    pub fn connected(&mut self, now: Instant) {
        let waiting: Vec<_> = self.asked.values().flat_map(|a| a.members.iter().map(|(u, _)| (a.guild, *u))).collect();
        for (guild, user) in waiting {
            self.queue(guild, user);
        }
        self.asked.clear();
        self.asking.clear();
        self.backoffs.clear();
        self.tries.clear();
        self.checked.clear();
        let later: Vec<_> =
            self.later.drain().flat_map(|(guild, users)| users.into_keys().map(move |u| (guild, u))).collect();
        for (guild, user) in later {
            self.queue(guild, user);
        }
        if !self.queued.is_empty() {
            self.send_soon(now);
        }
    }

    /// A resumed session: what is queued goes.
    pub fn resumed(&mut self, now: Instant) {
        if !self.queued.is_empty() {
            self.send_soon(now);
        }
    }

    /// When [`MemberCheck::poll`] next has something to do.
    pub fn next(&self) -> Option<Instant> {
        let later = self.later.values().flat_map(|users| users.values().copied());
        let deadlines = self.asked.values().map(|a| a.deadline);
        let backoffs = self.backoffs.values().filter_map(|b| b.until);
        self.send_at.into_iter().chain(later).chain(deadlines).chain(backoffs).min()
    }

    /// What is due at `now`: members whose wait ran out, requests left
    /// unanswered, backoffs over, and the requests to send if `connected`.
    /// `epoch_ms` names them; `random` is in `0..1`, for the backoff.
    pub fn poll(
        &mut self,
        now: Instant,
        epoch_ms: u64,
        members: &Members,
        connected: bool,
        mut random: impl FnMut() -> f64,
    ) -> Vec<Request> {
        self.later_due(now);
        let expired: Vec<String> =
            self.asked.iter().filter(|(_, a)| a.deadline <= now).map(|(nonce, _)| nonce.clone()).collect();
        for nonce in expired {
            self.unanswered(&nonce, now, random());
        }
        let over: Vec<Snowflake> =
            self.backoffs.iter().filter(|(_, b)| b.until.is_some_and(|t| t <= now)).map(|(g, _)| *g).collect();
        for guild in over {
            if let Some(backoff) = self.backoffs.get_mut(&guild) {
                backoff.until = None;
            }
            self.send_soon(now);
        }
        match self.send_at {
            Some(at) if at <= now => {
                self.send_at = None;
                self.send(now, epoch_ms, members, connected)
            }
            _ => Vec::new(),
        }
    }

    /// Discord answered: a chunk with `nonce`. Whom it says is no member, and
    /// whose member we hold as we did when asking, is to be forgotten.
    pub fn answered(
        &mut self,
        guild: Snowflake,
        nonce: &str,
        found: &[Snowflake],
        not_found: &[Snowflake],
        members: &Members,
        now: Instant,
    ) -> Vec<Snowflake> {
        if self.asked.get(nonce).is_none_or(|a| a.guild != guild) {
            return Vec::new();
        }
        let Some(asked) = self.asked.remove(nonce) else { return Vec::new() };
        self.backoffs.remove(&guild);
        if self.has_queued(guild) {
            self.send_soon(now);
        }
        let found: HashSet<_> = found.iter().collect();
        let not_found: HashSet<_> = not_found.iter().collect();
        let mut gone = Vec::new();
        for (user, held) in asked.members {
            self.stop_asking(guild, user, nonce);
            self.forget_tries(guild, user);
            if found.contains(&user) {
                self.checked.entry(guild).or_default().insert(user, now);
            } else if not_found.contains(&user)
                && members.known(guild, user).is_some_and(|k| k.joined_at == held.joined_at)
            {
                gone.push(user);
            }
        }
        gone
    }

    fn is_asking(&self, guild: Snowflake, user: Snowflake) -> bool {
        self.asking.get(&guild).is_some_and(|a| a.contains_key(&user))
    }

    fn has_queued(&self, guild: Snowflake) -> bool {
        self.queued.iter().any(|(g, _)| *g == guild)
    }

    fn queue(&mut self, guild: Snowflake, user: Snowflake) {
        let at = match self.queued.iter().position(|(g, _)| *g == guild) {
            Some(at) => at,
            None => {
                self.queued.push((guild, Vec::new()));
                self.queued.len() - 1
            }
        };
        let users = &mut self.queued[at].1;
        if !users.contains(&user) {
            users.push(user);
        }
    }

    /// `M.delay(false)`: a wait already under way isn't pushed back.
    fn send_soon(&mut self, now: Instant) {
        self.send_at.get_or_insert(now + WAIT);
    }

    fn stop_asking(&mut self, guild: Snowflake, user: Snowflake, nonce: &str) {
        if let Some(asking) = self.asking.get_mut(&guild) {
            if asking.get(&user).is_some_and(|n| n == nonce) {
                asking.remove(&user);
            }
            if asking.is_empty() {
                self.asking.remove(&guild);
            }
        }
    }

    fn forget_tries(&mut self, guild: Snowflake, user: Snowflake) {
        if let Some(tries) = self.tries.get_mut(&guild) {
            tries.remove(&user);
            if tries.is_empty() {
                self.tries.remove(&guild);
            }
        }
    }

    /// `O`: members whose wait is over are queued, unless being asked.
    fn later_due(&mut self, now: Instant) {
        let mut due = Vec::new();
        for (&guild, users) in &mut self.later {
            users.retain(|&user, &mut at| {
                let over = at <= now;
                if over {
                    due.push((guild, user));
                }
                !over
            });
        }
        self.later.retain(|_, users| !users.is_empty());
        let mut queued = false;
        for (guild, user) in due {
            if !self.is_asking(guild, user) {
                self.queue(guild, user);
                queued = true;
            }
        }
        if queued {
            self.send_soon(now);
        }
    }

    /// A request's 30 seconds ran out without an answer.
    fn unanswered(&mut self, nonce: &str, now: Instant, random: f64) {
        let Some(asked) = self.asked.remove(nonce) else { return };
        let guild = asked.guild;
        let mut again = false;
        for (user, _) in asked.members {
            self.stop_asking(guild, user, nonce);
            let tries = self.tries.entry(guild).or_default();
            let count = tries.get(&user).copied().unwrap_or(0) + 1;
            if count < TRIES {
                tries.insert(user, count);
                self.queue(guild, user);
                again = true;
            } else {
                tries.remove(&user);
            }
        }
        if self.tries.get(&guild).is_some_and(HashMap::is_empty) {
            self.tries.remove(&guild);
        }
        if again {
            let backoff = self.backoffs.entry(guild).or_insert(Backoff { current: BACKOFF_MIN, until: None });
            backoff.fail(now, random);
        } else {
            self.backoffs.remove(&guild);
            if self.has_queued(guild) {
                self.send_soon(now);
            }
        }
    }

    /// `M`'s run: forget checks older than [`AGAIN_AFTER`], then, connected,
    /// ask for each server's queue unless its backoff holds it.
    fn send(&mut self, now: Instant, epoch_ms: u64, members: &Members, connected: bool) -> Vec<Request> {
        for checked in self.checked.values_mut() {
            checked.retain(|_, at| now.duration_since(*at) < AGAIN_AFTER);
        }
        self.checked.retain(|_, c| !c.is_empty());
        if !connected {
            return Vec::new();
        }
        let mut requests = Vec::new();
        let queued = std::mem::take(&mut self.queued);
        for (guild, users) in queued {
            if self.backoffs.get(&guild).is_some_and(|b| b.until.is_some()) {
                self.queued.push((guild, users));
                continue;
            }
            for chunk in users.chunks(REQUEST_LIMIT) {
                let mut held = Vec::new();
                for &user in chunk {
                    match checkable(members, guild, user) {
                        Some(known) => held.push((user, known.clone())),
                        None => self.forget_tries(guild, user),
                    }
                }
                if held.is_empty() {
                    continue;
                }
                self.count += 1;
                let nonce = format!("gmv:{}:{}", base36(epoch_ms), base36(self.count));
                for (user, _) in &held {
                    self.asking.entry(guild).or_default().insert(*user, nonce.clone());
                }
                let users = held.iter().map(|(user, _)| *user).collect();
                self.asked.insert(nonce.clone(), Asked { guild, members: held, deadline: now + ANSWER_TIMEOUT });
                requests.push(Request { guild, users, nonce });
            }
        }
        requests
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const G: Snowflake = Snowflake(1);
    const ME: Snowflake = Snowflake(99);

    fn members(users: &[(u64, Option<i64>, bool)]) -> Members {
        let mut members = Members::default();
        let known = users
            .iter()
            .map(|&(id, joined_at, guest)| (Snowflake(id), Known { joined_at, guest, ..Known::default() }))
            .collect();
        members.add(G, known, &[]);
        members
    }

    fn ids(ids: &[u64]) -> Vec<Snowflake> {
        ids.iter().copied().map(Snowflake).collect()
    }

    #[test]
    fn listed_members_go_together_half_a_second_after_the_first() {
        let held = members(&[(2, Some(10), false), (3, Some(10), false), (4, Some(10), true)]);
        let mut check = MemberCheck::default();
        let t0 = Instant::now();
        check.suggested(G, &ids(&[2, 99, 4, 5]), ME, &held, t0);
        check.suggested(G, &ids(&[3]), ME, &held, t0 + Duration::from_millis(300));
        assert_eq!(check.next(), Some(t0 + WAIT), "the second list doesn't push the wait back");
        assert!(check.poll(t0 + Duration::from_millis(499), 0, &held, true, || 0.5).is_empty());
        let sent = check.poll(t0 + WAIT, 36 * 36, &held, true, || 0.5);
        assert_eq!(
            sent,
            [Request { guild: G, users: ids(&[2, 3]), nonce: "gmv:100:1".into() }],
            "not us, a guest or someone whose member we don't hold"
        );
    }

    #[test]
    fn nothing_goes_while_disconnected_and_a_new_session_sends_it() {
        let held = members(&[(2, Some(10), false)]);
        let mut check = MemberCheck::default();
        let t0 = Instant::now();
        check.suggested(G, &ids(&[2]), ME, &held, t0);
        assert!(check.poll(t0 + WAIT, 0, &held, false, || 0.5).is_empty());
        assert_eq!(check.next(), None, "it waits for the connection");
        check.connected(t0 + Duration::from_secs(5));
        let sent = check.poll(t0 + Duration::from_secs(5) + WAIT, 0, &held, true, || 0.5);
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].users, ids(&[2]));
    }

    #[test]
    fn a_hundred_to_a_request() {
        let users: Vec<_> = (1000..1150).map(|id| (id, Some(10), false)).collect();
        let held = members(&users);
        let mut check = MemberCheck::default();
        let t0 = Instant::now();
        let listed: Vec<_> = (1000..1150).map(Snowflake).collect();
        check.suggested(G, &listed, ME, &held, t0);
        let sent = check.poll(t0 + WAIT, 0, &held, true, || 0.5);
        assert_eq!(sent.iter().map(|r| r.users.len()).collect::<Vec<_>>(), [100, 50]);
        assert_eq!(sent[0].nonce, "gmv:0:1");
        assert_eq!(sent[1].nonce, "gmv:0:2");
    }

    #[test]
    fn whom_the_answer_doesnt_find_is_forgotten_if_held_as_asked() {
        let mut held = members(&[(2, Some(10), false), (3, Some(10), false), (4, Some(10), false)]);
        let mut check = MemberCheck::default();
        let t0 = Instant::now();
        check.suggested(G, &ids(&[2, 3, 4]), ME, &held, t0);
        let sent = check.poll(t0 + WAIT, 0, &held, true, || 0.5);
        // 4 rejoined meanwhile: the member we hold now isn't the one asked about.
        held.add(G, vec![(Snowflake(4), Known { joined_at: Some(20), ..Known::default() })], &[]);
        let gone = check.answered(G, &sent[0].nonce, &ids(&[2]), &ids(&[3, 4]), &held, t0 + Duration::from_secs(1));
        assert_eq!(gone, ids(&[3]));
        assert!(check.answered(G, &sent[0].nonce, &[], &ids(&[2]), &held, t0).is_empty(), "answered once");
        assert!(check.answered(G, "gmv:other", &[], &ids(&[2]), &held, t0).is_empty(), "not ours");
    }

    #[test]
    fn a_member_found_is_checked_again_five_minutes_later() {
        let held = members(&[(2, Some(10), false)]);
        let mut check = MemberCheck::default();
        let t0 = Instant::now();
        check.suggested(G, &ids(&[2]), ME, &held, t0);
        let sent = check.poll(t0 + WAIT, 0, &held, true, || 0.5);
        let found_at = t0 + Duration::from_secs(1);
        check.answered(G, &sent[0].nonce, &ids(&[2]), &[], &held, found_at);
        let listed_again = t0 + Duration::from_secs(60);
        check.suggested(G, &ids(&[2]), ME, &held, listed_again);
        assert!(check.poll(listed_again + WAIT, 0, &held, true, || 0.5).is_empty(), "checked a minute ago");
        assert_eq!(check.next(), Some(found_at + AGAIN_AFTER));
        assert!(check.poll(found_at + AGAIN_AFTER, 0, &held, true, || 0.5).is_empty());
        let sent = check.poll(found_at + AGAIN_AFTER + WAIT, 0, &held, true, || 0.5);
        assert_eq!(sent.len(), 1, "its five minutes ran out while listed");
    }

    #[test]
    fn unanswered_requests_go_again_behind_a_backoff_three_times_at_most() {
        let held = members(&[(2, Some(10), false)]);
        let mut check = MemberCheck::default();
        let mut now = Instant::now();
        check.suggested(G, &ids(&[2]), ME, &held, now);
        now += WAIT;
        assert_eq!(check.poll(now, 0, &held, true, || 0.0).len(), 1);
        // No jitter: the backoff holds at a second each time.
        now += ANSWER_TIMEOUT;
        assert!(check.poll(now, 0, &held, true, || 0.0).is_empty());
        assert_eq!(check.next(), Some(now + BACKOFF_MIN));
        now += BACKOFF_MIN;
        assert!(check.poll(now, 0, &held, true, || 0.0).is_empty());
        now += WAIT;
        assert_eq!(check.poll(now, 0, &held, true, || 0.0).len(), 1, "the second try");
        now += ANSWER_TIMEOUT;
        check.poll(now, 0, &held, true, || 0.0);
        now += BACKOFF_MIN;
        check.poll(now, 0, &held, true, || 0.0);
        now += WAIT;
        assert_eq!(check.poll(now, 0, &held, true, || 0.0).len(), 1, "the third");
        now += ANSWER_TIMEOUT;
        assert!(check.poll(now, 0, &held, true, || 0.0).is_empty());
        assert_eq!(check.next(), None, "given up");
    }

    #[test]
    fn the_backoff_grows_by_up_to_twice_its_wait_and_stops_at_a_minute() {
        let mut backoff = Backoff { current: BACKOFF_MIN, until: None };
        let now = Instant::now();
        backoff.fail(now, 0.5);
        assert_eq!(backoff.current, Duration::from_secs(2));
        backoff.fail(now, 1.0);
        assert_eq!(backoff.current, Duration::from_secs(6));
        for _ in 0..5 {
            backoff.fail(now, 1.0);
        }
        assert_eq!(backoff.until, Some(now + BACKOFF_MAX));
    }

    #[test]
    fn nonces_count_in_base_36() {
        assert_eq!(base36(0), "0");
        assert_eq!(base36(35), "z");
        assert_eq!(base36(1_763_400_000_000), "mi3etw5c", "as `Date.now().toString(36)` gives");
    }
}
