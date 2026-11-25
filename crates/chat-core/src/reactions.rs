//! Message reactions: the domain types and their pure rules. A message carries at most one
//! reaction of the account (`mine`) next to the per-type counts the server reports.

use serde::{Deserialize, Serialize};

/// Server reaction type code. Codes outside [`KNOWN`] are legitimate and stay clickable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ReactionCode(pub i64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReactionCount {
    pub code: ReactionCode,
    pub count: u32,
}

/// Reaction state of one message. `counts` keeps the server's order and never holds a zero
/// count; `version` is the server's `ReactionsVersion`, when it told one.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Reactions {
    pub counts: Vec<ReactionCount>,
    /// The account's own reaction, if known.
    pub mine: Option<ReactionCode>,
    pub version: Option<i64>,
}

/// Server state of the reactions of one message, from a push or a history row.
#[derive(Clone, Debug, PartialEq)]
pub struct ReactionsUpdate {
    pub counts: Vec<ReactionCount>,
    pub version: Option<i64>,
    /// Outer `None`: the event does not say what the account's reaction is; `Some(None)`: the
    /// account has none.
    pub mine: Option<Option<ReactionCode>>,
}

/// A reaction type the picker offers and its emoji.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KnownReaction {
    pub code: ReactionCode,
    pub emoji: &'static str,
}

/// The confirmed (code, emoji) pairs in picker order, transcribed from the "Reaction types"
/// section of `openspec/changes/add-message-reactions/research.md` (the web client's default
/// private-chat picker without the unconfirmed `100108`). Empty would hide the picker only
/// (chips still render with [`NEUTRAL_GLYPH`]).
pub const KNOWN: &[KnownReaction] = &[
    KnownReaction {
        code: ReactionCode(100109),
        emoji: "\u{2764}\u{FE0F}",
    }, // HEART ❤️
    KnownReaction {
        code: ReactionCode(100104),
        emoji: "\u{1F525}",
    }, // FIRE 🔥
    KnownReaction {
        code: ReactionCode(100102),
        emoji: "\u{1F44D}",
    }, // LIKE 👍
    KnownReaction {
        code: ReactionCode(100103),
        emoji: "\u{1F44E}",
    }, // DISLIKE 👎
    KnownReaction {
        code: ReactionCode(100106),
        emoji: "\u{1F632}",
    }, // SURPRISED 😲
    KnownReaction {
        code: ReactionCode(100105),
        emoji: "\u{1F62D}",
    }, // CRYING 😭
    KnownReaction {
        code: ReactionCode(100110),
        emoji: "\u{1F440}",
    }, // EYES 👀
    KnownReaction {
        code: ReactionCode(100101),
        emoji: "\u{2705}",
    }, // CHECK ✅
];

/// Drawn for a code that is not in [`KNOWN`].
pub const NEUTRAL_GLYPH: &str = "❔";

pub fn emoji(code: ReactionCode) -> Option<&'static str> {
    KNOWN
        .iter()
        .find(|known| known.code == code)
        .map(|known| known.emoji)
}

// The reaction state of a message is kept by one rule (design D5, "Invariant"). States are
// ordered by `version`, a missing version ranking below every version: the higher one wins and
// a state never replaces a higher one. Input without a version is therefore applied only while
// no version is held, and a state never takes a version its input did not carry. At the same
// version two states are the same server state, so the held one stays, except for the user's own
// confirmed choice, which is built on that version and survives it. Every way a state can enter
// (push, history row, edit copy, confirmed own choice) goes through `accept`.
impl Reactions {
    /// Applies the server's state from a push (see the rule above). `mine` follows the update
    /// when it says one and is dropped when its type has no count left. Returns whether
    /// anything changed.
    pub fn apply(&mut self, update: &ReactionsUpdate) -> bool {
        self.accept(&update.counts, update.version, update.mine, false)
    }

    /// Merges a state read from the server or the cache (a history row, an edit copy): what
    /// [`Reactions::apply`] does for a push that does not say `mine`. A state that carries no
    /// reactions and no version ([`Reactions::is_unknown`]) says nothing and changes nothing.
    pub fn merge(&mut self, incoming: &Reactions) -> bool {
        !incoming.is_unknown()
            && self.accept(
                &incoming.counts,
                incoming.version,
                incoming.mine.map(Some),
                false,
            )
    }

    /// Puts the user's own confirmed choice `own` (built on the server state of its version) over
    /// `self`: it survives the state of its base version and any lower one, never a higher one.
    /// Returns whether anything changed.
    pub fn overlay_own(&mut self, own: &Reactions) -> bool {
        self.accept(&own.counts, own.version, Some(own.mine), true)
    }

    /// Whether this carries no reaction state at all: no counts and no version (an edit push, a
    /// history row without reactions).
    pub fn is_unknown(&self) -> bool {
        self.counts.is_empty() && self.version.is_none()
    }

    /// The one merge. `mine`: `None` when the input does not say, else the account's reaction.
    fn accept(
        &mut self,
        counts: &[ReactionCount],
        version: Option<i64>,
        mine: Option<Option<ReactionCode>>,
        own: bool,
    ) -> bool {
        let wins = match (self.version, version) {
            (Some(held), Some(new)) => new > held || (new == held && own),
            (None, Some(_)) => true,
            (held, None) => held.is_none(),
        };
        if !wins {
            return false;
        }
        let mine = mine
            .unwrap_or(self.mine)
            .filter(|code| counts.iter().any(|count| count.code == *code));
        let changed = self.counts != counts || self.mine != mine || self.version != version;
        if changed {
            self.counts.clear();
            self.counts.extend_from_slice(counts);
            self.mine = mine;
            self.version = version;
        }
        changed
    }

    /// The state the user's choice of `code` leads to, and the reaction to ask the server for
    /// (`None` removes). Choosing the own type again removes it; any other type replaces the
    /// own one. The version is kept, so a newer push overwrites the overlay.
    pub fn toggled(&self, code: ReactionCode) -> (Reactions, Option<ReactionCode>) {
        let target = if self.mine == Some(code) {
            None
        } else {
            Some(code)
        };
        let mut overlay = self.clone();
        if let Some(old) = self.mine
            && let Some(at) = overlay.counts.iter().position(|count| count.code == old)
        {
            overlay.counts[at].count = overlay.counts[at].count.saturating_sub(1);
            if overlay.counts[at].count == 0 {
                overlay.counts.remove(at);
            }
        }
        if let Some(new) = target {
            match overlay.counts.iter_mut().find(|count| count.code == new) {
                Some(count) => count.count += 1,
                None => overlay.counts.push(ReactionCount {
                    code: new,
                    count: 1,
                }),
            }
        }
        overlay.mine = target;
        (overlay, target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(n: i64) -> ReactionCode {
        ReactionCode(n)
    }

    fn counts(pairs: &[(i64, u32)]) -> Vec<ReactionCount> {
        pairs
            .iter()
            .map(|&(c, count)| ReactionCount {
                code: code(c),
                count,
            })
            .collect()
    }

    fn state(pairs: &[(i64, u32)], mine: Option<i64>, version: Option<i64>) -> Reactions {
        Reactions {
            counts: counts(pairs),
            mine: mine.map(code),
            version,
        }
    }

    fn update(
        pairs: &[(i64, u32)],
        version: Option<i64>,
        mine: Option<Option<i64>>,
    ) -> ReactionsUpdate {
        ReactionsUpdate {
            counts: counts(pairs),
            version,
            mine: mine.map(|m| m.map(code)),
        }
    }

    #[test]
    fn apply_ignores_an_older_and_an_equal_version() {
        let held = state(&[(1, 2)], Some(1), Some(5));
        for version in [4, 5] {
            let mut r = held.clone();
            assert!(!r.apply(&update(&[(2, 9)], Some(version), Some(Some(2)))));
            assert_eq!(r, held);
        }
    }

    #[test]
    fn apply_takes_a_newer_version() {
        let mut r = state(&[(1, 2)], Some(1), Some(5));
        assert!(r.apply(&update(&[(1, 1), (2, 1)], Some(6), None)));
        assert_eq!(r, state(&[(1, 1), (2, 1)], Some(1), Some(6)));
    }

    #[test]
    fn apply_takes_the_version_of_the_update() {
        let mut unversioned = state(&[(1, 2)], None, None);
        assert!(unversioned.apply(&update(&[(1, 3)], Some(1), None)));
        assert_eq!(unversioned, state(&[(1, 3)], None, Some(1)));
    }

    #[test]
    fn apply_keeps_mine_when_the_update_does_not_say() {
        let mut r = state(&[(1, 1)], Some(1), Some(1));
        assert!(r.apply(&update(&[(1, 2)], Some(2), None)));
        assert_eq!(r.mine, Some(code(1)));
    }

    #[test]
    fn apply_sets_and_clears_mine_when_the_update_says() {
        let mut r = state(&[(1, 1)], Some(1), Some(1));
        assert!(r.apply(&update(&[(1, 1), (2, 1)], Some(2), Some(Some(2)))));
        assert_eq!(r.mine, Some(code(2)));
        assert!(r.apply(&update(&[(1, 1), (2, 1)], Some(3), Some(None))));
        assert_eq!(r.mine, None);
    }

    #[test]
    fn apply_clears_mine_whose_code_left_the_counts() {
        let mut r = state(&[(1, 1), (2, 1)], Some(1), Some(1));
        assert!(r.apply(&update(&[(2, 1)], Some(2), None)));
        assert_eq!(r, state(&[(2, 1)], None, Some(2)));
        let mut said = state(&[], None, None);
        assert!(said.apply(&update(&[(2, 1)], None, Some(Some(1)))));
        assert_eq!(said.mine, None);
    }

    #[test]
    fn apply_reports_no_change_for_an_identical_state() {
        let mut r = state(&[(1, 1)], Some(1), None);
        assert!(!r.apply(&update(&[(1, 1)], None, None)));
        assert!(!r.apply(&update(&[(1, 1)], None, Some(Some(1)))));
    }

    #[test]
    fn a_versionless_update_never_overrides_a_versioned_state_nor_gives_it_a_version() {
        let held = state(&[(1, 2)], Some(1), Some(5));
        let mut r = held.clone();
        assert!(!r.apply(&update(&[(1, 3)], None, Some(Some(1)))));
        assert_eq!(r, held);
        let mut versionless = state(&[(1, 2)], None, None);
        assert!(versionless.apply(&update(&[(1, 3)], None, None)));
        assert_eq!(
            versionless,
            state(&[(1, 3)], None, None),
            "later wins among versionless states"
        );
        assert!(
            versionless.apply(&update(&[(1, 4)], Some(1), None)),
            "any version beats none"
        );
        assert_eq!(versionless.version, Some(1));
    }

    #[test]
    fn merge_follows_the_same_order_and_ignores_an_unknown_state() {
        let held = state(&[(1, 2)], Some(1), Some(5));
        for (incoming, expected) in [
            (state(&[(1, 9)], None, Some(4)), held.clone()),
            (state(&[(1, 9)], None, Some(5)), held.clone()),
            (state(&[(1, 9)], None, None), held.clone()),
            (state(&[], None, None), held.clone()),
            (
                state(&[(1, 3)], None, Some(6)),
                state(&[(1, 3)], Some(1), Some(6)),
            ),
            (
                state(&[(2, 1)], Some(2), Some(6)),
                state(&[(2, 1)], Some(2), Some(6)),
            ),
        ] {
            let mut r = held.clone();
            r.merge(&incoming);
            assert_eq!(r, expected, "incoming {incoming:?}");
        }
        let mut bare = state(&[], None, None);
        assert!(bare.merge(&state(&[(1, 1)], None, None)));
        assert_eq!(bare, state(&[(1, 1)], None, None));
    }

    #[test]
    fn overlay_own_replaces_the_server_state_of_its_version_or_a_lower_one() {
        let own = state(&[(1, 2), (2, 1)], Some(2), Some(5));
        for held in [
            state(&[(1, 2)], None, Some(5)),
            state(&[(1, 1)], None, Some(4)),
            state(&[], None, None),
        ] {
            let mut r = held;
            assert!(r.overlay_own(&own));
            assert_eq!(r, own);
        }
        let mut same = own.clone();
        assert!(
            !same.overlay_own(&own),
            "nothing changes for the same state"
        );
    }

    #[test]
    fn overlay_own_leaves_a_newer_state_alone_and_a_versionless_one_never_beats_a_version() {
        let newer = state(&[(1, 3)], None, Some(7));
        let mut r = newer.clone();
        assert!(!r.overlay_own(&state(&[(1, 2), (2, 1)], Some(2), Some(5))));
        assert_eq!(r, newer);
        assert!(!r.overlay_own(&state(&[(1, 2), (2, 1)], Some(2), None)));
        assert_eq!(r, newer);
    }

    #[test]
    fn the_held_state_stays_at_the_same_version_even_against_a_snapshot_that_says_mine() {
        let mut held = state(&[(1, 2)], None, Some(5));
        assert!(!held.merge(&state(&[(1, 2)], Some(1), Some(5))));
        assert_eq!(held.mine, None);
    }

    #[test]
    fn toggled_first_reaction_adds_it() {
        let (overlay, target) = state(&[(2, 3)], None, Some(4)).toggled(code(1));
        assert_eq!(target, Some(code(1)));
        assert_eq!(overlay, state(&[(2, 3), (1, 1)], Some(1), Some(4)));
    }

    #[test]
    fn toggled_first_reaction_raises_an_existing_count() {
        let (overlay, target) = state(&[(1, 2)], None, None).toggled(code(1));
        assert_eq!(
            (overlay, target),
            (state(&[(1, 3)], Some(1), None), Some(code(1)))
        );
    }

    #[test]
    fn toggled_replace_lowers_the_old_count_and_drops_it_at_zero() {
        let (overlay, target) = state(&[(1, 2), (2, 1)], Some(1), None).toggled(code(2));
        assert_eq!(target, Some(code(2)));
        assert_eq!(overlay, state(&[(1, 1), (2, 2)], Some(2), None));
        let (overlay, _) = state(&[(1, 1), (2, 1)], Some(1), None).toggled(code(2));
        assert_eq!(overlay, state(&[(2, 2)], Some(2), None));
    }

    #[test]
    fn toggled_own_type_removes_it() {
        let (overlay, target) = state(&[(1, 2)], Some(1), Some(3)).toggled(code(1));
        assert_eq!((overlay, target), (state(&[(1, 1)], None, Some(3)), None));
        let (overlay, target) = state(&[(1, 1), (2, 1)], Some(1), None).toggled(code(1));
        assert_eq!((overlay, target), (state(&[(2, 1)], None, None), None));
    }

    #[test]
    fn toggled_appends_a_new_type_last() {
        let (overlay, _) = state(&[(1, 1), (2, 1)], Some(1), None).toggled(code(9));
        assert_eq!(overlay.counts, counts(&[(2, 1), (9, 1)]));
        let (overlay, _) = state(&[(1, 2), (2, 1)], Some(1), None).toggled(code(9));
        assert_eq!(overlay.counts, counts(&[(1, 1), (2, 1), (9, 1)]));
    }

    #[test]
    fn toggled_does_not_change_the_receiver() {
        let r = state(&[(1, 1)], Some(1), Some(1));
        let _ = r.toggled(code(2));
        assert_eq!(r, state(&[(1, 1)], Some(1), Some(1)));
    }

    #[test]
    fn known_matches_the_research_table() {
        let codes: Vec<i64> = KNOWN.iter().map(|known| known.code.0).collect();
        assert_eq!(
            codes,
            [
                100109, 100104, 100102, 100103, 100106, 100105, 100110, 100101
            ]
        );
    }

    #[test]
    fn known_codes_and_emoji_are_unique_and_non_empty() {
        for (i, known) in KNOWN.iter().enumerate() {
            assert!(!known.emoji.is_empty());
            assert_ne!(known.emoji, NEUTRAL_GLYPH);
            for other in &KNOWN[i + 1..] {
                assert_ne!(known.code, other.code);
                assert_ne!(known.emoji, other.emoji);
            }
        }
    }

    #[test]
    fn emoji_finds_known_codes_only() {
        for known in KNOWN {
            assert_eq!(emoji(known.code), Some(known.emoji));
        }
        assert_eq!(emoji(code(100101)), Some("\u{2705}"));
        assert_eq!(emoji(code(100108)), None);
        assert_eq!(emoji(code(i64::MIN)), None);
    }
}
