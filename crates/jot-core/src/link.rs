//! `[[uuid]]` link extraction from a note body.
//!
//! Stage 3's links, and the second use of the markdown parser stage 1b took on. The rule is the
//! same one the frontmatter splitter follows: **parse to an AST, read byte offsets, never call the
//! renderer.** Nothing here reconstructs markdown; the body is only ever sliced.
//!
//! # Why an AST and not a regex
//!
//! A regex over the raw body cannot tell prose from code, and `` `[[uuid]]` `` in a sentence about
//! linking is a person writing *about* a link, not making one. Walking the mdast makes that
//! distinction free: [`Node::Code`] and [`Node::InlineCode`] are simply not descended into.
//! Excluding inline code is a **decision**, not a side effect — see
//! `inline_code_is_excluded_and_that_is_a_decision`.
//!
//! # Why the source is sliced rather than the text node's value read
//!
//! markdown-rs decodes character references into [`Node::Text`]'s `value`, so `&amp;` arrives as
//! one byte where the source spent five. A link's offset is wanted for stage 5's reader, which
//! highlights it *in the file*, so the value's offsets would be the wrong ones. Slicing the body
//! with the node's span sidesteps the decoding entirely and gives offsets that index the bytes on
//! disk.
//!
//! It also makes escaping work with no code: `\[\[uuid]]` reaches this module as the six characters
//! `\ [ \ [ u …`, in which `[[` never appears, so an escaped link is not a link and nothing had to
//! check for one.

use crate::note::NoteId;
use markdown::mdast::Node;
use std::ops::Range;
use uuid::Uuid;

/// The opening delimiter. Its length is also how far past a match the scan resumes.
const OPEN: &str = "[[";

/// The closing delimiter.
const CLOSE: &str = "]]";

/// The separator between a link's target and its label in `[[uuid|label]]`.
const LABEL_SEPARATOR: char = '|';

/// One `[[uuid]]` or `[[uuid|label]]` found in a body.
///
/// Extraction is **purely textual** and never consults the index: a link to a purged note extracts
/// exactly like a link to a live one, and resolving it to
/// [`Present`](crate::query::Ref::Present) / [`Trashed`](crate::query::Ref::Trashed) /
/// [`Deleted`](crate::query::Ref::Deleted) is a separate step. That separation is what lets a
/// rebuild reproduce the `links` rows from the files alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    /// The note this link points at. Not known to exist — see the type docs.
    pub target: NoteId,
    /// The display text after `|`, if the link carried one. Never empty: `[[uuid|]]` is a link
    /// with no label, not a link with an empty one.
    pub label: Option<String>,
    /// Byte range of the whole `[[…]]` within the body it was extracted from, delimiters included.
    ///
    /// Kept because stage 5's reader highlights links in place, and recovering this later means
    /// re-parsing the note.
    pub span: Range<usize>,
}

/// Every link in `body`, in the order they appear.
///
/// Duplicates are **kept**: the same target linked twice in one body yields two [`Link`]s with
/// different spans, because each is a real position in the text. Collapsing them to one edge is
/// the caller's job and belongs where the edge set is built, not where the text is read.
#[must_use]
pub fn extract(body: &str) -> Vec<Link> {
    // `to_mdast` returns `Err` only for MDX syntax, which the default constructs do not enable.
    // A body that somehow fails to parse has no links this module is willing to claim.
    let Ok(tree) = markdown::to_mdast(body, &markdown::ParseOptions::default()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    walk(&tree, body, &mut out);
    out.sort_by_key(|link| link.span.start);
    out
}

/// Descend into `node`, scanning the source behind every text node and refusing to enter code.
fn walk(node: &Node, body: &str, out: &mut Vec<Link>) {
    match node {
        // Fenced and indented code, inline code, and raw HTML are all "not prose". A link written
        // inside any of them is being shown, not made.
        Node::Code(_) | Node::InlineCode(_) | Node::Html(_) => return,
        Node::Text(_) => {
            if let Some(position) = node.position() {
                scan(body, position.start.offset..position.end.offset, out);
            }
            return;
        }
        _ => {}
    }
    if let Some(children) = node.children() {
        for child in children {
            walk(child, body, out);
        }
    }
}

/// Find every well-formed link in `body[span]`, reporting spans absolute to `body`.
fn scan(body: &str, span: Range<usize>, out: &mut Vec<Link>) {
    let Some(text) = body.get(span.clone()) else {
        // A span that is not a char boundary cannot come from the parser; nothing to salvage.
        return;
    };
    let mut cursor = 0;
    while let Some(open) = text[cursor..].find(OPEN) {
        let open = cursor + open;
        let interior_start = open + OPEN.len();

        // A link never spans a line. Bounding the search at the next newline keeps an unclosed
        // `[[` from swallowing the rest of the paragraph looking for a `]]` that belongs to a
        // later, unrelated link.
        let rest = &text[interior_start..];
        let bound = rest.find('\n').unwrap_or(rest.len());
        let Some(close) = rest[..bound].find(CLOSE) else {
            cursor = interior_start;
            continue;
        };

        let interior = &rest[..close];
        if let Some(link) = parse_interior(interior) {
            let end = interior_start + close + CLOSE.len();
            out.push(Link {
                span: span.start + open..span.start + end,
                ..link
            });
            cursor = interior_start + close + CLOSE.len();
        } else {
            // Not a link — `[[not a uuid]]`, or a wiki-link to a title. Resume *inside* the
            // opening delimiter rather than past the whole span, so `[[[[uuid]]` still finds the
            // link its second `[[` opens.
            cursor = interior_start;
        }
    }
}

/// Turn the text between the delimiters into a link, or `None` when it does not name a UUID.
///
/// The target must be a UUID and nothing else. jot links by identity, so `[[some title]]` is not a
/// link with an unresolvable target — it is not a link at all, and quietly dropping it is right:
/// the syntax belongs to other tools and a body may have been pasted from one.
fn parse_interior(interior: &str) -> Option<Link> {
    let (target, label) = match interior.split_once(LABEL_SEPARATOR) {
        Some((target, label)) => (target, Some(label)),
        None => (interior, None),
    };
    let target = NoteId::from(Uuid::parse_str(target.trim()).ok()?);
    let label = label
        .map(str::trim)
        .filter(|label| !label.is_empty())
        .map(str::to_owned);
    Some(Link {
        target,
        label,
        // Rewritten by the caller, which is the only place that knows the absolute offset.
        span: 0..0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "01a03d60-0000-7000-8000-00000000000a";
    const B: &str = "01a03d61-0000-7000-8000-00000000000b";

    fn nid(s: &str) -> NoteId {
        s.parse().unwrap()
    }

    fn targets(body: &str) -> Vec<NoteId> {
        extract(body).into_iter().map(|l| l.target).collect()
    }

    // --------------------------------------------------------------------------- the basic forms

    #[test]
    fn a_bare_link_yields_its_target_and_no_label() {
        let links = extract(&format!("see [[{A}]] for more"));
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, nid(A));
        assert_eq!(links[0].label, None);
    }

    #[test]
    fn a_labelled_link_keeps_its_label_trimmed() {
        let links = extract(&format!("see [[{A}| the first note ]]"));
        assert_eq!(links[0].target, nid(A));
        assert_eq!(links[0].label.as_deref(), Some("the first note"));
    }

    #[test]
    fn an_empty_label_is_no_label_rather_than_an_empty_one() {
        let links = extract(&format!("[[{A}|]] and [[{A}|   ]]"));
        assert_eq!(links.len(), 2);
        assert!(links.iter().all(|l| l.label.is_none()));
    }

    #[test]
    fn a_label_may_contain_anything_but_a_newline_and_the_closing_fence() {
        let links = extract(&format!("[[{A}|a | b [c] d]]"));
        assert_eq!(links[0].label.as_deref(), Some("a | b [c] d"));
    }

    #[test]
    fn several_links_come_back_in_source_order() {
        assert_eq!(
            targets(&format!("[[{B}]] then [[{A}]]")),
            vec![nid(B), nid(A)]
        );
    }

    #[test]
    fn the_same_target_twice_is_two_links_because_each_is_a_real_position() {
        let links = extract(&format!("[[{A}]] and again [[{A}]]"));
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].target, links[1].target);
        assert_ne!(links[0].span, links[1].span);
    }

    // ------------------------------------------------------------------------------- the offsets

    #[test]
    fn the_span_covers_the_delimiters_and_slices_back_to_the_link() {
        let body = format!("prefix [[{A}]] suffix");
        let links = extract(&body);
        assert_eq!(&body[links[0].span.clone()], format!("[[{A}]]"));
    }

    #[test]
    fn a_span_is_absolute_to_the_body_not_to_the_paragraph_it_sits_in() {
        let body = format!("first paragraph\n\nsecond, with [[{A}]] in it");
        let links = extract(&body);
        assert_eq!(&body[links[0].span.clone()], format!("[[{A}]]"));
        assert!(links[0].span.start > body.find("second").unwrap());
    }

    #[test]
    fn a_span_survives_multibyte_text_before_it() {
        let body = format!("한국어 문장 안의 [[{A}|링크]] 하나");
        let links = extract(&body);
        assert_eq!(&body[links[0].span.clone()], format!("[[{A}|링크]]"));
    }

    #[test]
    fn a_span_is_measured_in_source_bytes_not_decoded_ones() {
        // `&amp;` is five source bytes and one decoded character. Reading the text node's value
        // instead of the source would put this link four bytes early.
        let body = format!("a &amp; b [[{A}]]");
        let links = extract(&body);
        assert_eq!(&body[links[0].span.clone()], format!("[[{A}]]"));
    }

    // -------------------------------------------------------------------------- what is not prose

    #[test]
    fn a_link_in_a_fenced_code_block_is_not_a_link() {
        assert!(targets(&format!("```\n[[{A}]]\n```")).is_empty());
    }

    #[test]
    fn a_link_in_an_indented_code_block_is_not_a_link() {
        assert!(targets(&format!("    [[{A}]]")).is_empty());
    }

    #[test]
    fn inline_code_is_excluded_and_that_is_a_decision() {
        // Not a side effect of the walk: someone writing `` `[[uuid]]` `` is documenting the
        // syntax, and picking that up would make every page of jot's own docs a linking note.
        assert!(targets(&format!("the syntax is `[[{A}]]`")).is_empty());
    }

    #[test]
    fn a_link_in_raw_html_is_not_a_link() {
        assert!(targets(&format!("<div>[[{A}]]</div>")).is_empty());
    }

    #[test]
    fn the_same_uuid_in_prose_in_a_fence_and_in_inline_code_yields_exactly_one_link() {
        let body = format!(
            "prose [[{A}]]\n\n```\n[[{A}]]\n```\n\nand `[[{A}]]`\n",
            A = A
        );
        assert_eq!(targets(&body), vec![nid(A)]);
    }

    // ------------------------------------------------------------------------- what is not a link

    #[test]
    fn a_target_that_is_not_a_uuid_is_not_a_link_at_all() {
        assert!(targets("[[some page title]]").is_empty());
        assert!(targets("[[not-a-uuid|label]]").is_empty());
    }

    #[test]
    fn an_unclosed_link_is_not_a_link() {
        assert!(targets(&format!("[[{A}")).is_empty());
    }

    #[test]
    fn a_link_never_spans_a_line() {
        assert!(targets(&format!("[[{A}\n]]")).is_empty());
    }

    #[test]
    fn an_unclosed_opener_does_not_swallow_a_later_link_on_the_same_line() {
        assert_eq!(targets(&format!("[[oops [[{A}]]")), vec![nid(A)]);
    }

    #[test]
    fn an_escaped_link_is_not_a_link() {
        // Nothing checks for this: the escapes are still in the source slice, so `[[` never
        // appears. The test pins the behavior, not an implementation.
        assert!(targets(&format!("\\[\\[{A}]]")).is_empty());
    }

    #[test]
    fn a_single_bracket_pair_is_a_markdown_link_and_not_ours() {
        assert!(targets(&format!("[{A}](somewhere)")).is_empty());
    }

    #[test]
    fn an_empty_body_and_a_body_with_no_links_both_yield_nothing() {
        assert!(extract("").is_empty());
        assert!(extract("just some prose, no links").is_empty());
    }

    #[test]
    fn a_uuid_in_prose_without_delimiters_is_not_a_link() {
        assert!(targets(&format!("the id is {A}")).is_empty());
    }

    // ------------------------------------------------------------------------- structural bodies

    #[test]
    fn links_are_found_inside_lists_quotes_headings_and_emphasis() {
        let bodies = [
            format!("- [[{A}]]"),
            format!("> [[{A}]]"),
            format!("# [[{A}]]"),
            format!("*[[{A}]]*"),
            format!("| a | [[{A}]] |"),
        ];
        for body in bodies {
            assert_eq!(targets(&body), vec![nid(A)], "body: {body}");
        }
    }

    #[test]
    fn a_link_inside_a_markdown_link_label_is_still_extracted() {
        // The text node is nested under a Link node; the walk descends into it like any other
        // container. Whether this is *useful* is a rendering question, not an extraction one.
        assert_eq!(targets(&format!("[[[{A}]]](target)")), vec![nid(A)]);
    }
}
