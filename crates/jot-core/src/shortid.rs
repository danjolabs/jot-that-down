//! Abbreviating ids to the shortest prefix that is still unambiguous.
//!
//! Git's short ids work because a SHA is random from its first bit. A note id need not be: a
//! UUIDv7 *happens* to begin with a millisecond timestamp, so notes captured in the same minute
//! share their first eight characters. jot reads nothing out of the id, but it cannot pretend the
//! ids are random either — so the width is computed the way git actually computes it: long enough
//! to be unique among the ids it is shown beside, and no longer, with a small floor for
//! readability.
//!
//! * **An abbreviation is always a genuine prefix**, so anything printed can be handed back to
//!   `Workspace::resolve`.
//! * **The width is a property of the set, not of the id.** It changes when something is added,
//!   so it is a display convenience only and never enters machine-readable output.

use std::collections::BTreeMap;

/// The conventional floor: readable, and what people expect a short id to look like.
///
/// A floor only. Uniqueness overrides it upward and never downward.
pub const MIN_WIDTH: usize = 8;

/// Map each id to the shortest prefix that no other id in `ids` shares, at least `min` long.
///
/// Duplicate ids collapse — they are one key — so a repeated id cannot force the width up.
///
/// Ids are compared as ASCII text; every id format jot accepts is ASCII.
///
/// ```
/// use jot_core::shortid;
///
/// let a = "01a03d60-1111-7000-8000-00000000000a";
/// let b = "01a03d60-1111-7000-8000-00000000000b";
///
/// // These differ only in the last character, so eight is nowhere near enough.
/// let short = shortid::abbreviate([a, b], shortid::MIN_WIDTH);
/// assert!(short[a].len() > 8);
/// assert!(a.starts_with(&short[a]));
/// assert_ne!(short[a], short[b]);
/// ```
#[must_use]
pub fn abbreviate<'a, I>(ids: I, min: usize) -> BTreeMap<String, String>
where
    I: IntoIterator<Item = &'a str>,
{
    // Sorted, so the only ids that can share a long prefix with any given one are its immediate
    // neighbours. That is what makes this a single linear pass rather than an all-pairs comparison.
    let sorted: std::collections::BTreeSet<&str> = ids.into_iter().collect();
    let text: Vec<&str> = sorted.iter().copied().collect();

    text.iter()
        .enumerate()
        .map(|(i, id)| {
            let needed = [i.checked_sub(1), (i + 1 < text.len()).then_some(i + 1)]
                .into_iter()
                .flatten()
                .map(|other| common_prefix(text[i], text[other]) + 1)
                .max()
                .unwrap_or(0);
            let width = needed.max(min).min(text[i].len());
            ((*id).to_owned(), text[i][..width].to_owned())
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

    /// Four ids that already differ at character 8.
    fn spread() -> Vec<String> {
        ["01a03d60", "01a03d61", "01a03d62", "01a03d63"]
            .iter()
            .map(|prefix| format!("{prefix}-0000-7000-8000-00000000000a"))
            .collect()
    }

    fn ab(ids: &[String], min: usize) -> BTreeMap<String, String> {
        abbreviate(ids.iter().map(String::as_str), min)
    }

    /// Four ids captured in one millisecond: identical but for the final character.
    fn burst() -> Vec<String> {
        ["a", "b", "c", "d"]
            .iter()
            .map(|tail| format!("01a03d60-1111-7000-8000-00000000000{tail}"))
            .collect()
    }

    #[test]
    fn every_abbreviation_is_a_real_prefix_of_its_id() {
        for ids in [spread(), burst()] {
            for (id, short) in ab(&ids, MIN_WIDTH) {
                assert!(id.starts_with(&short), "`{short}` is not a prefix of {id}");
            }
        }
    }

    #[test]
    fn abbreviations_are_distinct_from_one_another() {
        for ids in [spread(), burst()] {
            let short = ab(&ids, MIN_WIDTH);
            let mut values: Vec<&String> = short.values().collect();
            let before = values.len();
            values.sort();
            values.dedup();
            assert_eq!(values.len(), before, "two ids abbreviated to one string");
        }
    }

    #[test]
    fn ids_that_differ_early_stop_at_the_floor() {
        let short = ab(&spread(), MIN_WIDTH);
        assert!(short.values().all(|s| s.len() == MIN_WIDTH), "{short:?}");
    }

    #[test]
    fn ids_sharing_a_timestamp_grow_past_the_floor() {
        // The whole reason this module exists: eight characters cannot separate a burst.
        let short = ab(&burst(), MIN_WIDTH);
        assert!(short.values().all(|s| s.len() > MIN_WIDTH), "{short:?}");
    }

    #[test]
    fn the_floor_is_a_floor_and_not_a_target() {
        // With a floor of 1 these still need eight, because that is where they first differ.
        let short = ab(&spread(), 1);
        assert!(short.values().all(|s| s.len() == 8), "{short:?}");
    }

    #[test]
    fn a_lone_id_gets_the_floor_width() {
        let short = ab(&spread()[..1], MIN_WIDTH);
        assert_eq!(short.values().next().unwrap().len(), MIN_WIDTH);
    }

    #[test]
    fn an_empty_set_abbreviates_to_nothing() {
        assert!(ab(&[], MIN_WIDTH).is_empty());
    }

    #[test]
    fn a_repeated_id_is_one_key_and_does_not_force_the_width_up() {
        let id = spread()[0].clone();
        let short = ab(&[id.clone(), id.clone(), id.clone()], MIN_WIDTH);
        assert_eq!(short.len(), 1);
        assert_eq!(short[&id].len(), MIN_WIDTH);
    }

    #[test]
    fn an_abbreviation_never_ends_on_a_hyphen() {
        // Not arranged for: two ids cannot first differ at a hyphen position, because both carry a
        // hyphen there. Pinned because a trailing `-` would look like a truncation bug.
        for ids in [spread(), burst()] {
            for short in ab(&ids, MIN_WIDTH).values() {
                assert!(!short.ends_with('-'), "`{short}`");
            }
        }
    }

    #[test]
    fn a_width_beyond_the_id_is_clamped_to_the_whole_id() {
        let id = spread()[0].clone();
        let short = ab(std::slice::from_ref(&id), 999);
        assert_eq!(short[&id], id);
    }
}
