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
//!   replaces it only while that winner is not above `b`, and not at all once a push changed the
//!   state meanwhile;
//! - `mine` is exact and, unlike counts, depends on delivery order: an accepted input that says
//!   it (a push or a row naming FIRE, a push clearing it) sets it, one that does not say keeps
//!   the held one while its code still has a count, and a rejected input changes nothing. So v6
//!   saying FIRE then v7 saying nothing ends at FIRE, while v7 then v6 ends at the start's LIKE
//!   (v6 is stale and ignored). The confirmed choice sets `mine` to its target;
//! - the edit changes the body for good, whichever request delivers its older snapshot later;
//! - the state does not depend on whether the cache holds the message: a replaced range may drop
//!   it, and while a request is still out the state accepted so far (pushes and the confirmed
//!   choice included) outlives the row and meets the snapshot that puts it back. Only when no
//!   request is out any more, and the message is not cached, is it forgotten; a copy of the
//!   message that comes later starts from nothing. The exception is an own choice still in
//!   flight: while it is unanswered the state is kept whether or not a request is out.
//!
//! Each request has its own `Shape`, which adds the boundaries of a replaced range. `Single`: the
//! snapshot is message 3 alone and says the chat starts there. `Tail`: the server grows a message
//! 4 the client was not told about, so a request started before it holds a snapshot ending at
//! message 3 while another one stores message 4; message 4 must survive the older request.
//! `Jump`: the request returns a page far above everything cached (message 10), so the cache
//! below it is dropped as a hole in the timeline, except what a live change touched while the
//! request was out (an edit re-inserts message 3, outside the page). The shapes are mixed on
//! purpose: a `Jump` can drop message 3 while a `Single` that saw it is still out, and the
//! `Single` then puts back the state it saw, which must not be older than what the cache had
//! accepted.

use super::*;
use std::collections::BTreeSet;
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
    /// The request is a snapshot of message 3 alone, the whole chat.
    Single,
    /// The request is a snapshot of message 3, and of message 4 once the server has grown it.
    Tail,
    /// The request is a page of message 10 alone, above everything cached.
    Jump,
}

#[derive(Clone, Debug)]
struct Case {
    /// The shape of each of the two requests.
    shapes: [Shape; 2],
    snaps: [Snap; 2],
    pushes: [Push; 2],
    edit: bool,
    own: Own,
    /// The cache starts with message 3 carrying no reaction state (no counts, no version).
    bare: bool,
}

impl Case {
    fn uniform(shape: Shape, snaps: [Snap; 2], pushes: [Push; 2], edit: bool, own: Own) -> Case {
        Case { shapes: [shape; 2], snaps, pushes, edit, own, bare: false }
    }
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
    /// The user's confirmed overlay (or versionless counts) is what the counts are.
    overlay: Option<Vec<ReactionCount>>,
}

impl Held {
    /// No state at all: a message the cache gets without any (a live copy of a message it dropped).
    fn fresh() -> Held {
        Held { version: None, mine: None, overlay: None }
    }

    fn counts(&self) -> Vec<ReactionCount> {
        self.overlay.clone().unwrap_or_else(|| self.version.map_or_else(Vec::new, server_counts))
    }

    /// An input of version `v` (`None`: without one) that says `says` about `mine` (`None`: does
    /// not say). Without a version it counts only while no version is held. Returns whether the
    /// state changed.
    fn offer(&mut self, v: Option<i64>, says: Option<Option<ReactionCode>>) -> bool {
        let before = (self.version, self.mine, self.counts());
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
            _ => return false,
        };
        if let Some(says) = says {
            self.mine = says;
        }
        self.mine = self.mine.filter(|m| counts.iter().any(|c| c.code == *m));
        before != (self.version, self.mine, self.counts())
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

/// Whether the confirmed choice, built on `base`, replaces the state of `held`.
fn own_wins(held: Option<i64>, base: Option<i64>) -> bool {
    match (held, base) {
        (Some(held), Some(base)) => base >= held,
        (None, _) => true,
        (Some(_), None) => false,
    }
}

/// What a request saw when it started, and what reached the chat's cache since.
struct Started {
    /// The newest message the cache held.
    newest: Option<MessageTs>,
    /// The server had grown message 4.
    grown: bool,
    /// The cache changed since: what is newer than the snapshot is kept (`Engine::touch`).
    later: bool,
    /// An edit of message 3 was logged since: the replay puts it back.
    edited: bool,
}

/// The reference: which messages the cache holds and what it holds of message 3.
struct Model {
    present: BTreeSet<MessageTs>,
    held: Held,
    /// Requests started and not yet delivered.
    out: usize,
    grown: bool,
    started: [Option<Started>; 2],
    /// The own reaction in flight: the version and counts it was built on, and `mine` before it.
    reacting: bool,
    base: Option<i64>,
    start_counts: Vec<ReactionCount>,
    started_with: Option<ReactionCode>,
    /// A push changed the cached state while the own reaction was in flight.
    superseded: bool,
}

impl Model {
    fn new(bare: bool) -> Model {
        Model {
            present: BTreeSet::from([TS]),
            held: if bare { Held::fresh() } else { Held { version: Some(START), mine: Some(LIKE), overlay: None } },
            out: 0,
            grown: false,
            started: [None, None],
            reacting: false,
            base: None,
            start_counts: Vec::new(),
            started_with: None,
            superseded: false,
        }
    }

    fn cached(&self, ts: MessageTs) -> bool {
        self.present.contains(&ts)
    }

    /// The cache changed: every request still out must keep what is newer than its snapshot.
    fn touch(&mut self) {
        for started in self.started.iter_mut().flatten() {
            started.later = true;
        }
    }

    fn step(&mut self, case: &Case, step: Step) {
        match step {
            Step::FetchStart(k) => {
                self.started[k] = Some(Started { newest: self.present.last().copied(), grown: self.grown, later: false, edited: false });
                self.out += 1;
            }
            Step::FetchEnd(k) => self.fetch_end(case.shapes[k], case.snaps[k], k),
            Step::Grow => self.grown = true,
            Step::Push(i) => {
                self.touch();
                // Cached or not, the state is accepted: by the cache, or by the ledger while a
                // request is out or the own choice is unanswered. With none, nothing is held and
                // the push is gone.
                if let (true, Some((v, says))) = (self.cached(TS) || self.out > 0 || self.reacting, push_offer(case.pushes[i])) {
                    let changed = self.held.offer(v, says);
                    self.superseded |= changed && self.reacting;
                }
            }
            Step::Edit => {
                self.touch();
                for started in self.started.iter_mut().flatten() {
                    started.edited = true;
                }
                self.present.insert(TS);
            }
            Step::ReactStart => {
                self.reacting = self.cached(TS);
                self.base = self.held.version;
                self.start_counts = self.held.counts();
                self.started_with = self.held.mine;
                self.superseded = false;
            }
            Step::ReactEnd => {
                let reacting = std::mem::take(&mut self.reacting);
                let kept = self.cached(TS) || self.out > 0;
                if reacting && case.own == Own::Confirmed && !self.superseded && kept && own_wins(self.held.version, self.base) {
                    let (overlay, mine) = toggled_to_fire(self.start_counts.clone(), self.started_with);
                    self.held.overlay = Some(overlay);
                    self.held.mine = mine;
                    self.held.version = self.base;
                    if self.cached(TS) {
                        self.touch();
                    }
                }
                self.superseded = false;
                // The choice no longer keeps what the cache accepted of an uncached message.
                if self.out == 0 && !self.cached(TS) {
                    self.held = Held::fresh();
                }
            }
        }
    }

    /// The range a request replaces, from what the engine decides for a first-page request
    /// (`store_chunk`), and what that does to the cache and to message 3's state.
    fn fetch_end(&mut self, shape: Shape, snap: Snap, k: usize) {
        self.out -= 1;
        let started = self.started[k].take().unwrap();
        let rows: Vec<MessageTs> = match shape {
            Shape::Single => vec![TS],
            Shape::Tail if started.grown => vec![TS, 4],
            Shape::Tail => vec![TS],
            Shape::Jump => vec![10],
        };
        let (min, max) = (rows[0], *rows.last().unwrap());
        let hi = (started.later || started.edited).then_some(max);
        let lo = match shape {
            Shape::Single => None,
            _ if started.newest.is_none_or(|newest| newest >= min) => Some(min),
            _ => None,
        };
        let inside = |ts: MessageTs| lo.is_none_or(|lo| ts >= lo) && hi.is_none_or(|hi| ts <= hi);
        let mut after: BTreeSet<MessageTs> = self.present.iter().copied().filter(|ts| !inside(*ts)).collect();
        after.extend(rows.iter().copied());
        // The replay puts an edited message back, inside the replaced range.
        if started.edited && inside(TS) {
            after.insert(TS);
        }
        if rows.contains(&TS)
            && let Some((v, says)) = snap_offer(snap)
        {
            self.held.offer(v, says);
        }
        self.present = after;
        // Nothing keeps the state of an uncached message once no request is out and no own
        // choice is unanswered.
        if self.out == 0 && !self.cached(TS) && !self.reacting {
            self.held = Held::fresh();
        }
        // Requests that started before this one must not take what it stored for stale.
        self.touch();
    }
}

struct Pending {
    mark: u64,
    known_newest: Option<MessageTs>,
    rows: Vec<Message>,
}

/// An engine over a cache with message 3 at version 5 and the account's own reaction LIKE, or,
/// when `bare`, with message 3 carrying no reaction state at all.
fn engine_for(
    refused: bool,
    bare: bool,
) -> (Engine<Fake>, async_channel::Receiver<Input>, async_channel::Receiver<ClientEvent>) {
    let (events, event_rx) = async_channel::unbounded();
    let (inputs, input_rx) = async_channel::unbounded();
    let fake = Fake::default();
    if refused {
        let refused = BackendError::Rejected { reason: "no".into(), permanent: false };
        fake.state().write_results.push_back(Err(refused));
    }
    let mut cache = Cache::open_in_memory().unwrap();
    cache.replace_chats(&[chat("a", Some(TS), None)]).unwrap();
    let mut first = reacted();
    first.reactions = if bare { Reactions::default() } else { reactions(&[(LIKE, 2)], Some(LIKE), START) };
    cache.replace_range(&id("a"), None, None, &[first], Some(true)).unwrap();
    let config = ClientConfig::new(Path::new("/nonexistent"), Path::new("/nonexistent").join("session.json"), account());
    let mut engine = Engine::new(Arc::new(fake), config, events, inputs, cache);
    engine.phase = Phase::Online;
    (engine, input_rx, event_rx)
}

/// Runs `order` and reports the first step after which the cache differs from the invariant.
fn run(case: &Case, order: &[Step], rt: &tokio::runtime::Runtime) -> Result<(), String> {
    let (mut engine, input_rx, event_rx) = engine_for(case.own == Own::Refused, case.bare);
    let mut model = Model::new(case.bare);
    let mut pending: [Option<Pending>; 2] = [None, None];
    let (mut edited, mut grown, mut reacting) = (false, false, false);
    for (at, step) in order.iter().enumerate() {
        let mut delivered = None;
        match *step {
            Step::FetchStart(k) => {
                let rows = match case.shapes[k] {
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
                        if grown && case.shapes[k] == Shape::Tail {
                            rows.push(msg("a", 4, "four"));
                        }
                        rows
                    }
                };
                let known_newest = engine.db(engine.cache.newest_ts(&id("a"))).flatten();
                pending[k] = Some(Pending { mark: engine.begin_snapshots(1), known_newest, rows });
            }
            Step::FetchEnd(k) => {
                let Pending { mark, known_newest, rows } = pending[k].take().unwrap();
                let chunk = HistoryChunk {
                    chat: id("a"),
                    messages: rows,
                    reached_start: case.shapes[k] == Shape::Single,
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
            Step::ReactStart => {
                // A message the cache does not hold is refused: no request goes out.
                reacting = engine.db(engine.cache.message(&id("a"), TS)).flatten().is_some();
                engine.toggle_reaction(id("a"), TS, FIRE);
            }
            Step::ReactEnd => {
                if std::mem::take(&mut reacting) {
                    let input = rt.block_on(input_rx.recv()).unwrap();
                    assert!(matches!(input, Input::Action { .. }));
                    engine.handle(input);
                }
            }
        }
        model.step(case, *step);
        let mut chunk_row = None;
        while let Ok(event) = event_rx.try_recv() {
            if let ClientEvent::History(chunk) = event {
                chunk_row = chunk.messages.into_iter().find(|m| m.timestamp == TS);
            }
        }
        let held = engine.db(engine.cache.message(&id("a"), TS)).flatten();
        let cached = |ts| engine.db(engine.cache.message(&id("a"), ts)).flatten().is_some();
        let present = [3, 4, 10].map(|ts| (cached(ts), model.cached(ts)));
        let (want_counts, want_version, want_mine) = (model.held.counts(), model.held.version, model.held.mine);
        let ok = present.iter().all(|(got, want)| got == want)
            && held.as_ref().is_none_or(|held| {
                held.reactions.counts == want_counts
                    && held.reactions.version == want_version
                    && held.reactions.mine == want_mine
                    && held.edited == edited
                    && delivered.is_none_or(|()| chunk_row.as_ref().is_none_or(|row| row.reactions == held.reactions && row.body == held.body))
            });
        if !ok {
            return Err(format!(
                "{case:?}\nafter step {at} of {order:?}\nheld {held:?} cached (3, 4, 10) {:?}\nwant counts {want_counts:?} version {want_version:?} mine {want_mine:?} cached {:?}\nchunk row {chunk_row:?}",
                present.map(|(got, _)| got),
                present.map(|(_, want)| want),
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
    if case.shapes.contains(&Shape::Tail) {
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
    check_every_interleaving(Case::uniform(
        Shape::Single,
        [Snap::Bare, Snap::Absent],
        [Push::Versionless, Push::At(5)],
        true,
        Own::Confirmed,
    ));
}

/// Two requests, one without a version, a newer one delivered between, and an edit.
#[test]
fn every_interleaving_of_two_snapshots_an_edit_and_a_confirmed_choice() {
    check_every_interleaving(Case::uniform(
        Shape::Single,
        [Snap::Versionless, Snap::At(6)],
        [Push::At(7), Push::Absent],
        true,
        Own::Confirmed,
    ));
    check_every_interleaving(Case::uniform(Shape::Single, [Snap::Versionless, Snap::At(6)], [Push::Absent; 2], true, Own::Off));
}

#[test]
fn every_interleaving_of_older_equal_and_newer_states_with_a_refused_choice() {
    check_every_interleaving(Case::uniform(
        Shape::Single,
        [Snap::At(5), Snap::At(4)],
        [Push::At(6), Push::At(5)],
        false,
        Own::Refused,
    ));
}

#[test]
fn every_interleaving_around_a_confirmed_choice() {
    for (snap, pushes) in [
        (Snap::At(5), [Push::At(6), Push::Versionless]),
        (Snap::At(6), [Push::At(5), Push::At(7)]),
        (Snap::Bare, [Push::At(5), Push::At(6)]),
    ] {
        check_every_interleaving(Case::uniform(Shape::Single, [snap, Snap::Absent], pushes, true, Own::Confirmed));
    }
}

#[test]
fn every_interleaving_without_the_users_own_reaction() {
    check_every_interleaving(Case::uniform(
        Shape::Single,
        [Snap::At(7), Snap::Bare],
        [Push::At(6), Push::Versionless],
        true,
        Own::Off,
    ));
}

/// `mine` follows delivery order: v6 saying FIRE then v7 saying nothing keeps FIRE, v7 then v6
/// keeps the start's LIKE; a push clearing it, and the user's choice over either, are exact too.
#[test]
fn every_interleaving_of_input_that_says_or_does_not_say_mine() {
    check_every_interleaving(Case::uniform(
        Shape::Single,
        [Snap::Absent; 2],
        [Push::AtMine(6), Push::At(7)],
        false,
        Own::Off,
    ));
    check_every_interleaving(Case::uniform(
        Shape::Single,
        [Snap::AtMine(6), Snap::Absent],
        [Push::AtClear(6), Push::At(7)],
        true,
        Own::Confirmed,
    ));
    check_every_interleaving(Case::uniform(
        Shape::Single,
        [Snap::At(7), Snap::AtMine(6)],
        [Push::AtMine(6), Push::AtClear(7)],
        false,
        Own::Confirmed,
    ));
}

/// A request started before the server grew message 4 ends after another one stored it, with
/// pushes and an edit around: message 4 stays (a request only vouches for what it saw).
#[test]
fn every_interleaving_keeps_a_tail_another_request_stored() {
    check_every_interleaving(Case::uniform(Shape::Tail, [Snap::At(5), Snap::At(6)], [Push::At(6), Push::At(7)], false, Own::Off));
    check_every_interleaving(Case::uniform(Shape::Tail, [Snap::At(5), Snap::At(6)], [Push::At(7), Push::Absent], true, Own::Off));
    check_every_interleaving(Case::uniform(Shape::Tail, [Snap::At(5), Snap::At(5)], [Push::At(6), Push::Absent], false, Own::Confirmed));
}

/// A page above everything cached drops the cache below it, except a message a live change
/// touched meanwhile, which is put back with the reactions of the cache, not rolled back to
/// the state it had when the request started.
#[test]
fn every_interleaving_of_a_page_above_the_cache_keeps_the_reactions_of_a_touched_message() {
    check_every_interleaving(Case::uniform(Shape::Jump, [Snap::At(5), Snap::At(5)], [Push::At(6), Push::AtMine(7)], true, Own::Off));
    check_every_interleaving(Case::uniform(Shape::Jump, [Snap::At(5), Snap::Absent], [Push::At(6), Push::AtClear(7)], true, Own::Off));
}

/// Mixed ranges (ReviewRecon5): a page above the cache drops message 3 while a snapshot that saw
/// it is still out. A push, an edit or the user's choice that reached message 3 meanwhile must
/// survive in whichever order the snapshots end: the snapshot puts message 3 back with the
/// state the cache had accepted, never the one it saw.
#[test]
fn every_interleaving_of_a_snapshot_and_a_page_above_the_cache() {
    for shapes in [[Shape::Single, Shape::Jump], [Shape::Jump, Shape::Single]] {
        for (snaps, pushes, edit, own) in [
            ([Snap::At(5), Snap::Bare], [Push::At(7), Push::Absent], false, Own::Off),
            ([Snap::At(5), Snap::Bare], [Push::At(6), Push::AtMine(7)], true, Own::Off),
            ([Snap::At(6), Snap::Bare], [Push::AtClear(7), Push::Absent], false, Own::Confirmed),
        ] {
            check_every_interleaving(Case { shapes, snaps, pushes, edit, own, bare: false });
        }
    }
}

/// ReviewRecon5: cache has message 3 at version 5; request A saw it at version 5; request B is a
/// page of message 10, above the cache; a push takes message 3 to version 7; B ends and drops
/// message 3; A ends. A must not bring back version 5.
#[test]
fn a_snapshot_must_not_bring_back_an_older_state_of_a_message_a_page_dropped() {
    let (mut engine, _input_rx, event_rx) = engine_for(false, false);
    let rt = runtime();
    let _enter = rt.enter();
    let page = |rows: Vec<Message>, reached_start| HistoryChunk {
        chat: id("a"),
        messages: rows,
        reached_start,
        replaces: None,
        payloads: Vec::new(),
    };

    let mut seen = msg("a", TS, "hi");
    seen.reactions = Reactions { counts: server_counts(5), mine: None, version: Some(5) };
    let a = HistoryFetch {
        chat: id("a"),
        before: None,
        known_newest: Some(TS),
        mark: engine.begin_snapshots(1),
        result: Ok(page(vec![seen], true)),
    };
    let b = HistoryFetch {
        chat: id("a"),
        before: None,
        known_newest: Some(TS),
        mark: engine.begin_snapshots(1),
        result: Ok(page(vec![msg("a", 10, "ten")], false)),
    };
    engine.handle_live(LiveEvent::ReactionsChanged {
        chat: id("a"),
        timestamp: TS,
        update: ReactionsUpdate { counts: server_counts(7), version: Some(7), mine: None },
    });
    engine.handle(Input::History(b));
    assert!(engine.db(engine.cache.message(&id("a"), TS)).flatten().is_none(), "the page drops message 3");
    while event_rx.try_recv().is_ok() {}
    engine.handle(Input::History(a));

    let held = engine.db(engine.cache.message(&id("a"), TS)).flatten().expect("message 3 is back");
    assert_eq!(held.reactions.version, Some(7));
    assert_eq!(held.reactions.counts, server_counts(7));
    let shown = std::iter::from_fn(|| event_rx.try_recv().ok())
        .find_map(|event| match event {
            ClientEvent::History(chunk) => chunk.messages.into_iter().find(|m| m.timestamp == TS),
            _ => None,
        })
        .expect("the snapshot's chunk is shown");
    assert_eq!(shown.reactions, held.reactions, "the chunk shows what the cache holds");
    assert!(engine.reaction_ledger.is_empty(), "nothing is kept once no request is out");
}

/// ReviewRecon6: message 3 has no reaction state; request A holds its bare row, request B is a
/// page of message 10; the user chooses FIRE; B ends and drops message 3; a versionless push
/// changes its counts; the choice is confirmed; A ends. The server's push replaced the overlay
/// (as it does for a cached message), so the confirmation must not overwrite it and A must show
/// the push, not the overlay.
#[test]
fn a_push_for_an_evicted_message_supersedes_the_pending_choice() {
    let (mut engine, input_rx, event_rx) = engine_for(false, true);
    let rt = runtime();
    let _enter = rt.enter();
    let page = |rows: Vec<Message>, reached_start| HistoryChunk {
        chat: id("a"),
        messages: rows,
        reached_start,
        replaces: None,
        payloads: Vec::new(),
    };
    let a = HistoryFetch {
        chat: id("a"),
        before: None,
        known_newest: Some(TS),
        mark: engine.begin_snapshots(1),
        result: Ok(page(vec![msg("a", TS, "hi")], true)),
    };
    let b = HistoryFetch {
        chat: id("a"),
        before: None,
        known_newest: Some(TS),
        mark: engine.begin_snapshots(1),
        result: Ok(page(vec![msg("a", 10, "ten")], false)),
    };
    engine.toggle_reaction(id("a"), TS, FIRE);
    engine.handle(Input::History(b));
    assert!(engine.db(engine.cache.message(&id("a"), TS)).flatten().is_none(), "the page drops message 3");
    engine.handle_live(LiveEvent::ReactionsChanged {
        chat: id("a"),
        timestamp: TS,
        update: ReactionsUpdate { counts: junk(), version: None, mine: None },
    });
    let input = rt.block_on(input_rx.recv()).unwrap();
    assert!(matches!(input, Input::Action { .. }));
    engine.handle(input);
    while event_rx.try_recv().is_ok() {}
    engine.handle(Input::History(a));

    let held = engine.db(engine.cache.message(&id("a"), TS)).flatten().expect("message 3 is back");
    assert_eq!(held.reactions.counts, junk(), "the push, not the overlay");
    let shown = std::iter::from_fn(|| event_rx.try_recv().ok())
        .find_map(|event| match event {
            ClientEvent::History(chunk) => chunk.messages.into_iter().find(|m| m.timestamp == TS),
            _ => None,
        })
        .expect("the snapshot's chunk is shown");
    assert_eq!(shown.reactions, held.reactions);
}

/// The same, in every order: versionless and versioned pushes for a message the cache dropped
/// while the user's choice is in flight, with the cache starting without reaction state.
#[test]
fn every_interleaving_of_pushes_for_an_evicted_message_and_a_pending_choice() {
    for shapes in [[Shape::Single, Shape::Jump], [Shape::Jump, Shape::Single]] {
        for pushes in [[Push::Versionless, Push::Absent], [Push::Versionless, Push::At(6)]] {
            check_every_interleaving(Case {
                shapes,
                snaps: [Snap::Bare, Snap::Bare],
                pushes,
                edit: false,
                own: Own::Confirmed,
                bare: true,
            });
        }
    }
}

/// ReviewRecon7: the only request drops message 3 while the user's choice is out; a request that
/// starts later brings a push that repeats the state the cache had accepted. That is no change,
/// so the confirmation must still count and the page must show the choice, not version 5 without
/// it. What the cache accepted stays in the ledger for the choice even with no request out.
#[test]
fn a_push_repeating_the_base_after_the_ledger_was_dropped_does_not_supersede_the_choice() {
    let (mut engine, input_rx, event_rx) = engine_for(false, false);
    let rt = runtime();
    let _enter = rt.enter();
    let b = HistoryFetch {
        chat: id("a"),
        before: None,
        known_newest: Some(TS),
        mark: engine.begin_snapshots(1),
        result: Ok(HistoryChunk {
            chat: id("a"),
            messages: vec![msg("a", 10, "ten")],
            reached_start: false,
            replaces: None,
            payloads: Vec::new(),
        }),
    };
    engine.toggle_reaction(id("a"), TS, FIRE);
    engine.handle(Input::History(b));
    assert!(engine.db(engine.cache.message(&id("a"), TS)).flatten().is_none(), "the page drops message 3");
    assert_eq!(
        engine.reaction_ledger.values().map(|held| held.len()).sum::<usize>(),
        1,
        "no request is out: only the state the unanswered choice relies on stays"
    );
    let mut seen = msg("a", TS, "hi");
    seen.reactions = reactions(&[(LIKE, 2)], Some(LIKE), START);
    let a = HistoryFetch {
        chat: id("a"),
        before: None,
        known_newest: Some(10),
        mark: engine.begin_snapshots(1),
        result: Ok(HistoryChunk {
            chat: id("a"),
            messages: vec![seen],
            reached_start: true,
            replaces: None,
            payloads: Vec::new(),
        }),
    };
    engine.handle_live(LiveEvent::ReactionsChanged {
        chat: id("a"),
        timestamp: TS,
        update: ReactionsUpdate { counts: server_counts(START), version: Some(START), mine: None },
    });
    let input = rt.block_on(input_rx.recv()).unwrap();
    assert!(matches!(input, Input::Action { .. }));
    engine.handle(input);
    while event_rx.try_recv().is_ok() {}
    engine.handle(Input::History(a));

    let (counts, mine) = toggled_to_fire(server_counts(START), Some(LIKE));
    let want = Reactions { counts, mine, version: Some(START) };
    let held = engine.db(engine.cache.message(&id("a"), TS)).flatten().expect("message 3 is back");
    assert_eq!(held.reactions, want, "the confirmed choice survives the repeated state");
    let shown = std::iter::from_fn(|| event_rx.try_recv().ok())
        .find_map(|event| match event {
            ClientEvent::History(chunk) => chunk.messages.into_iter().find(|m| m.timestamp == TS),
            _ => None,
        })
        .expect("the snapshot's chunk is shown");
    assert_eq!(shown.reactions, want);
}

fn page_fetch(engine: &mut Engine<Fake>, known_newest: Option<MessageTs>, rows: Vec<Message>, reached_start: bool) -> HistoryFetch {
    HistoryFetch {
        chat: id("a"),
        before: None,
        known_newest,
        mark: engine.begin_snapshots(1),
        result: Ok(HistoryChunk { chat: id("a"), messages: rows, reached_start, replaces: None, payloads: Vec::new() }),
    }
}

/// ReviewRecon7 (second finding): the state kept for an unanswered choice follows the message
/// across several entries and exits of the row, not only the state at the time of the choice.
/// The only page above the cache drops message 3 and the only snapshot brings it back bare (it
/// takes the state the cache accepted); another page then drops the bare row while a snapshot
/// that holds it is out. A versionless push meets the accepted version 5 (and is rejected, as it
/// would be in the cached row), and the confirmed choice wins over version 5 when that snapshot
/// ends.
#[test]
fn the_state_kept_for_a_pending_choice_survives_the_row_coming_and_going() {
    let (mut engine, input_rx, event_rx) = engine_for(false, false);
    let rt = runtime();
    let _enter = rt.enter();
    engine.toggle_reaction(id("a"), TS, FIRE);

    let jump = page_fetch(&mut engine, Some(TS), vec![msg("a", 10, "ten")], false);
    engine.handle(Input::History(jump));
    let bare = page_fetch(&mut engine, Some(10), vec![msg("a", TS, "hi")], true);
    engine.handle(Input::History(bare));
    let back = engine.db(engine.cache.message(&id("a"), TS)).flatten().expect("the snapshot puts message 3 back");
    assert_eq!(back.reactions.version, Some(START), "a bare row takes the state the cache accepted");

    let held_snapshot = page_fetch(&mut engine, Some(TS), vec![msg("a", TS, "hi")], true);
    let jump_again = page_fetch(&mut engine, Some(TS), vec![msg("a", 10, "ten")], false);
    engine.handle(Input::History(jump_again));
    assert!(engine.db(engine.cache.message(&id("a"), TS)).flatten().is_none(), "the page drops message 3 again");
    engine.handle_live(LiveEvent::ReactionsChanged {
        chat: id("a"),
        timestamp: TS,
        update: ReactionsUpdate { counts: junk(), version: None, mine: None },
    });
    let input = rt.block_on(input_rx.recv()).unwrap();
    assert!(matches!(input, Input::Action { .. }));
    engine.handle(input);
    while event_rx.try_recv().is_ok() {}
    engine.handle(Input::History(held_snapshot));

    let (counts, mine) = toggled_to_fire(server_counts(START), Some(LIKE));
    let want = Reactions { counts, mine, version: Some(START) };
    let held = engine.db(engine.cache.message(&id("a"), TS)).flatten().expect("message 3 is back");
    assert_eq!(held.reactions, want, "the choice over the accepted version 5, without the rejected push");
    let shown = std::iter::from_fn(|| event_rx.try_recv().ok())
        .find_map(|event| match event {
            ClientEvent::History(chunk) => chunk.messages.into_iter().find(|m| m.timestamp == TS),
            _ => None,
        })
        .expect("the snapshot's chunk is shown");
    assert_eq!(shown.reactions, want);
}

/// The same, in every order: a page above the cache drops message 3 while the choice is out, and
/// the pushes repeat or raise the state it was built on.
#[test]
fn every_interleaving_of_pushes_after_the_ledger_was_dropped_under_a_pending_choice() {
    for shapes in [[Shape::Jump, Shape::Single], [Shape::Single, Shape::Jump]] {
        for pushes in [[Push::At(5), Push::Absent], [Push::Versionless, Push::At(5)], [Push::At(5), Push::At(6)]] {
            for snaps in [[Snap::At(5), Snap::At(5)], [Snap::Bare, Snap::At(5)], [Snap::At(5), Snap::Bare]] {
                check_every_interleaving(Case { shapes, snaps, pushes, edit: false, own: Own::Confirmed, bare: false });
            }
        }
    }
}

/// ReviewRecon3 #1: the page is far above the cache, an edit and then a version-6 push reach
/// message 3 while it is out; the edit puts message 3 back, and it must keep version 6.
#[test]
fn a_message_put_back_outside_the_page_keeps_the_state_the_cache_reached() {
    let case = Case::uniform(Shape::Jump, [Snap::At(5), Snap::Absent], [Push::At(6), Push::Absent], true, Own::Off);
    let order = [Step::FetchStart(0), Step::Edit, Step::Push(0), Step::FetchEnd(0)];
    let rt = runtime();
    let _enter = rt.enter();
    run(&case, &order, &rt).unwrap_or_else(|failure| panic!("{failure}"));
}

/// ReviewRecon3 #2: a request that saw message 3 as the newest ends after another one stored
/// message 4 and a push touched message 3; message 4 must stay.
#[test]
fn a_tail_another_request_stored_survives_a_request_that_started_before_it() {
    let case = Case::uniform(Shape::Tail, [Snap::At(5), Snap::At(5)], [Push::At(6), Push::Absent], false, Own::Off);
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
    for _ in 0..6000 {
        let shapes = [[Shape::Single, Shape::Tail, Shape::Jump][next(3)], [Shape::Single, Shape::Tail, Shape::Jump][next(3)]];
        let case = Case {
            shapes,
            snaps: [snap(next(8)), snap(next(8))],
            pushes: [push(next(9)), push(next(9))],
            edit: next(2) == 1,
            own: [Own::Off, Own::Confirmed, Own::Refused][next(3)],
            bare: next(3) == 0,
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
