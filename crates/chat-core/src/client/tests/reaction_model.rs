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
}

#[derive(Clone, Debug)]
struct Case {
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

/// What the invariant says the cache holds after `trace`: counts, version and `mine`.
fn expected(case: &Case, trace: &[Step]) -> (Vec<ReactionCount>, i64, Option<ReactionCode>) {
    struct Held {
        version: i64,
        mine: Option<ReactionCode>,
        /// The user's confirmed overlay is what the counts are.
        overlay: Option<Vec<ReactionCount>>,
    }
    let mut held = Held { version: START, mine: Some(LIKE), overlay: None };
    let (mut base, mut started_with) = (None, None);
    // An input of version `v` that says `says` about `mine` (`None`: does not say).
    let offer = |held: &mut Held, v: i64, says: Option<Option<ReactionCode>>| {
        if v > held.version {
            held.version = v;
            held.overlay = None;
            if let Some(says) = says {
                held.mine = says;
            }
            held.mine = held.mine.filter(|m| server_counts(v).iter().any(|c| c.code == *m));
        }
    };
    for step in trace {
        match *step {
            Step::Push(i) => match case.pushes[i] {
                Push::At(v) => offer(&mut held, v, None),
                Push::AtMine(v) => offer(&mut held, v, Some(Some(FIRE))),
                Push::AtClear(v) => offer(&mut held, v, Some(None)),
                Push::Absent | Push::Versionless => {}
            },
            Step::FetchEnd(k) => match case.snaps[k] {
                Snap::At(v) => offer(&mut held, v, None),
                Snap::AtMine(v) => offer(&mut held, v, Some(Some(FIRE))),
                Snap::Absent | Snap::Bare | Snap::Versionless => {}
            },
            Step::ReactStart => {
                base = Some(held.version);
                started_with = Some(held.mine);
            }
            Step::ReactEnd => {
                if case.own == Own::Confirmed && base == Some(held.version) {
                    let (overlay, mine) = toggled_to_fire(server_counts(held.version), started_with.unwrap());
                    held.overlay = Some(overlay);
                    held.mine = mine;
                }
            }
            Step::FetchStart(_) | Step::Edit => {}
        }
    }
    (held.overlay.unwrap_or_else(|| server_counts(held.version)), held.version, held.mine)
}

struct Pending {
    mark: u64,
    known_newest: Option<MessageTs>,
    snapshot: Message,
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
    let mut edited = false;
    for (at, step) in order.iter().enumerate() {
        let mut delivered = None;
        match *step {
            Step::FetchStart(k) => {
                let mut snapshot = msg("a", TS, if edited { "hi!" } else { "hi" });
                snapshot.edited = edited;
                snapshot.reactions = match case.snaps[k] {
                    Snap::Absent | Snap::Bare => Reactions::default(),
                    Snap::Versionless => Reactions { counts: junk(), mine: None, version: None },
                    Snap::At(v) => Reactions { counts: server_counts(v), mine: None, version: Some(v) },
                    Snap::AtMine(v) => Reactions { counts: server_counts(v), mine: Some(FIRE), version: Some(v) },
                };
                let known_newest = engine.db(engine.cache.newest_ts(&id("a"))).flatten();
                pending[k] = Some(Pending { mark: engine.live_seq, known_newest, snapshot });
            }
            Step::FetchEnd(k) => {
                let Pending { mark, known_newest, snapshot } = pending[k].take().unwrap();
                let chunk = HistoryChunk {
                    chat: id("a"),
                    messages: vec![snapshot],
                    reached_start: true,
                    replaces: None,
                    payloads: Vec::new(),
                };
                engine.handle(Input::History(HistoryFetch { chat: id("a"), before: None, known_newest, mark, result: Ok(chunk) }));
                delivered = Some(());
            }
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
        let held = engine.db(engine.cache.message(&id("a"), TS)).flatten().unwrap();
        let (want_counts, want_version, want_mine) = expected(case, &order[..=at]);
        let ok = held.reactions.counts == want_counts
            && held.reactions.version == Some(want_version)
            && held.reactions.mine == want_mine
            && held.edited == edited
            && delivered.is_none_or(|()| chunk_row.as_ref().is_none_or(|row| row.reactions == held.reactions && row.body == held.body));
        if !ok {
            return Err(format!(
                "{case:?}\nafter step {at} of {order:?}\nheld {:?} body {:?} edited {}\nwant counts {want_counts:?} version {want_version} mine {want_mine:?}\nchunk row {chunk_row:?}",
                held.reactions, held.body, held.edited
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
        snaps: [Snap::Versionless, Snap::At(6)],
        pushes: [Push::At(7), Push::Absent],
        edit: true,
        own: Own::Confirmed,
    });
    check_every_interleaving(Case { snaps: [Snap::Versionless, Snap::At(6)], pushes: [Push::Absent; 2], edit: true, own: Own::Off });
}

#[test]
fn every_interleaving_of_older_equal_and_newer_states_with_a_refused_choice() {
    check_every_interleaving(Case {
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
        check_every_interleaving(Case { snaps: [snap, Snap::Absent], pushes, edit: true, own: Own::Confirmed });
    }
}

#[test]
fn every_interleaving_without_the_users_own_reaction() {
    check_every_interleaving(Case {
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
        snaps: [Snap::Absent; 2],
        pushes: [Push::AtMine(6), Push::At(7)],
        edit: false,
        own: Own::Off,
    });
    check_every_interleaving(Case {
        snaps: [Snap::AtMine(6), Snap::Absent],
        pushes: [Push::AtClear(6), Push::At(7)],
        edit: true,
        own: Own::Confirmed,
    });
    check_every_interleaving(Case {
        snaps: [Snap::At(7), Snap::AtMine(6)],
        pushes: [Push::AtMine(6), Push::AtClear(7)],
        edit: false,
        own: Own::Confirmed,
    });
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
    for _ in 0..4000 {
        let case = Case {
            snaps: [snap(next(8)), snap(next(8))],
            pushes: [push(next(9)), push(next(9))],
            edit: next(2) == 1,
            own: [Own::Off, Own::Confirmed, Own::Refused][next(3)],
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
