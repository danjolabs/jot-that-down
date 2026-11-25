//! Short ids: the first four characters, `...`, and the shortest tail that is still unique.
//!
//! A UUIDv7 begins with a millisecond timestamp, so notes captured close together share a long
//! prefix: a shortest-unique *prefix* grows past twelve characters exactly when notes come in
//! bursts, and the eight characters people expect tell a burst apart not at all. The randomness is
//! at the end. So a short id is the id's first [`HEAD`] characters, then [`ELLIPSIS`], then the
//! shortest suffix no other id in the workspace ends with, at least [`MIN_TAIL`] long:
//! `01a0...eefa`. The head is never needed for uniqueness; it is there so the text still reads as
//! the front of an id.
//!
//! * **Every short id resolves.** [`parse`] reads `head...tail` (and `head…tail`) back, and
//!   `Workspace::resolve` matches ids that start with the head and end with the tail. Anything
//!   without an ellipsis is a prefix, as before, so a full id always works.
//! * **The width is a property of the set, not of the id.** It changes when something is added,
//!   so it is a display convenience only and never enters machine-readable output.

use std::collections::{BTreeMap, BTreeSet};

/// How many leading characters a short id keeps.
pub const HEAD: usize = 4;

/// The floor on the tail: readable, and what a short id is expected to look like.
///
/// A floor only. Uniqueness overrides it upward and never downward.
pub const MIN_TAIL: usize = 4;

/// What separates the head from the tail when a short id is printed.
pub const ELLIPSIS: &str = "...";

/// What a user typed to name a note.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Query<'a> {
    /// No ellipsis: the id starts with this.
    Prefix(&'a str),
    /// `head...tail` or `head…tail`: the id starts with `head` and ends with `tail`. Either side
    /// may be empty, so `...eefa` names a note by its tail alone.
    Ends {
        /// What the id starts with.
        head: &'a str,
        /// What the id ends with.
        tail: &'a str,
    },
}

impl Query<'_> {
    /// Whether `id` is a note this query names. Compared as written; lowercase the query first.
    #[must_use]
    pub fn matches(&self, id: &str) -> bool {
        match *self {
            Query::Prefix(prefix) => id.starts_with(prefix),
            Query::Ends { head, tail } => {
                id.len() >= head.len() + tail.len() && id.starts_with(head) && id.ends_with(tail)
            }
        }
    }
}

/// Read what a user typed: `head...tail` or `head…tail`, else a prefix.
///
/// ```
/// use jot_core::shortid::{self, Query};
///
/// assert_eq!(shortid::parse("01a0...eefa"), Query::Ends { head: "01a0", tail: "eefa" });
/// assert_eq!(shortid::parse("01a0…eefa"), Query::Ends { head: "01a0", tail: "eefa" });
/// assert_eq!(shortid::parse("01a03d4"), Query::Prefix("01a03d4"));
/// ```
#[must_use]
pub fn parse(text: &str) -> Query<'_> {
    for ellipsis in [ELLIPSIS, "…"] {
        if let Some((head, tail)) = text.split_once(ellipsis) {
            return Query::Ends { head, tail };
        }
    }
    Query::Prefix(text)
}

/// Map each id to its short form: `head...tail`, where the tail is the shortest suffix no other
/// id in `ids` ends with, at least `min_tail` long.
///
/// An id the short form would not shorten is given whole. Duplicate ids collapse — they are one
/// key — so a repeated id cannot force the width up.
///
/// Ids are compared as ASCII text; every id format jot accepts is ASCII.
///
/// ```
/// use jot_core::shortid;
///
/// // Captured in the same millisecond: identical but for the end, which is all a short id shows.
/// let a = "01a03d60-1111-7000-8000-00000000000a";
/// let b = "01a03d60-1111-7000-8000-00000000000b";
///
/// let short = shortid::abbreviate([a, b], shortid::MIN_TAIL);
/// assert_eq!(short[a], "01a0...000a");
/// assert_eq!(short[b], "01a0...000b");
/// ```
#[must_use]
pub fn abbreviate<'a, I>(ids: I, min_tail: usize) -> BTreeMap<String, String>
where
    I: IntoIterator<Item = &'a str>,
{
    // Sorted back to front, so the only ids that can share a long suffix with any given one are
    // its immediate neighbours. That is what makes this a single linear pass rather than an
    // all-pairs comparison.
    let reversed: BTreeSet<String> = ids
        .into_iter()
        .map(|id| id.chars().rev().collect())
        .collect();
    let text: Vec<&str> = reversed.iter().map(String::as_str).collect();

    text.iter()
        .enumerate()
        .map(|(i, rev)| {
            let needed = [i.checked_sub(1), (i + 1 < text.len()).then_some(i + 1)]
                .into_iter()
                .flatten()
                .map(|other| common_prefix(rev, text[other]) + 1)
                .max()
                .unwrap_or(0);
            let id: String = rev.chars().rev().collect();
            let tail = needed.max(min_tail);
            let short = if HEAD + ELLIPSIS.len() + tail >= id.len() {
                id.clone()
            } else {
                format!("{}{ELLIPSIS}{}", &id[..HEAD], &id[id.len() - tail..])
            };
            (id, short)
        })
        .collect()
}

/// How many leading bytes two strings share. Ids are ASCII hex and hyphens, so bytes are
/// characters.
fn common_prefix(a: &str, b: &str) -> usize {
    a.bytes().zip(b.bytes()).take_while(|(x, y)| x == y).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Four ids that first differ four characters from the end.
    fn spread() -> Vec<String> {
        ["a000", "b000", "c000", "d000"]
            .iter()
            .map(|tail| format!("01a03d60-1111-7000-8000-00000000{tail}"))
            .collect()
    }

    /// Four ids captured in one millisecond: identical but for the final character.
    fn burst() -> Vec<String> {
        ["a", "b", "c", "d"]
            .iter()
            .map(|tail| format!("01a03d60-1111-7000-8000-00000000000{tail}"))
            .collect()
    }

    /// Four ids from different days that happen to end alike: they differ only up front.
    fn same_tail() -> Vec<String> {
        ["01a03d60", "01a03d61", "01a03d62", "01a03d63"]
            .iter()
            .map(|prefix| format!("{prefix}-0000-7000-8000-00000000eefa"))
            .collect()
    }

    fn ab(ids: &[String], min: usize) -> BTreeMap<String, String> {
        abbreviate(ids.iter().map(String::as_str), min)
    }

    /// The length of a short id whose tail is `tail` long.
    fn width(tail: usize) -> usize {
        HEAD + ELLIPSIS.len() + tail
    }

    #[test]
    fn a_short_id_is_the_head_an_ellipsis_and_the_tail() {
        let id = "01a03d4e-78a0-76bc-be78-8ae41b38eefa";
        let short = abbreviate([id], MIN_TAIL);
        assert_eq!(short[id], "01a0...eefa");
    }

    #[test]
    fn every_short_id_reads_back_as_a_query_naming_its_own_id() {
        for ids in [spread(), burst(), same_tail()] {
            let short = ab(&ids, MIN_TAIL);
            for (id, text) in &short {
                let query = parse(text);
                assert!(query.matches(id), "`{text}` does not name {id}");
                let others = ids.iter().filter(|other| query.matches(other)).count();
                assert_eq!(others, 1, "`{text}` names {others} ids");
            }
        }
    }

    #[test]
    fn short_ids_are_distinct_from_one_another() {
        for ids in [spread(), burst(), same_tail()] {
            let short = ab(&ids, MIN_TAIL);
            let mut values: Vec<&String> = short.values().collect();
            let before = values.len();
            values.sort();
            values.dedup();
            assert_eq!(values.len(), before, "two ids shortened to one string");
        }
    }

    #[test]
    fn a_burst_needs_no_more_than_the_floor() {
        // The reason the tail is shown: ids from one millisecond differ at the end, not the start.
        let short = ab(&burst(), MIN_TAIL);
        assert!(
            short.values().all(|s| s.len() == width(MIN_TAIL)),
            "{short:?}"
        );
    }

    #[test]
    fn ids_sharing_a_tail_grow_the_tail_past_the_floor() {
        let short = ab(&same_tail(), MIN_TAIL);
        for (id, s) in &short {
            assert!(s.len() > width(MIN_TAIL), "{short:?}");
            assert!(
                id.starts_with(&s[..HEAD]),
                "{s} does not keep the head of {id}"
            );
        }
    }

    #[test]
    fn the_floor_is_a_floor_and_not_a_target() {
        // With a floor of 1 these still need four, because that is where they first differ.
        let short = ab(&spread(), 1);
        assert!(short.values().all(|s| s.len() == width(4)), "{short:?}");
    }

    #[test]
    fn a_lone_id_gets_the_floor() {
        let short = ab(&spread()[..1], MIN_TAIL);
        assert_eq!(short.values().next().unwrap().len(), width(MIN_TAIL));
    }

    #[test]
    fn an_empty_set_shortens_to_nothing() {
        assert!(ab(&[], MIN_TAIL).is_empty());
    }

    #[test]
    fn a_repeated_id_is_one_key_and_does_not_force_the_width_up() {
        let id = spread()[0].clone();
        let short = ab(&[id.clone(), id.clone(), id.clone()], MIN_TAIL);
        assert_eq!(short.len(), 1);
        assert_eq!(short[&id].len(), width(MIN_TAIL));
    }

    #[test]
    fn a_tail_never_starts_with_a_hyphen() {
        // Not arranged for: two ids cannot first differ at a hyphen, because both carry one there.
        // Pinned because `01a0...-8000-…` would look like a slicing bug.
        for ids in [spread(), burst(), same_tail()] {
            for short in ab(&ids, MIN_TAIL).values() {
                assert!(!short.contains("...-"), "`{short}`");
            }
        }
    }

    #[test]
    fn a_short_form_that_would_not_shorten_gives_the_whole_id() {
        let id = spread()[0].clone();
        let short = ab(std::slice::from_ref(&id), 999);
        assert_eq!(short[&id], id);
    }

    #[test]
    fn a_query_without_an_ellipsis_is_a_prefix() {
        assert_eq!(parse("01a0"), Query::Prefix("01a0"));
        assert!(parse("01a0").matches("01a03d4e-78a0-76bc-be78-8ae41b38eefa"));
    }

    #[test]
    fn a_query_may_leave_either_side_of_the_ellipsis_empty() {
        let id = "01a03d4e-78a0-76bc-be78-8ae41b38eefa";
        assert!(parse("...eefa").matches(id));
        assert!(parse("01a0...").matches(id));
        assert!(!parse("...eefb").matches(id));
    }

    #[test]
    fn a_head_and_tail_longer_than_the_id_together_match_nothing() {
        // "ab...ba" must not name "aba" by letting the head and tail share its middle character.
        assert!(!parse("ab...ba").matches("aba"));
    }
}
