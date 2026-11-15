// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Signing in is refused for a while once a name has been failed at too
//! often.
//!
//! Two counts are kept. Ten failed password sign-ins for one name from one
//! client address inside fifteen minutes stop that name being tried *from
//! that address* for the rest of the window; another address can still sign
//! in to that name, so nobody who merely knows a name can hold its owner out
//! of their own account. Two hundred failures for one name from any address
//! inside fifteen minutes stop that name being tried from everywhere for the
//! rest of the window, which is what slows a caller spreading attempts over
//! many addresses.
//!
//! A password is not worth checking while either count says otherwise, so a
//! refused attempt is refused before the store is read and says nothing about
//! whether the name exists or what was wrong. A sign-in that works clears the
//! count for its own address and that address alone; it does not undo what
//! other addresses have accumulated, and it does not lift the name's ceiling,
//! because one address signing in proves nothing about another.
//!
//! Each count is kept for one window and nothing longer: it is opened by the
//! first failure it holds and closes fifteen minutes later, taking any wait it
//! brought down with it. A caller is therefore held out for at most fifteen
//! minutes at a time and then starts from nothing, whether or not the wait
//! that window brought down had run to its end.
//!
//! Both counts live in this process's memory and are never replicated: every
//! server keeps its own and a restart starts clean. That is enough for what
//! this is against, a caller working through one reachable port. A caller
//! spread over every server gets the same ceilings per server, not per cloud.
//!
//! Both tables are bounded. A key takes its seat the first time an attempt
//! arrives for it and gives it back when its count is forgotten, and a seat
//! for a new key is bought from the key gone longest without a try — a
//! caller's own key is only ever the one bought once its whole table has been
//! walked without one. A caller inventing names cannot make a server grow past
//! `MAX_NAMES`, and one inventing addresses cannot make it grow past
//! `MAX_COUNTS`. The name ceiling is smaller than the product of the two
//! bounds by a wide margin, so there is always room to count a name across
//! the addresses that are held.

use chrono::{DateTime, TimeDelta as Duration, Utc};
use std::collections::{HashMap, VecDeque};
use std::hash::Hash;
use std::net::IpAddr;

/// Failed sign-ins for one name from one address that bring the wait down.
pub const MAX_FAILURES: u32 = 10;
/// Failed sign-ins for one name from any address that bring the wait down.
/// The ceiling that holds a name out everywhere, for a caller spreading its
/// attempts over addresses one address at a time never reaches.
pub const MAX_FAILURES_PER_NAME: u32 = 200;
/// How long failures are counted, and how long the wait then runs.
pub const WINDOW: Duration = Duration::minutes(15);
/// Most names held at once.
pub const MAX_NAMES: usize = 10_000;
/// Most (name, address) counts held at once.
pub const MAX_COUNTS: usize = 50_000;

/// What a sign-in against a name, from an address, may do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Attempt {
    /// The password may be checked.
    Try,
    /// The name is in a wait and is refused for this long with no password
    /// check. The wait belongs to this server alone: the counts are in its
    /// memory, so a caller that reached the other servers is not stopped.
    Locked(Duration),
}

/// The failures counted over a window opened by the first of them, and the
/// wait a full count brought down. The wait runs only to the window's end, so
/// one clock serves both.
#[derive(Clone, Copy, Debug)]
struct Count {
    failures: u32,
    /// The attempt that opened the window the failures are counted over.
    since: DateTime<Utc>,
    /// When the name may be tried again, once a count is full.
    until: Option<DateTime<Utc>>,
}

impl Count {
    /// A window opened by a failure at `now`.
    fn open(now: DateTime<Utc>) -> Self {
        Count {
            failures: 0,
            since: now,
            until: None,
        }
    }

    /// Whether this window has closed. Past it nothing is remembered: not the
    /// failures, and not the wait they brought down.
    fn closed(self, now: DateTime<Utc>) -> bool {
        self.since + WINDOW <= now
    }

    /// Whether this count is worth answering from. A window that has closed
    /// counted nothing worth holding, and neither has one whose wait has run
    /// out — the wait ends with the window it was counted in, so the second
    /// test asks the same question and the second answer is the one that is
    /// cheap.
    fn counts(self, now: DateTime<Utc>) -> bool {
        self.until.is_none_or(|until| now < until) && !self.closed(now)
    }
}

/// Failures held under one key of one of the two tables: a name, or a name
/// with the address it was tried from.
trait Key: Clone + Eq + Hash {}
impl<K: Clone + Eq + Hash> Key for K {}

/// One table: failures per key, in one server's memory, bounded by cutting
/// the key gone longest without a try. `bound` is how many keys it holds
/// before a new one has to buy a seat.
struct Table<K> {
    counts: HashMap<K, Count>,
    /// Keys with a seat, in the order they were last tried, oldest first. A
    /// key takes its seat the first time an attempt arrives for it and gives
    /// it back when its count is forgotten, so the order's length is the
    /// table's length, and room is made from its front.
    order: VecDeque<K>,
    bound: usize,
}

impl<K: Key> Table<K> {
    fn new(bound: usize) -> Self {
        Table {
            counts: HashMap::new(),
            order: VecDeque::new(),
            bound,
        }
    }

    fn len(&self) -> usize {
        self.order.len()
    }

    fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    /// The wait the key is in, if it is in one.
    fn wait(&self, key: &K, now: DateTime<Utc>) -> Option<Duration> {
        match self.counts.get(key) {
            Some(c) if c.counts(now) && c.until.is_some() => {
                Some((c.until.unwrap() - now).max(Duration::zero()))
            }
            _ => None,
        }
    }

    /// Count a failure against `key`, saying whether this one brought a wait
    /// down.
    fn failed(&mut self, key: K, ceiling: u32, now: DateTime<Utc>) -> bool {
        // The seat comes first, and is settled on the order rather than on
        // the count: a key the order knows holds a seat and never buys
        // another, whether or not what it was counted for still stands — a
        // key keeps its seat through a closed window and through a sign-in
        // that worked, until something forgets the key and gives the seat
        // back with whatever was counted in it. Only a key the order has
        // never seen buys.
        //
        // It buys while the attempt is still outside the order, which is the
        // one moment at which the key being worked over could lose its seat
        // to its own fresh count. A buyer takes the front of the order, and
        // the attempt moves to the back after the seat is settled, so an
        // attempt never buys the seat it is standing on and a key already
        // here is exempt from the walk by being behind it.
        if !self.order.iter().any(|k| *k == key) {
            self.make_room();
        }
        // What is held for a key is worth answering from until its window
        // runs out — a wait it brought down ends with that window, so nothing
        // is carried past it — and this attempt opens the window it belongs
        // to rather than joining one that has closed.
        let mut c = self
            .counts
            .get(&key)
            .filter(|c| c.counts(now))
            .copied()
            .unwrap_or_else(|| Count::open(now));
        self.touch(&key);
        c.failures = c.failures.saturating_add(1).min(ceiling);
        if c.failures >= ceiling {
            c.until = Some((c.since + WINDOW).min(now + WINDOW));
        }
        let filled = c.failures >= ceiling;
        self.counts.insert(key, c);
        debug_assert!(self.counts.len() <= self.order.len());
        filled
    }

    /// Forget a key: its count and its seat together. This is where memory
    /// comes back — a seat bought is held until the key is forgotten,
    /// whoever forgets it.
    fn forget(&mut self, key: &K) {
        self.counts.remove(key);
        if let Some(at) = self.order.iter().position(|k| k == key) {
            self.order.remove(at);
        }
    }

    /// Forget the count a sign-in that worked leaves behind, and its seat.
    fn succeed(&mut self, key: K) {
        self.forget(&key);
    }

    /// Open a seat from the front of the order — the key gone longest without
    /// a try, which stays the front until it is cut, since an attempt moves
    /// only its own key. Seats are therefore bought one after another down
    /// the order, the same key never twice in a row, and no seat costs a
    /// hunt. What the front finds is cut whether or not anything is still
    /// counted for it: a key the order holds with nothing to answer for —
    /// what a sign-in that worked or a closed window leaves until the next
    /// attempt hands it back — owes nothing, and one it holds with a count
    /// has had its whole window to be worth holding. The key being tried is
    /// exempt and always has been: it is not in the order when its seat is
    /// bought and sits at the back after. The bound is kept against a caller
    /// with more keys than the table holds, and that caller is the only one
    /// who gets here.
    fn make_room(&mut self) {
        if self.order.len() < self.bound {
            return;
        }
        if let Some(key) = self.order.pop_front() {
            self.counts.remove(&key);
        }
    }

    /// Move `key` to the back of the order of trial, so the key being tried
    /// is never the one at the front and a key worked over keeps its place at
    /// the back, where no buyer for a seat reaches it while the caller is
    /// still guessing at it.
    fn touch(&mut self, key: &K) {
        if let Some(at) = self.order.iter().position(|k| k == key) {
            self.order.remove(at);
        }
        self.order.push_back(key.clone());
    }
}

/// Failed sign-ins, for one server: per (name, address), and per name across
/// every address.
pub struct Lockouts {
    /// Where the ceiling of `MAX_FAILURES` is counted.
    per_address: Table<(String, IpAddr)>,
    /// Where the ceiling of `MAX_FAILURES_PER_NAME` is counted, for every
    /// address that is being refused.
    per_name: Table<String>,
}

impl Default for Lockouts {
    fn default() -> Self {
        Lockouts {
            per_address: Table::new(MAX_COUNTS),
            per_name: Table::new(MAX_NAMES),
        }
    }
}

impl Lockouts {
    pub fn new() -> Self {
        Self::default()
    }

    /// Seats taken in the name table: a name is remembered from the attempt
    /// that first took a count for it until that count is forgotten, so this
    /// is what the table costs rather than what it happens to answer for at
    /// the moment.
    pub fn len(&self) -> usize {
        self.per_name.len()
    }

    pub fn is_empty(&self) -> bool {
        self.per_name.is_empty()
    }

    /// Seats taken in the (name, address) table, which is the one that grows
    /// with the addresses a caller arrives from.
    pub fn counts(&self) -> usize {
        self.per_address.len()
    }

    /// Whether `name` may be tried from `addr` now. Ask before the password
    /// is checked. Whichever count is in a wait answers first; a caller is not
    /// told which one, since that would tell it where its own attempts have
    /// reached.
    pub fn attempt(&self, name: &str, addr: IpAddr, now: DateTime<Utc>) -> Attempt {
        let left = [
            self.per_name.wait(&name.to_owned(), now),
            self.per_address.wait(&(name.to_owned(), addr), now),
        ]
        .into_iter()
        .flatten()
        .next();
        match left {
            Some(left) => Attempt::Locked(left),
            None => Attempt::Try,
        }
    }

    /// A sign-in with a wrong password, or with a name that is not there.
    /// Says whether this attempt brought a wait down. Both counts take the
    /// failure: the name's total is what catches a caller that spreads.
    pub fn failed(&mut self, name: &str, addr: IpAddr, now: DateTime<Utc>) -> bool {
        let by_name = self
            .per_name
            .failed(name.to_owned(), MAX_FAILURES_PER_NAME, now);
        let by_address = self
            .per_address
            .failed((name.to_owned(), addr), MAX_FAILURES, now);
        by_name || by_address
    }

    /// A sign-in that worked. What was counted for `name` from this address
    /// is forgotten, seat and all; what was counted for the name across every
    /// address is not, because a caller spread over addresses is not
    /// disproved by one address getting in. Nothing counted for another
    /// address is touched either — one address signing in proves nothing
    /// about another.
    pub fn succeeded(&mut self, name: &str, addr: IpAddr) {
        self.per_address.succeed((name.to_owned(), addr));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv6Addr;

    const HOUR: Duration = Duration::hours(1);
    const LAST: i64 = MAX_NAMES as i64 - 1;
    const A: IpAddr = IpAddr::V4(std::net::Ipv4Addr::new(203, 0, 113, 1));
    const B: IpAddr = IpAddr::V4(std::net::Ipv4Addr::new(203, 0, 113, 2));

    /// A fixed clock, so a window's edge is a second and not a race.
    fn t(secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(1_800_000_000, 0).unwrap() + Duration::seconds(secs)
    }

    /// Fail `n` times from `addr`, one second apart from `now`. Says what the
    /// last said.
    fn fail(l: &mut Lockouts, name: &str, addr: IpAddr, n: u32, now: DateTime<Utc>) -> bool {
        (0..n)
            .map(|i| l.failed(name, addr, now + Duration::seconds(i as i64)))
            .last()
            .unwrap_or(false)
    }

    /// The wait a name is in from `addr`, if it is in one.
    fn wait(l: &Lockouts, name: &str, addr: IpAddr, now: DateTime<Utc>) -> Option<Duration> {
        match l.attempt(name, addr, now) {
            Attempt::Try => None,
            Attempt::Locked(left) => Some(left),
        }
    }

    /// Fill the per-name table, every name counted in a window opened `gap`
    /// after the last, so the counts close in the order the names were tried.
    /// Says when the last of them was tried.
    fn fill(l: &mut Lockouts, now: DateTime<Utc>, gap: Duration) -> DateTime<Utc> {
        let mut last = now;
        for i in 0..MAX_NAMES as i64 {
            // Scaled, not summed: a minute apart over ten thousand names runs
            // past what a 32-bit count of seconds holds.
            let at = now + gap * i as i32;
            fail(l, &format!("n{i}"), A, 1, at);
            last = at;
        }
        assert_eq!(l.len(), MAX_NAMES);
        assert_eq!(l.per_name.order.len(), MAX_NAMES);
        last
    }

    /// Fill the per-address table for one name, one address per attempt. The
    /// name's own count is kept under its ceiling so the name stays answerable
    /// from and what is being tested is the address table's bound.
    fn fill_counts(l: &mut Lockouts, name: &str, now: DateTime<Utc>) {
        for i in 0..MAX_COUNTS as u32 {
            // Stay under the name's own ceiling, or the name is refused from
            // everywhere and the address table is no longer what is tested.
            let near = l
                .per_name
                .counts
                .get(name)
                .is_some_and(|c| c.failures >= MAX_FAILURES_PER_NAME - 2);
            l.failed(name, spread(i), now);
            if near {
                l.per_name.counts.get_mut(name).unwrap().failures = 1;
            }
        }
        assert!(l.counts() > MAX_COUNTS / 2, "{} counts held", l.counts());
    }

    /// Addresses to spread attempts over. `2001:db8::` is documentation
    /// space, and `A` and `B` are v4, so no address here is either of them.
    /// Every number this is given has a distinct low byte.
    fn spread(i: u32) -> IpAddr {
        IpAddr::from(Ipv6Addr::new(
            0x2001,
            0xdb8,
            0,
            0,
            0,
            0,
            (i >> 8) as u16,
            i as u16,
        ))
    }

    /// Addresses invented to crowd the count table out. Counted once each,
    /// none of them ever reaches its own ceiling, so a name worked over
    /// afterwards starts under its own. Every number has a distinct second
    /// and third low byte, and shares none with `spread`, `A` or `B`.
    fn crowd(i: u32) -> IpAddr {
        IpAddr::from(Ipv6Addr::new(
            0x2001,
            0xdb8,
            0,
            0,
            0,
            0xc000,
            (i >> 16) as u16 | 0x8000,
            i as u16 | 0x4000,
        ))
    }

    #[test]
    fn the_tenth_failure_from_an_address_brings_a_wait_down_for_the_window() {
        let mut l = Lockouts::new();
        let now = t(0);
        assert_eq!(l.attempt("alice", A, now), Attempt::Try);
        for i in 0..MAX_FAILURES - 1 {
            assert!(!l.failed("alice", A, now + Duration::seconds(i as i64)));
            assert_eq!(l.attempt("alice", A, now), Attempt::Try, "{i} failures");
        }
        assert!(l.failed("alice", A, now + Duration::seconds(9)));
        let left = wait(&l, "alice", A, now + Duration::seconds(10)).unwrap();
        assert!(left > WINDOW - HOUR && left <= WINDOW, "{left}");
        // Refused to the last second of the wait, open after it.
        assert!(wait(&l, "alice", A, now + WINDOW - Duration::seconds(1)).is_some());
        assert_eq!(l.attempt("alice", A, now + WINDOW), Attempt::Try);
        // Open again means a fresh ten, not the eleventh of the old run.
        assert!(!l.failed("alice", A, now + WINDOW));
        assert_eq!(l.per_address.counts[&("alice".to_owned(), A)].failures, 1);
    }

    #[test]
    fn an_address_in_a_wait_does_not_hold_the_name_out_elsewhere() {
        let mut l = Lockouts::new();
        let now = t(0);
        assert!(fail(&mut l, "alice", A, MAX_FAILURES, now));
        assert!(wait(&l, "alice", A, now).is_some());
        // The point of counting per address: a caller who knows the admin's
        // name cannot hold the admin out of the admin's own account.
        assert_eq!(l.attempt("alice", B, now), Attempt::Try);
        assert_eq!(l.attempt("carol", A, now), Attempt::Try);
    }

    #[test]
    fn failures_spread_over_addresses_still_fill_the_name_ceiling() {
        let mut l = Lockouts::new();
        let now = t(0);
        for i in 0..MAX_FAILURES_PER_NAME - 1 {
            let from = spread(i);
            let filled = l.failed("alice", from, now);
            // Each address is short of its own ten, so none of these failures
            // — the 199th of the name's window included — holds the name out.
            assert!(!filled, "failure {i}");
            assert_eq!(l.attempt("alice", from, now), Attempt::Try);
        }
        assert!(l.failed("alice", spread(250), now));
        // The name is refused from every address, including one that has
        // never failed at all, and including one still short of its own ten.
        assert!(fail(&mut l, "alice", A, 3, now));
        assert!(wait(&l, "alice", A, now).is_some());
        assert!(wait(&l, "alice", B, now).is_some());
        assert!(wait(&l, "alice", spread(251), now).is_some());
    }

    #[test]
    fn a_sign_in_that_works_clears_its_own_address_only() {
        let mut l = Lockouts::new();
        let now = t(0);
        // A's nine and B's nine, from before the sign-in that worked.
        fail(&mut l, "alice", A, MAX_FAILURES - 1, now);
        fail(&mut l, "alice", B, MAX_FAILURES - 1, now);
        assert_eq!(l.counts(), 2);
        l.succeeded("alice", A);
        // A's nine count for nothing now; B's nine are still nine.
        assert!(!l.per_address.counts.contains_key(&("alice".to_owned(), A)));
        assert!(l.per_address.counts.contains_key(&("alice".to_owned(), B)));
        // A starts from nothing: nine more is nine, not a tenth.
        assert!(!fail(&mut l, "alice", A, MAX_FAILURES - 1, now));
        assert_eq!(l.attempt("alice", A, now), Attempt::Try);
        // B never signed in anywhere and gets no such start: nine and one.
        assert!(l.failed("alice", B, now));
        assert!(wait(&l, "alice", B, now).is_some());
        // And one address getting in does not shorten the wait another
        // address is now in.
        l.succeeded("alice", spread(9));
        assert!(wait(&l, "alice", B, now).is_some());
    }

    #[test]
    fn a_wait_is_ended_by_time_and_by_nothing_else() {
        let mut l = Lockouts::new();
        let now = t(0);
        // The address's own wait, and the name's, brought down together.
        fail(&mut l, "alice", A, MAX_FAILURES_PER_NAME, now);
        assert!(wait(&l, "alice", A, now).is_some());
        // A sign-in that works elsewhere takes neither of them off: the name
        // was not proved safe by one address getting in.
        l.succeeded("alice", spread(9));
        assert!(wait(&l, "alice", A, now).is_some());
        // Nothing shortens a wait but the window it was counted in closing.
        assert_eq!(l.attempt("alice", A, now + WINDOW), Attempt::Try);
        assert_eq!(l.attempt("alice", B, now + WINDOW), Attempt::Try);
    }

    #[test]
    fn a_wait_never_runs_past_the_window_it_was_counted_in() {
        let mut l = Lockouts::new();
        let now = t(0);
        // Nine failures now, the tenth one second before that window closes.
        fail(&mut l, "alice", A, MAX_FAILURES - 1, now);
        assert!(l.failed("alice", A, now + WINDOW - Duration::seconds(1)));
        assert!(wait(&l, "alice", A, now + WINDOW - Duration::seconds(1)).is_some());
        // The window closing is the end of what was counted, so the name is
        // open again even though the wait that came out of it is not up. What
        // a caller can be held out for is never more than one window.
        assert_eq!(l.attempt("alice", A, now + WINDOW), Attempt::Try);
        // A fresh ten, not the eleventh of the run that filled.
        assert!(!l.failed("alice", A, now + WINDOW));
        assert_eq!(l.per_address.counts[&("alice".to_owned(), A)].failures, 1);
    }

    #[test]
    fn a_name_wait_ends_with_the_window_that_brought_it_down() {
        let mut l = Lockouts::new();
        let now = t(0);
        fail(&mut l, "alice", A, MAX_FAILURES_PER_NAME, now);
        assert!(wait(&l, "alice", B, now).is_some());
        assert_eq!(l.attempt("alice", B, now + WINDOW), Attempt::Try);
    }

    #[test]
    fn only_failures_inside_a_window_are_counted() {
        let mut l = Lockouts::new();
        let now = t(0);
        fail(&mut l, "alice", A, MAX_FAILURES - 1, now);
        // Those age out. The next run starts its own window and is short one.
        for i in 0..MAX_FAILURES - 1 {
            let at = now + WINDOW + Duration::seconds(i as i64);
            assert_eq!(l.attempt("alice", A, at), Attempt::Try);
            assert!(!l.failed("alice", A, at));
        }
        assert_eq!(
            l.attempt("alice", A, now + WINDOW + Duration::seconds(9)),
            Attempt::Try
        );
        assert!(l.failed("alice", A, now + WINDOW + Duration::seconds(9)));
        assert!(wait(&l, "alice", A, now + WINDOW + Duration::seconds(9)).is_some());
    }

    #[test]
    fn a_count_is_held_only_while_it_costs_something() {
        let mut l = Lockouts::new();
        let now = t(0);
        fail(&mut l, "alice", A, MAX_FAILURES, now);
        fail(&mut l, "bob", A, 3, now);
        assert_eq!(l.counts(), 2);
        // Both windows have closed, so neither count is answered from. They
        // are still held: holding a count costs nothing until the table is
        // full, and that is the only moment room is made.
        let late = now + WINDOW + Duration::seconds(1);
        assert_eq!(l.attempt("alice", A, late), Attempt::Try);
        assert_eq!(l.attempt("bob", A, late), Attempt::Try);
        l.failed("carol", A, late);
        assert_eq!(l.counts(), 3);
        // A sign-in that works gives its count's seat back at once.
        l.succeeded("bob", A);
        assert!(!l.per_address.counts.contains_key(&("bob".to_owned(), A)));
    }

    #[test]
    fn the_name_table_holds_no_more_than_its_bound() {
        let mut l = Lockouts::new();
        let now = t(0);
        for i in 0..MAX_NAMES as i64 * 2 {
            l.failed(&format!("n{i}"), A, now);
            assert!(l.len() <= MAX_NAMES, "{} names held", l.len());
            assert!(l.per_name.order.len() <= MAX_NAMES);
        }
        assert_eq!(l.len(), MAX_NAMES);
        // The names tried first made room for the rest and count nothing now.
        assert_eq!(l.attempt("n0", A, now), Attempt::Try);
        assert!(l
            .per_name
            .counts
            .contains_key(&format!("n{}", MAX_NAMES as i64 * 2 - 1)));
    }

    #[test]
    fn the_count_table_holds_no_more_than_its_bound() {
        let mut l = Lockouts::new();
        let now = t(0);
        // Names and addresses invented together, double the count bound, so
        // both tables are walked and neither is allowed to grow past its own.
        for i in 0..(MAX_COUNTS * 2) as u32 {
            l.failed(&format!("n{}", i), crowd(i), now);
            assert!(l.counts() <= MAX_COUNTS, "{} counts held", l.counts());
            assert!(l.per_address.order.len() <= MAX_COUNTS);
            assert!(l.len() <= MAX_NAMES, "{} names held", l.len());
        }
        assert_eq!(l.counts(), MAX_COUNTS);
        // The counts tried first made room for the rest and hold nothing now;
        // the last one tried is still held.
        assert_eq!(l.attempt("n0", crowd(0), now), Attempt::Try);
        assert!(l.per_address.counts.contains_key(&(
            format!("n{}", MAX_COUNTS * 2 - 1),
            crowd(MAX_COUNTS as u32 * 2 - 1)
        )));
    }

    #[test]
    fn an_address_being_worked_over_keeps_its_count_while_addresses_are_invented() {
        let mut l = Lockouts::new();
        let now = t(0);
        // A caller inventing addresses one after another cannot take the seat
        // from the address it is actually guessing at, any more than a caller
        // inventing names can take a name's seat: what it works over is the
        // last thing tried, and a buyer takes the key gone longest without a
        // try. The crowd invents a name with every address, so both tables are
        // walked at once, and stops at each table's bound.
        fail(&mut l, "alice", A, MAX_FAILURES - 1, now);
        let rounds = MAX_COUNTS.min(MAX_NAMES) * 2;
        for i in 0..rounds as u32 {
            l.failed(&format!("n{i}"), crowd(i), now);
            assert!(l.counts() <= MAX_COUNTS, "{} counts held", l.counts());
            assert!(l.len() <= MAX_NAMES, "{} names held", l.len());
        }
        // alice's tenth, both tables walked past their bounds and back, and
        // her windows still open: the wait comes down on the caller that was
        // really guessing at her.
        assert!(l.failed("alice", A, now));
        assert!(wait(&l, "alice", A, now).is_some());
    }

    #[test]
    fn a_full_name_table_makes_room_for_the_name_going_longest_without_a_try() {
        let mut l = Lockouts::new();
        let now = t(0);
        let last = fill(&mut l, now, Duration::seconds(1));
        // The table is full and every seat holds a live count, so the name
        // tried next buys its seat from the one gone longest without a try:
        // n0, tried before all of them, with the walk standing at the front.
        l.failed("newcomer", A, last + Duration::seconds(1));
        assert_eq!(l.len(), MAX_NAMES);
        assert!(!l.per_name.counts.contains_key("n0"));
        assert!(l.per_name.counts.contains_key("newcomer"));
        for i in 1..MAX_NAMES as i64 - 1 {
            assert!(l.per_name.counts.contains_key(&format!("n{i}")), "n{i}");
        }
        // The next name buys the seat behind rather than the same one twice,
        // and a name already in the table is tried without a seat being bought
        // for it at all.
        l.failed("another", A, last + Duration::seconds(2));
        assert!(!l.per_name.counts.contains_key("n1"));
        assert!(l.per_name.counts.contains_key("newcomer"));
        l.failed(&format!("n{LAST}"), A, last + Duration::seconds(3));
        assert!(l.per_name.counts.contains_key(&format!("n{LAST}")));
        assert!(l.per_name.counts.contains_key("n2"));
        assert!(l.per_name.counts.contains_key("another"));
        // A name worked over keeps its seat at the back, where no walk for
        // room reaches it while the caller is still guessing at it.
        for _ in 0..2 * MAX_NAMES {
            l.failed("newcomer", A, last + Duration::seconds(4));
            assert!(l.per_name.counts.contains_key("newcomer"));
            assert!(l.len() <= MAX_NAMES);
        }
    }

    #[test]
    fn a_seat_is_bought_by_the_name_gone_longest_without_a_try() {
        let mut l = Lockouts::new();
        let now = t(0);
        // A count that has run its whole window and closed, and one tried a
        // minute later whose window still has most of itself ahead.
        fail(&mut l, "filled", A, MAX_FAILURES, now);
        l.failed("counting", A, now + Duration::minutes(1));
        // Fill the table around them so the next name has to buy a seat.
        for i in 0..MAX_NAMES as i64 - 2 {
            l.failed(&format!("n{i}"), A, now + Duration::minutes(2));
        }
        // The next name buys its seat from the front of the order: the name
        // that ran longest without a try, whether or not what it was counted
        // for still stands.
        let late = now + WINDOW;
        assert_eq!(l.attempt("filled", A, late), Attempt::Try);
        l.failed("newcomer", A, late);
        assert!(!l.per_name.counts.contains_key("filled"));
        assert!(l.per_name.counts.contains_key("counting"));
        assert!(l.per_name.counts.contains_key("n0"));
    }

    #[test]
    fn a_name_still_being_tried_keeps_its_count_past_a_closed_window() {
        let mut l = Lockouts::new();
        let now = t(0);
        // A full table of stale counts: every window closed, no wait brought
        // down, so each name starts again from the attempt now.
        for i in 0..MAX_NAMES as i64 {
            l.failed(&format!("n{i}"), A, now);
        }
        let later = now + WINDOW + Duration::seconds(1);
        for i in 0..MAX_FAILURES - 1 {
            l.failed("newcomer", A, later + Duration::seconds(i as i64));
        }
        assert!(l.failed("newcomer", A, later + Duration::seconds(9)));
        assert!(wait(&l, "newcomer", A, later + Duration::seconds(9)).is_some());
    }

    #[test]
    fn a_wait_brought_down_by_the_last_failure_of_a_window_is_refused_at_once() {
        // The tenth failure is refused the way any other in the wait is: the
        // answer comes from the count, not from the password that filled it.
        let mut l = Lockouts::new();
        let now = t(0);
        assert!(fail(&mut l, "alice", A, MAX_FAILURES, now));
        assert!(wait(&l, "alice", A, now).is_some());
    }

    #[test]
    fn a_full_count_table_still_counts_a_name_across_addresses() {
        // The name ceiling has to be reachable even when the address table is
        // full and none of its seats belong to the name being worked over.
        let mut l = Lockouts::new();
        let now = t(0);
        for i in 0..MAX_NAMES as i64 {
            l.failed(&format!("n{i}"), A, now);
        }
        fill_counts(&mut l, "crowded", now);
        assert!(fail(&mut l, "alice", A, MAX_FAILURES_PER_NAME, now));
        assert!(wait(&l, "alice", B, now).is_some());
        assert!(l.counts() <= MAX_COUNTS);
    }
}
