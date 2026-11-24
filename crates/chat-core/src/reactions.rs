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

/// The confirmed (code, emoji) pairs in picker order. Empty hides the picker only (chips
/// still render with [`NEUTRAL_GLYPH`]).
pub const KNOWN: &[KnownReaction] = &[];

/// Drawn for a code that is not in [`KNOWN`].
pub const NEUTRAL_GLYPH: &str = "❔";

pub fn emoji(code: ReactionCode) -> Option<&'static str> {
    KNOWN.iter().find(|known| known.code == code).map(|known| known.emoji)
}

impl Reactions {
    /// Applies the server's state. An update whose version is not newer than the held one is
    /// stale and ignored. Otherwise the counts are replaced, the version follows the update
    /// (kept when it has none), `mine` follows the update when it says one, and `mine` is
    /// dropped when its type has no count left. Returns whether anything changed.
    pub fn apply(&mut self, update: &ReactionsUpdate) -> bool {
        if let (Some(held), Some(new)) = (self.version, update.version) {
            if new <= held {
                return false;
            }
        }
        let mine = update
            .mine
            .unwrap_or(self.mine)
            .filter(|code| update.counts.iter().any(|count| count.code == *code));
        let version = update.version.or(self.version);
        let changed = self.counts != update.counts || self.mine != mine || self.version != version;
        if changed {
            self.counts.clone_from(&update.counts);
            self.mine = mine;
            self.version = version;
        }
        changed
    }

    /// The state the user's choice of `code` leads to, and the reaction to ask the server for
    /// (`None` removes). Choosing the own type again removes it; any other type replaces the
    /// own one. The version is kept, so a newer push overwrites the overlay.
    pub fn toggled(&self, code: ReactionCode) -> (Reactions, Option<ReactionCode>) {
        let target = if self.mine == Some(code) { None } else { Some(code) };
        let mut overlay = self.clone();
        if let Some(old) = self.mine {
            if let Some(at) = overlay.counts.iter().position(|count| count.code == old) {
                overlay.counts[at].count = overlay.counts[at].count.saturating_sub(1);
                if overlay.counts[at].count == 0 {
                    overlay.counts.remove(at);
                }
            }
        }
        if let Some(new) = target {
            match overlay.counts.iter_mut().find(|count| count.code == new) {
                Some(count) => count.count += 1,
                None => overlay.counts.push(ReactionCount { code: new, count: 1 }),
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
        pairs.iter().map(|&(c, count)| ReactionCount { code: code(c), count }).collect()
    }

    fn state(pairs: &[(i64, u32)], mine: Option<i64>, version: Option<i64>) -> Reactions {
        Reactions { counts: counts(pairs), mine: mine.map(code), version }
    }

    fn update(pairs: &[(i64, u32)], version: Option<i64>, mine: Option<Option<i64>>) -> ReactionsUpdate {
        ReactionsUpdate { counts: counts(pairs), version, mine: mine.map(|m| m.map(code)) }
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
    fn apply_takes_a_versionless_update_and_keeps_the_held_version() {
        let mut r = state(&[(1, 2)], None, Some(5));
        assert!(r.apply(&update(&[(1, 3)], None, None)));
        assert_eq!(r, state(&[(1, 3)], None, Some(5)));
        let mut unversioned = state(&[(1, 2)], None, None);
        assert!(unversioned.apply(&update(&[(1, 3)], Some(1), None)));
        assert_eq!(unversioned.version, Some(1));
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
        let mut r = state(&[(1, 1)], Some(1), Some(2));
        assert!(!r.apply(&update(&[(1, 1)], None, None)));
        assert!(!r.apply(&update(&[(1, 1)], None, Some(Some(1)))));
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
        assert_eq!((overlay, target), (state(&[(1, 3)], Some(1), None), Some(code(1))));
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
    fn emoji_finds_known_codes_only() {
        for known in KNOWN {
            assert_eq!(emoji(known.code), Some(known.emoji));
        }
        assert_eq!(emoji(code(i64::MIN)), None);
    }
}
