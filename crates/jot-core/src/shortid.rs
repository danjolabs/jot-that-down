//! Abbreviating UUIDs to the shortest prefix that is still unambiguous.
//!
//! # Why this is not "the first eight characters"
//!
//! Git's short ids work because a SHA is random from its first bit, so eight characters separate
//! anything you will ever have. **A UUIDv7's leading 48 bits are a millisecond timestamp**, and
//! eight hex characters cover only the top 32 of them — one shared value per roughly 65 seconds.
//! Randomness does not begin until character 13.
//!
//! This is not a theoretical concern. The first `jot ls` run against a real vault rendered every
//! row as the same string, `01a05a57`, and not one of those ids resolved: they were all ambiguous.
//! Notes captured in the same minute share a prefix, and notes captured in the same minute are
//! exactly the ones you refer to by short id — "jot a thought, reply to it". A surface that prints
//! an id it cannot accept back is worse than one that prints full UUIDs.
//!
//! So the width is computed the way git *actually* computes it: long enough to be unique among the
//! ids it is shown beside, and no longer. A burst of same-millisecond captures naturally produces
//! longer ids, which is honest rather than unfortunate — those ids really are that similar.
//!
//! # The two rules that follow, and that every surface inherits
//!
//! * **An abbreviation is always a genuine prefix**, so anything printed can be handed straight
//!   back to a resolver.
//! * **The width is a property of the set, not of the id.** It can change when something is added.
//!   That makes it a display convenience only: it must never be persisted, and never emitted into
//!   machine-readable output, which always carries full UUIDs.
//!
//! # Why this lives in core rather than in a surface
//!
//! Two callers already: note ids in a vault ([`Snapshot::abbreviations`](crate::snapshot::Snapshot::abbreviations))
//! and workspace ids in the registry. Both are UUIDv7 and both hit the timestamp-prefix problem
//! identically, so the rule is domain knowledge about the id scheme, not presentation. A surface
//! that reimplemented it would get the floor right and the uniqueness wrong.

use std::collections::BTreeMap;
use uuid::Uuid;

/// The conventional floor: readable, and what people expect a short id to look like.
///
/// A floor only. Uniqueness overrides it upward and never downward.
pub const MIN_WIDTH: usize = 8;

/// Map each id to the shortest prefix that no other id in `ids` shares, at least `min` long.
///
/// Duplicate ids collapse — they are one key — so a repeated id cannot force the width up.
///
/// ```
/// use jot_core::shortid;
/// use uuid::Uuid;
///
/// let a: Uuid = "01a03d60-1111-7000-8000-00000000000a".parse().unwrap();
/// let b: Uuid = "01a03d60-1111-7000-8000-00000000000b".parse().unwrap();
///
/// // These differ only in the last character, so eight is nowhere near enough.
/// let short = shortid::abbreviate([a, b], shortid::MIN_WIDTH);
/// assert!(short[&a].len() > 8);
/// assert!(a.hyphenated().to_string().starts_with(&short[&a]));
/// assert_ne!(short[&a], short[&b]);
/// ```
#[must_use]
pub fn abbreviate<I>(ids: I, min: usize) -> BTreeMap<Uuid, String>
where
    I: IntoIterator<Item = Uuid>,
{
    // Sorted, so the only ids that can share a long prefix with any given one are its immediate
    // neighbours. That is what makes this a single linear pass rather than an all-pairs comparison.
    let sorted: BTreeMap<Uuid, String> = ids
        .into_iter()
        .map(|id| (id, id.hyphenated().to_string()))
        .collect();
    let text: Vec<&String> = sorted.values().collect();

    sorted
        .keys()
        .enumerate()
        .map(|(i, id)| {
            let needed = [i.checked_sub(1), (i + 1 < text.len()).then_some(i + 1)]
                .into_iter()
                .flatten()
                .map(|other| common_prefix(text[i], text[other]) + 1)
                .max()
                .unwrap_or(0);
            let width = needed.max(min).min(text[i].len());
            (*id, text[i][..width].to_owned())
        })
        .collect()
}

/// How many leading bytes two ids share. Both are ASCII hex and hyphens, so bytes are characters.
fn common_prefix(a: &str, b: &str) -> usize {
    a.bytes().zip(b.bytes()).take_while(|(x, y)| x == y).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uuid(s: &str) -> Uuid {
        s.parse().unwrap()
    }

    /// Four ids that already differ at character 8.
    fn spread() -> Vec<Uuid> {
        ["01a03d60", "01a03d61", "01a03d62", "01a03d63"]
            .iter()
            .map(|prefix| uuid(&format!("{prefix}-0000-7000-8000-00000000000a")))
            .collect()
    }

    /// Four ids captured in one millisecond: identical but for the final character.
    fn burst() -> Vec<Uuid> {
        ["a", "b", "c", "d"]
            .iter()
            .map(|tail| uuid(&format!("01a03d60-1111-7000-8000-00000000000{tail}")))
            .collect()
    }

    #[test]
    fn every_abbreviation_is_a_real_prefix_of_its_id() {
        for ids in [spread(), burst()] {
            for (id, short) in abbreviate(ids, MIN_WIDTH) {
                assert!(
                    id.hyphenated().to_string().starts_with(&short),
                    "`{short}` is not a prefix of {id}"
                );
            }
        }
    }

    #[test]
    fn abbreviations_are_distinct_from_one_another() {
        for ids in [spread(), burst()] {
            let short = abbreviate(ids, MIN_WIDTH);
            let mut values: Vec<&String> = short.values().collect();
            let before = values.len();
            values.sort();
            values.dedup();
            assert_eq!(values.len(), before, "two ids abbreviated to one string");
        }
    }

    #[test]
    fn ids_that_differ_early_stop_at_the_floor() {
        let short = abbreviate(spread(), MIN_WIDTH);
        assert!(short.values().all(|s| s.len() == MIN_WIDTH), "{short:?}");
    }

    #[test]
    fn ids_sharing_a_timestamp_grow_past_the_floor() {
        // The whole reason this module exists: eight characters cannot separate a burst.
        let short = abbreviate(burst(), MIN_WIDTH);
        assert!(short.values().all(|s| s.len() > MIN_WIDTH), "{short:?}");
    }

    #[test]
    fn the_floor_is_a_floor_and_not_a_target() {
        // With a floor of 1 these still need eight, because that is where they first differ.
        let short = abbreviate(spread(), 1);
        assert!(short.values().all(|s| s.len() == 8), "{short:?}");
    }

    #[test]
    fn a_lone_id_gets_the_floor_width() {
        let short = abbreviate(spread().into_iter().take(1), MIN_WIDTH);
        assert_eq!(short.values().next().unwrap().len(), MIN_WIDTH);
    }

    #[test]
    fn an_empty_set_abbreviates_to_nothing() {
        assert!(abbreviate([], MIN_WIDTH).is_empty());
    }

    #[test]
    fn a_repeated_id_is_one_key_and_does_not_force_the_width_up() {
        let id = spread()[0];
        let short = abbreviate([id, id, id], MIN_WIDTH);
        assert_eq!(short.len(), 1);
        assert_eq!(short[&id].len(), MIN_WIDTH);
    }

    #[test]
    fn an_abbreviation_never_ends_on_a_hyphen() {
        // Not arranged for: two ids cannot first differ at a hyphen position, because both carry a
        // hyphen there. Pinned because a trailing `-` would look like a truncation bug.
        for ids in [spread(), burst()] {
            for short in abbreviate(ids, MIN_WIDTH).values() {
                assert!(!short.ends_with('-'), "`{short}`");
            }
        }
    }

    #[test]
    fn a_width_beyond_the_id_is_clamped_to_the_whole_id() {
        let id = spread()[0];
        let short = abbreviate([id], 999);
        assert_eq!(short[&id], id.hyphenated().to_string());
    }
}
