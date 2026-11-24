//! Model-based test of the reaction invariant (design D5, "Invariant"). The engine of one chat
//! with one message is driven through every interleaving of a small alphabet of events: two
//! history requests (the server's snapshot is taken when a request starts and delivered when it
//! ends), two reaction pushes, an edit that says nothing about reactions, and the user's own
//! reaction (the request goes out, the answer arrives). After every step the cache is compared
//! with the reference below, which sees the inputs in the order they were delivered.
//!
//! The reference, with the cache starting at version 5 and the account's own reaction LIKE:
//! - counts and version: the state with the highest version among the cache's start, the pushes
//!   delivered and the snapshots delivered wins, whatever the order; an input without a version
//!   never counts (a versioned state is held from the start, and nothing may lower it or lend it
//!   a version);
//! - the user's confirmed choice (the request went out over version `b`, the answer was `Ok`)
//!   replaces it only while that winner is not above `b`;
//! - `mine` is exact and, unlike counts, depends on delivery order: an accepted input that says
//!   it (a push or a row naming FIRE, a push clearing it) sets it, one that does not say keeps
//!   the held one while its code still has a count, and a rejected input changes nothing. So v6
//!   saying FIRE then v7 saying nothing ends at FIRE, while v7 then v6 ends at the start's LIKE
//!   (v6 is stale and ignored). The confirmed choice sets `mine` to its target;
//! - the edit changes the body for good, whichever request delivers its older snapshot later.
//!
//! Beyond the one message, `Shape` adds the boundaries of a replaced range. `Tail`: the server
//! grows a message 4 the client was not told about, so a request started before it holds a
//! snapshot ending at message 3 while another one stores message 4; message 4 must survive the
//! older request. `Jump`: the request returns a page far above everything cached (message 10), so
//! the cache below it is dropped as a hole in the timeline, except what a live change touched
//! while the request was out (an edit re-inserts message 3, outside the page); that message must
//! come out with the reactions of the cache, not the snapshot's.

use super::*;
use std::path::Path;

const TS: MessageTs = 3;
/// The version the cache starts with.
const START: i64 = 5;

#[derive(Clone, Copy, Debug)]
enum Snap {
    /// The request does not take part.
    Absent,
    Bare,
    Versionless,
    /// A row that does not say `mine`.
    At(i64),
    /// A row whose recent list names FIRE as the account's reaction (only where FIRE has a count).
    AtMine(i64),
}

#[derive(Clone, Copy, Debug)]
enum Push {
    /// The push does not take part.
    Absent,
    Versionless,
    /// A push that does not say `mine`.
    At(i64),
    /// A push saying the account's reaction is FIRE (only where FIRE has a count).
    AtMine(i64),
    /// A push saying the account has no reaction.
    AtClear(i64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Own {
    Off,
    Confirmed,
    Refused,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    FetchStart(usize),
    FetchEnd(usize),
    Push(usize),
    Edit,
    ReactStart,
    ReactEnd,
    /// The server gets message 4; the client is not told (a missed push).
    Grow,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shape {
    /// Every request is a snapshot of message 3 alone.
    Single,
    /// A request is a snapshot of message 3, and of message 4 once the server has grown it.
    Tail,
    /// A request is a page of message 10 alone, above everything cached.
    Jump,
}

#[derive(Clone, Debug)]
struct Case {
    shape: Shape,
    snaps: [Snap; 2],
    pushes: [Push; 2],
    edit: bool,
    own: Own,
}

/// The server's counts at `version`: the same version always means the same state.
fn server_counts(version: i64) -> Vec<ReactionCount> {
    match version {
        4 => counts(&[(LIKE, 1)]),
        5 => counts(&[(LIKE, 2)]),
        6 => counts(&[(LIKE, 2), (FIRE, 1)]),
        _ => counts(&[(LIKE, 3), (FIRE, 1)]),
    }
}

/// Counts of input without a version; recognisable if they ever leak into the cache.
fn junk() -> Vec<ReactionCount> {
    counts(&[(FIRE, 9)])
}

/// The user's choice of FIRE over `counts` when the account's reaction is `mine`: the overlay
/// and what `mine` becomes (choosing FIRE again removes it).
fn toggled_to_fire(mut counts: Vec<ReactionCount>, mine: Option<ReactionCode>) -> (Vec<ReactionCount>, Option<ReactionCode>) {
    let lower = |counts: &mut Vec<ReactionCount>, code| {
        if let Some(at) = counts.iter().position(|c| c.code == code) {
            counts[at].count -= 1;
            if counts[at].count == 0 {
                counts.remove(at);
            }
        }
    };
    if let Some(old) = mine {
        lower(&mut counts, old);
    }
    if mine == Some(FIRE) {
        return (counts, None);
    }
    match counts.iter_mut().find(|c| c.code == FIRE) {
        Some(fire) => fire.count += 1,
        None => counts.push(ReactionCount { code: FIRE, count: 1 }),
    }
    (counts, Some(FIRE))
}

/// The reaction state of message 3 as the reference tracks it.
#[derive(Clone)]
struct Held {
    version: Option<i64>,
    mine: Option<ReactionCode>,
    /// The user's confirmed overlay is what the counts are.
    overlay: Option<Vec<ReactionCount>>,
}

impl Held {
    /// A message the cache gets without reaction state (a live copy of a message it dropped).
    fn fresh() -> Held {
        Held { version: None, mine: None, overlay: None }
    }

    /// An input of version `v` (`None`: without one) that says `says` about `mine` (`None`: does
    /// not say). Without a version it counts only while no version is held.
    fn offer(&mut self, v: Option<i64>, says: Option<Option<ReactionCode>>) {
        let counts = match v {
            Some(v) if self.version.is_none_or(|held| v > held) => {
                self.version = Some(v);
                self.overlay = None;
                server_counts(v)
            }
            None if self.version.is_none() => {
                self.overlay = Some(junk());
                junk()
            }
            _ => return,
        };
        if let Some(says) = says {
            self.mine = says;
        }
        self.mine = self.mine.filter(|m| counts.iter().any(|c| c.code == *m));
    }
}

/// What a push or a snapshot row offers: its version and what it says about `mine`; `None` for
/// one that does not take part or carries no reactions.
fn push_offer(push: Push) -> Option<(Option<i64>, Option<Option<ReactionCode>>)> {
    match push {
        Push::At(v) => Some((Some(v), None)),
        Push::AtMine(v) => Some((Some(v), Some(Some(FIRE)))),
        Push::AtClear(v) => Some((Some(v), Some(None))),
        Push::Versionless => Some((None, None)),
        Push::Absent => None,
    }
}

fn snap_offer(snap: Snap) -> Option<(Option<i64>, Option<Option<ReactionCode>>)> {
    match snap {
        Snap::At(v) => Some((Some(v), None)),
        Snap::AtMine(v) => Some((Some(v), Some(Some(FIRE)))),
        Snap::Versionless => Some((None, None)),
        Snap::Absent | Snap::Bare => None,
    }
}

/// What the invariant says the cache holds for message 3 after `trace`, whenever it is cached:
/// counts, version and `mine`. A message the cache dropped has no state; one that comes back
/// (an edit pushed meanwhile, or put back by the replay of an edit over a page above the cache)
/// starts from what the copy carried and takes the pushes that reached it while it was away.
fn expected(case: &Case, trace: &[Step]) -> (Vec<ReactionCount>, Option<i64>, Option<ReactionCode>) {
    let mut held = Held { version: Some(START), mine: Some(LIKE), overlay: None };
    let mut alive = true;
    let (mut base, mut started_with) = (None, None);
    // Per step: was message 3 cached when it happened, and the state an edit's copy carried.
    let mut alive_before = Vec::new();
    let mut edit_copy: Vec<Option<Held>> = Vec::new();
    for (at, step) in trace.iter().enumerate() {
        alive_before.push(alive);
        let mut copy = None;
        match *step {
            Step::Push(i) => {
                if let (true, Some((v, says))) = (alive, push_offer(case.pushes[i])) {
                    held.offer(v, says);
                }
            }
            Step::Edit => {
                if !alive {
                    held = Held::fresh();
                    alive = true;
                }
                copy = Some(held.clone());
            }
            Step::FetchEnd(k) if case.shape == Shape::Jump => {
                let start = trace.iter().position(|s| *s == Step::FetchStart(k)).unwrap();
                let hole = !trace[..start].iter().any(|s| matches!(s, Step::FetchEnd(_)));
                let first_edit = (start..at).find(|j| trace[*j] == Step::Edit);
                match (hole, first_edit) {
                    // The replay puts an edited message back, outside the page.
                    (true, Some(e)) if !alive => {
                        held = edit_copy[e].clone().unwrap();
                        for j in e + 1..at {
                            if let (Step::Push(i), false) = (trace[j], alive_before[j]) {
                                if let Some((v, says)) = push_offer(case.pushes[i]) {
                                    held.offer(v, says);
                                }
                            }
                        }
                        alive = true;
                    }
                    (true, None) => alive = false,
                    _ => {}
                }
            }
            Step::FetchEnd(k) => {
                if let Some((v, says)) = snap_offer(case.snaps[k]) {
                    held.offer(v, says);
                }
            }
            Step::ReactStart => {
                base = Some(held.version);
                started_with = Some(held.mine);
            }
            Step::ReactEnd => {
                if case.own == Own::Confirmed && base == Some(held.version) {
                    let (overlay, mine) = toggled_to_fire(server_counts(held.version.unwrap()), started_with.unwrap());
                    held.overlay = Some(overlay);
                    held.mine = mine;
                }
            }
            Step::FetchStart(_) | Step::Grow => {}
        }
        edit_copy.push(copy);
    }
    let counts = held.overlay.unwrap_or_else(|| held.version.map_or_else(Vec::new, server_counts));
    (counts, held.version, held.mine)
}

/// Which messages the cache holds after `trace`.
struct Presence {
    three: bool,
    four: bool,
}

fn presence(case: &Case, trace: &[Step]) -> Presence {
    let mut now = Presence { three: true, four: false };
    for (at, step) in trace.iter().enumerate() {
        match *step {
            Step::Edit => now.three = true,
            Step::FetchEnd(k) => {
                let start = trace.iter().position(|s| *s == Step::FetchStart(k)).unwrap();
                match case.shape {
                    Shape::Single => {}
                    Shape::Tail => {
                        now.three = true;
                        now.four |= trace[..start].contains(&Step::Grow);
                    }
                    Shape::Jump => {
                        // Only a request that started while nothing above message 3 was cached
                        // drops what is below its page; later ones overlap the cache. An edit
                        // logged meanwhile puts message 3 back, outside the page.
                        let hole = !trace[..start].iter().any(|s| matches!(s, Step::FetchEnd(_)));
                        if hole {
                            now.three = trace[start..at].contains(&Step::Edit);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    now
}

struct Pending {
    mark: u64,
    known_newest: Option<MessageTs>,
    rows: Vec<Message>,
}

/// Runs `order` and reports the first step after which the cache differs from the invariant.
fn run(case: &Case, order: &[Step], rt: &tokio::runtime::Runtime) -> Result<(), String> {
    let (events, event_rx) = async_channel::unbounded();
    let (inputs, input_rx) = async_channel::unbounded();
    let fake = Fake::default();
    if case.own == Own::Refused {
        let refused = BackendError::Rejected { reason: "no".into(), permanent: false };
        fake.state().write_results.push_back(Err(refused));
    }
    let mut cache = Cache::open_in_memory().unwrap();
    cache.replace_chats(&[chat("a", Some(TS), None)]).unwrap();
    let mut first = reacted();
    first.reactions = reactions(&[(LIKE, 2)], Some(LIKE), START);
    cache.replace_range(&id("a"), None, None, &[first], Some(true)).unwrap();
    let config = ClientConfig::new(Path::new("/nonexistent"), Path::new("/nonexistent").join("session.json"), account());
    let mut engine = Engine::new(Arc::new(fake), config, events, inputs, cache);
    engine.phase = Phase::Online;

    let mut pending: [Option<Pending>; 2] = [None, None];
    let (mut edited, mut grown) = (false, false);
    for (at, step) in order.iter().enumerate() {
        let mut delivered = None;
        match *step {
            Step::FetchStart(k) => {
                let rows = match case.shape {
                    Shape::Jump => vec![msg("a", 10, "ten")],
                    Shape::Single | Shape::Tail => {
                        let mut snapshot = msg("a", TS, if edited { "hi!" } else { "hi" });
                        snapshot.edited = edited;
                        snapshot.reactions = match case.snaps[k] {
                            Snap::Absent | Snap::Bare => Reactions::default(),
                            Snap::Versionless => Reactions { counts: junk(), mine: None, version: None },
                            Snap::At(v) => Reactions { counts: server_counts(v), mine: None, version: Some(v) },
                            Snap::AtMine(v) => Reactions { counts: server_counts(v), mine: Some(FIRE), version: Some(v) },
                        };
                        let mut rows = vec![snapshot];
                        if grown && case.shape == Shape::Tail {
                            rows.push(msg("a", 4, "four"));
                        }
                        rows
                    }
                };
                let known_newest = engine.db(engine.cache.newest_ts(&id("a"))).flatten();
                pending[k] = Some(Pending { mark: engine.live_seq, known_newest, rows });
            }
            Step::FetchEnd(k) => {
                let Pending { mark, known_newest, rows } = pending[k].take().unwrap();
                let chunk = HistoryChunk {
                    chat: id("a"),
                    messages: rows,
                    reached_start: case.shape == Shape::Single,
                    replaces: None,
                    payloads: Vec::new(),
                };
                engine.handle(Input::History(HistoryFetch { chat: id("a"), before: None, known_newest, mark, result: Ok(chunk) }));
                delivered = Some(());
            }
            Step::Grow => grown = true,
            Step::Push(i) => {
                let update = match case.pushes[i] {
                    Push::Absent | Push::Versionless => ReactionsUpdate { counts: junk(), version: None, mine: None },
                    Push::At(v) => ReactionsUpdate { counts: server_counts(v), version: Some(v), mine: None },
                    Push::AtMine(v) => ReactionsUpdate { counts: server_counts(v), version: Some(v), mine: Some(Some(FIRE)) },
                    Push::AtClear(v) => ReactionsUpdate { counts: server_counts(v), version: Some(v), mine: Some(None) },
                };
                engine.handle_live(LiveEvent::ReactionsChanged { chat: id("a"), timestamp: TS, update });
            }
            Step::Edit => {
                edited = true;
                let mut message = msg("a", TS, "hi!");
                message.edited = true;
                engine.handle_live(LiveEvent::MessageUpserted(message));
            }
            Step::ReactStart => engine.toggle_reaction(id("a"), TS, FIRE),
            Step::ReactEnd => {
                let input = rt.block_on(input_rx.recv()).unwrap();
                assert!(matches!(input, Input::Action { .. }));
                engine.handle(input);
            }
        }
        let mut chunk_row = None;
        while let Ok(event) = event_rx.try_recv() {
            if let ClientEvent::History(chunk) = event {
                chunk_row = chunk.messages.into_iter().find(|m| m.timestamp == TS);
            }
        }
        let held = engine.db(engine.cache.message(&id("a"), TS)).flatten();
        let four = engine.db(engine.cache.message(&id("a"), 4)).flatten().is_some();
        let here = presence(case, &order[..=at]);
        let (want_counts, want_version, want_mine) = expected(case, &order[..=at]);
        let ok = held.is_some() == here.three
            && four == here.four
            && held.as_ref().is_none_or(|held| {
                held.reactions.counts == want_counts
                    && held.reactions.version == want_version
                    && held.reactions.mine == want_mine
                    && held.edited == edited
                    && delivered.is_none_or(|()| chunk_row.as_ref().is_none_or(|row| row.reactions == held.reactions && row.body == held.body))
            });
        if !ok {
            return Err(format!(
                "{case:?}\nafter step {at} of {order:?}\nheld {held:?} message 4 cached {four}\nwant message 3 {} message 4 {} counts {want_counts:?} version {want_version:?} mine {want_mine:?}\nchunk row {chunk_row:?}",
                here.three, here.four
            ));
        }
    }
    Ok(())
}

/// Every order of the chains that keeps each chain's own order.
fn interleavings(chains: &[Vec<Step>]) -> Vec<Vec<Step>> {
    fn go(chains: &[Vec<Step>], at: &mut Vec<usize>, now: &mut Vec<Step>, out: &mut Vec<Vec<Step>>) {
        let mut moved = false;
        for c in 0..chains.len() {
            if at[c] < chains[c].len() {
                moved = true;
                now.push(chains[c][at[c]]);
                at[c] += 1;
                go(chains, at, now, out);
                at[c] -= 1;
                now.pop();
            }
        }
        if !moved {
            out.push(now.clone());
        }
    }
    let mut out = Vec::new();
    go(chains, &mut vec![0; chains.len()], &mut Vec::new(), &mut out);
    out
}

fn chains(case: &Case) -> Vec<Vec<Step>> {
    let mut chains = Vec::new();
    for k in 0..2 {
        if !matches!(case.snaps[k], Snap::Absent) {
            chains.push(vec![Step::FetchStart(k), Step::FetchEnd(k)]);
        }
        if !matches!(case.pushes[k], Push::Absent) {
            chains.push(vec![Step::Push(k)]);
        }
    }
    if case.edit {
        chains.push(vec![Step::Edit]);
    }
    if case.shape == Shape::Tail {
        chains.push(vec![Step::Grow]);
    }
    if case.own != Own::Off {
        chains.push(vec![Step::ReactStart, Step::ReactEnd]);
    }
    chains
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap()
}

fn check_every_interleaving(case: Case) {
    let rt = runtime();
    let _enter = rt.enter();
    let orders = interleavings(&chains(&case));
    for order in &orders {
        if let Err(failure) = run(&case, order, &rt) {
            panic!("{failure}");
        }
    }
}

/// A push without a version, then one at the cached version, while a snapshot without
/// reactions is out.
#[test]
fn every_interleaving_of_versionless_pushes_over_a_bare_snapshot() {
    check_every_interleaving(Case {
        shape: Shape::Single,
        snaps: [Snap::Bare, Snap::Absent],
        pushes: [Push::Versionless, Push::At(5)],
        edit: true,
        own: Own::Confirmed,
    });
}

/// Two requests, one without a version, a newer one delivered between, and an edit.
#[test]
fn every_interleaving_of_two_snapshots_an_edit_and_a_confirmed_choice() {
    check_every_interleaving(Case {
        shape: Shape::Single,
        snaps: [Snap::Versionless, Snap::At(6)],
        pushes: [Push::At(7), Push::Absent],
        edit: true,
        own: Own::Confirmed,
    });
    check_every_interleaving(Case { snaps: [Snap::Versionless, Snap::At(6)], pushes: [Push::Absent; 2], shape: Shape::Single, edit: true, own: Own::Off });
}

#[test]
fn every_interleaving_of_older_equal_and_newer_states_with_a_refused_choice() {
    check_every_interleaving(Case {
        shape: Shape::Single,
        snaps: [Snap::At(5), Snap::At(4)],
        pushes: [Push::At(6), Push::At(5)],
        edit: false,
        own: Own::Refused,
    });
}

#[test]
fn every_interleaving_around_a_confirmed_choice() {
    for (snap, pushes) in [
        (Snap::At(5), [Push::At(6), Push::Versionless]),
        (Snap::At(6), [Push::At(5), Push::At(7)]),
        (Snap::Bare, [Push::At(5), Push::At(6)]),
    ] {
        check_every_interleaving(Case { snaps: [snap, Snap::Absent], pushes, shape: Shape::Single, edit: true, own: Own::Confirmed });
    }
}

#[test]
fn every_interleaving_without_the_users_own_reaction() {
    check_every_interleaving(Case {
        shape: Shape::Single,
        snaps: [Snap::At(7), Snap::Bare],
        pushes: [Push::At(6), Push::Versionless],
        edit: true,
        own: Own::Off,
    });
}

/// `mine` follows delivery order: v6 saying FIRE then v7 saying nothing keeps FIRE, v7 then v6
/// keeps the start's LIKE; a push clearing it, and the user's choice over either, are exact too.
#[test]
fn every_interleaving_of_input_that_says_or_does_not_say_mine() {
    check_every_interleaving(Case {
        shape: Shape::Single,
        snaps: [Snap::Absent; 2],
        pushes: [Push::AtMine(6), Push::At(7)],
        edit: false,
        own: Own::Off,
    });
    check_every_interleaving(Case {
        shape: Shape::Single,
        snaps: [Snap::AtMine(6), Snap::Absent],
        pushes: [Push::AtClear(6), Push::At(7)],
        edit: true,
        own: Own::Confirmed,
    });
    check_every_interleaving(Case {
        shape: Shape::Single,
        snaps: [Snap::At(7), Snap::AtMine(6)],
        pushes: [Push::AtMine(6), Push::AtClear(7)],
        edit: false,
        own: Own::Confirmed,
    });
}

/// A request started before the server grew message 4 ends after another one stored it, with
/// pushes and an edit around: message 4 stays (a request only vouches for what it saw).
#[test]
fn every_interleaving_keeps_a_tail_another_request_stored() {
    check_every_interleaving(Case {
        shape: Shape::Tail,
        snaps: [Snap::At(5), Snap::At(6)],
        pushes: [Push::At(6), Push::At(7)],
        edit: false,
        own: Own::Off,
    });
    check_every_interleaving(Case {
        shape: Shape::Tail,
        snaps: [Snap::At(5), Snap::At(6)],
        pushes: [Push::At(7), Push::Absent],
        edit: true,
        own: Own::Off,
    });
    check_every_interleaving(Case {
        shape: Shape::Tail,
        snaps: [Snap::At(5), Snap::At(5)],
        pushes: [Push::At(6), Push::Absent],
        edit: false,
        own: Own::Confirmed,
    });
}

/// A page above everything cached drops the cache below it, except a message a live change
/// touched meanwhile, which is put back with the reactions of the cache, not rolled back to
/// the state it had when the request started.
#[test]
fn every_interleaving_of_a_page_above_the_cache_keeps_the_reactions_of_a_touched_message() {
    check_every_interleaving(Case {
        shape: Shape::Jump,
        snaps: [Snap::At(5), Snap::At(5)],
        pushes: [Push::At(6), Push::AtMine(7)],
        edit: true,
        own: Own::Off,
    });
    check_every_interleaving(Case {
        shape: Shape::Jump,
        snaps: [Snap::At(5), Snap::Absent],
        pushes: [Push::At(6), Push::AtClear(7)],
        edit: true,
        own: Own::Off,
    });
}

/// ReviewRecon3 #1: the page is far above the cache, an edit and then a version-6 push reach
/// message 3 while it is out; the edit puts message 3 back, and it must keep version 6.
#[test]
fn a_message_put_back_outside_the_page_keeps_the_state_the_cache_reached() {
    let case = Case {
        shape: Shape::Jump,
        snaps: [Snap::At(5), Snap::Absent],
        pushes: [Push::At(6), Push::Absent],
        edit: true,
        own: Own::Off,
    };
    let order = [Step::FetchStart(0), Step::Edit, Step::Push(0), Step::FetchEnd(0)];
    let rt = runtime();
    let _enter = rt.enter();
    run(&case, &order, &rt).unwrap_or_else(|failure| panic!("{failure}"));
}

/// ReviewRecon3 #2: a request that saw message 3 as the newest ends after another one stored
/// message 4 and a push touched message 3; message 4 must stay.
#[test]
fn a_tail_another_request_stored_survives_a_request_that_started_before_it() {
    let case = Case {
        shape: Shape::Tail,
        snaps: [Snap::At(5), Snap::At(5)],
        pushes: [Push::At(6), Push::Absent],
        edit: false,
        own: Own::Off,
    };
    let order = [Step::FetchStart(0), Step::Grow, Step::FetchStart(1), Step::FetchEnd(1), Step::Push(0), Step::FetchEnd(0)];
    let rt = runtime();
    let _enter = rt.enter();
    run(&case, &order, &rt).unwrap_or_else(|failure| panic!("{failure}"));
}

#[test]
fn random_inputs_in_random_orders() {
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    let mut next = move |n: usize| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state % n as u64) as usize
    };
    let snap = |n: usize| {
        [Snap::Bare, Snap::Versionless, Snap::At(4), Snap::At(5), Snap::At(6), Snap::At(7), Snap::AtMine(6), Snap::AtMine(7)][n]
    };
    let push = |n: usize| {
        [Push::Versionless, Push::At(4), Push::At(5), Push::At(6), Push::At(7), Push::AtMine(6), Push::AtMine(7), Push::AtClear(6), Push::AtClear(7)][n]
    };
    let rt = runtime();
    let _enter = rt.enter();
    for _ in 0..3000 {
        let shape = [Shape::Single, Shape::Tail, Shape::Jump][next(3)];
        let case = Case {
            shape,
            snaps: [snap(next(8)), snap(next(8))],
            pushes: [push(next(9)), push(next(9))],
            edit: next(2) == 1,
            own: if shape == Shape::Jump { Own::Off } else { [Own::Off, Own::Confirmed, Own::Refused][next(3)] },
        };
        let chains = chains(&case);
        let (mut at, mut order) = (vec![0; chains.len()], Vec::new());
        while order.len() < chains.iter().map(Vec::len).sum() {
            let open: Vec<usize> = (0..chains.len()).filter(|&c| at[c] < chains[c].len()).collect();
            let c = open[next(open.len())];
            order.push(chains[c][at[c]]);
            at[c] += 1;
        }
        if let Err(failure) = run(&case, &order, &rt) {
            panic!("{failure}");
        }
    }
}

/// More page fetches than the replay log holds finish while a snapshot is held: the edit of
/// message 3 and the deletion of message 2 that arrived after the snapshot was taken must still
/// be replayed onto it. The pages change nothing the snapshot could undo, so they are noted as
/// "the chat was touched" (the bounds of the replacement) apart from the capped replay log.
#[test]
fn a_flood_of_page_fetches_does_not_push_an_edit_and_a_deletion_out_of_the_replay() {
    let (events, _event_rx) = async_channel::unbounded();
    let (inputs, _input_rx) = async_channel::unbounded();
    let mut cache = Cache::open_in_memory().unwrap();
    cache.replace_chats(&[chat("a", Some(TS), None)]).unwrap();
    let two = msg("a", 2, "two");
    cache.replace_range(&id("a"), None, None, &[two.clone(), reacted()], Some(true)).unwrap();
    let config = ClientConfig::new(Path::new("/nonexistent"), Path::new("/nonexistent").join("session.json"), account());
    let mut engine = Engine::new(Arc::new(Fake::default()), config, events, inputs, cache);
    engine.phase = Phase::Online;
    let rt = runtime();
    let _enter = rt.enter();

    // The held snapshot: both messages as they were, taken before the changes below.
    let mark = engine.live_seq;
    let snapshot = HistoryChunk {
        chat: id("a"),
        messages: vec![two, reacted()],
        reached_start: true,
        replaces: None,
        payloads: Vec::new(),
    };
    let mut edited = msg("a", TS, "hi!");
    edited.edited = true;
    engine.handle_live(LiveEvent::MessageUpserted(edited));
    engine.handle_live(LiveEvent::MessageDeleted { chat: id("a"), timestamp: 2 });
    for _ in 0..LIVE_LOG_CAP + 4 {
        let page = HistoryChunk {
            chat: id("a"),
            messages: vec![msg("a", 1, "one")],
            reached_start: false,
            replaces: None,
            payloads: Vec::new(),
        };
        let fetch = HistoryFetch { chat: id("a"), before: Some(2), known_newest: Some(TS), mark: engine.live_seq, result: Ok(page) };
        engine.handle(Input::History(fetch));
    }
    let fetch = HistoryFetch { chat: id("a"), before: None, known_newest: Some(TS), mark, result: Ok(snapshot) };
    engine.handle(Input::History(fetch));

    let body = |ts| engine.db(engine.cache.message(&id("a"), ts)).flatten().map(|m| m.body);
    assert_eq!(body(3), Some(MessageBody::Text("hi!".into())), "the edit must be replayed");
    assert_eq!(body(2), Some(MessageBody::Deleted), "the deletion must be replayed");
}
