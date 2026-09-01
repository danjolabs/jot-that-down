//! Thread algebra: the two projections a stored adjacency list supports.
//!
//! Storage is one edge per note — `relation:reply_to`, plus the denormalized `relation:root` that
//! makes a whole thread one lookup. Neither shape below is persisted, and that is the point: a
//! thread is tens of nodes, both forms fall out of a single in-memory assembly, and anything
//! *stored* is a second copy of the truth that can drift from the files.
//!
//! # The two forms
//!
//! Using `docs/plans/stage3.md`'s worked example — edges `A→B`, `B→C`, `C→E`, `C→D`, `A→F`:
//!
//! ```text
//!   B - C - E          paths     (A,B,C,D), (A,B,C,E), (A,F)
//!  /     \             segments  (A,B,C), (A,F), (C,D), (C,E)
//! A       D
//!  \
//!   F
//! ```
//!
//! * **Paths** ([`TreeNode::paths`]) — every root-to-leaf line. One per leaf. This is "read one
//!   branch end to end".
//! * **Segments** ([`TreeNode::segments`]) — chains that begin at the root or at a branch point and
//!   run until the next branch point or leaf. This is the micro-blog reading order: a conversation
//!   is a run of single replies, and a fork is where it becomes two.
//!
//! # Sibling order
//!
//! Creation order, read straight off the UUIDv7 ids. There is no `position` column and no ordering
//! metadata, so there is nothing to keep consistent and nothing a hand-edit can corrupt.
//!
//! # Cycles
//!
//! `relation:reply_to` is a line in a text file, so `A→B→A` is a normal input rather than
//! corruption. Every walk here is bounded by a visited set: a cycle truncates the structure at the
//! repeat instead of hanging. The *diagnosable* error for a cycle belongs to the caller that walked
//! the files ([`Error::ReplyCycle`](crate::error::Error::ReplyCycle)); by the time a tree is being
//! assembled in memory the job is only to terminate.

use crate::note::{NoteId, NoteMeta};
use std::collections::{BTreeMap, HashSet};

/// A thread, loaded once and projected many times.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thread {
    /// The note the thread was asked about.
    pub focus: NoteId,
    /// Root → parent of the focus. Always linear, and empty when the focus is a root.
    pub ancestors: Vec<NoteMeta>,
    /// The focus and everything beneath it.
    pub tree: TreeNode,
}

impl Thread {
    /// The root of the thread: the first ancestor, or the focus when there is none.
    #[must_use]
    pub fn root(&self) -> &NoteMeta {
        self.ancestors.first().unwrap_or(&self.tree.note)
    }

    /// The single line from the thread root down to the focus — stage 4's `--path`.
    ///
    /// Ancestors are linear, so this needs no search: it is the ancestor chain with the focus
    /// appended.
    #[must_use]
    pub fn path_to_focus(&self) -> Vec<NoteId> {
        self.ancestors
            .iter()
            .map(|meta| meta.id)
            .chain(std::iter::once(self.focus))
            .collect()
    }
}

/// One node of a thread tree: a note and its replies, in creation order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeNode {
    /// This node's note.
    pub note: NoteMeta,
    /// Direct replies, sorted by id and therefore by creation time.
    pub children: Vec<TreeNode>,
}

impl TreeNode {
    /// A childless node.
    #[must_use]
    pub fn leaf(note: NoteMeta) -> Self {
        TreeNode {
            note,
            children: Vec::new(),
        }
    }

    /// Assemble the subtree rooted at `focus` from an adjacency map of parent id → replies.
    ///
    /// `children_of` is built once from a flat set of notes — one `SELECT … WHERE root_id = ?` in
    /// stage 2's terms, one filter over the vault snapshot in stage 3's — so assembling a whole
    /// thread costs one pass over its notes, not one lookup per node.
    ///
    /// A node reached twice is not descended into a second time, which is what makes a hand-written
    /// cycle terminate here rather than hang.
    #[must_use]
    pub fn assemble(focus: NoteMeta, children_of: &BTreeMap<NoteId, Vec<NoteMeta>>) -> TreeNode {
        let mut seen = HashSet::new();
        seen.insert(focus.id);
        descend(focus, children_of, &mut seen)
    }

    /// This node's id.
    #[must_use]
    pub fn id(&self) -> NoteId {
        self.note.id
    }

    /// The number of notes in this subtree, the node itself included. Never zero.
    #[must_use]
    pub fn len(&self) -> usize {
        1 + self.children.iter().map(TreeNode::len).sum::<usize>()
    }

    /// Always `false` — a tree node is its own note, so a subtree is never empty.
    ///
    /// Present only because clippy asks for it beside [`TreeNode::len`]; a caller wanting "has no
    /// replies" wants `children.is_empty()`.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        false
    }

    /// The number of direct replies.
    #[must_use]
    pub fn reply_count(&self) -> usize {
        self.children.len()
    }

    /// Every node in the subtree, depth-first, parents before children.
    pub fn iter(&self) -> impl Iterator<Item = &TreeNode> {
        let mut stack = vec![self];
        std::iter::from_fn(move || {
            let node = stack.pop()?;
            // Reversed so the depth-first order matches sibling order rather than inverting it.
            stack.extend(node.children.iter().rev());
            Some(node)
        })
    }

    /// **Form 1** — every path from this node to a leaf beneath it.
    ///
    /// One path per leaf, each starting at this node. A childless node yields exactly one path
    /// holding only itself, which keeps "the number of paths equals the number of leaves" true
    /// without a special case.
    #[must_use]
    pub fn paths(&self) -> Vec<Vec<NoteId>> {
        let mut out = Vec::new();
        let mut prefix = Vec::new();
        collect_paths(self, &mut prefix, &mut out);
        out
    }

    /// **Form 2** — the subtree cut into chains at its branch points.
    ///
    /// A segment starts at this node or at a node with more than one reply, and runs down through
    /// single replies until it reaches a leaf or the next branch point, which it includes as its
    /// last element. A childless node has no segments at all: segments describe edges, and it has
    /// none.
    #[must_use]
    pub fn segments(&self) -> Vec<Segment> {
        let mut out = Vec::new();
        let mut branches = vec![self];
        while let Some(branch) = branches.pop() {
            // Reversed for the same reason `iter` reverses: the branch points discovered while
            // walking this node's children must be visited in sibling order, and `branches` is a
            // stack.
            let mut discovered = Vec::new();
            for child in &branch.children {
                let mut nodes = vec![branch.id(), child.id()];
                let mut tail = child;
                while let [only] = tail.children.as_slice() {
                    tail = only;
                    nodes.push(tail.id());
                }
                out.push(Segment { nodes });
                if tail.children.len() > 1 {
                    discovered.push(tail);
                }
            }
            branches.extend(discovered.into_iter().rev());
        }
        out
    }
}

impl<'a> IntoIterator for &'a TreeNode {
    type Item = &'a TreeNode;
    type IntoIter = Box<dyn Iterator<Item = &'a TreeNode> + 'a>;

    fn into_iter(self) -> Self::IntoIter {
        Box::new(self.iter())
    }
}

/// A chain of notes from a branch point (or the tree's own root) to the next one.
///
/// Always at least two ids: a segment is a run of edges, so there is no such thing as a segment of
/// one note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    /// The chain, in reply order. `nodes[0]` is the anchor the segment hangs from.
    pub nodes: Vec<NoteId>,
}

impl Segment {
    /// The branch point this segment hangs from — the only node in it shared with another segment.
    #[must_use]
    pub fn anchor(&self) -> NoteId {
        self.nodes[0]
    }

    /// The segment's own notes: everything after the anchor. Never empty.
    #[must_use]
    pub fn body(&self) -> &[NoteId] {
        &self.nodes[1..]
    }

    /// The number of edges this segment covers.
    #[must_use]
    pub fn edges(&self) -> usize {
        self.nodes.len() - 1
    }
}

/// Recursive half of [`TreeNode::assemble`].
fn descend(
    note: NoteMeta,
    children_of: &BTreeMap<NoteId, Vec<NoteMeta>>,
    seen: &mut HashSet<NoteId>,
) -> TreeNode {
    let children = children_of
        .get(&note.id)
        .into_iter()
        .flatten()
        .filter(|child| seen.insert(child.id))
        .cloned()
        .collect::<Vec<_>>()
        .into_iter()
        .map(|child| descend(child, children_of, seen))
        .collect();
    TreeNode { note, children }
}

/// Recursive half of [`TreeNode::paths`].
fn collect_paths(node: &TreeNode, prefix: &mut Vec<NoteId>, out: &mut Vec<Vec<NoteId>>) {
    prefix.push(node.id());
    if node.children.is_empty() {
        out.push(prefix.clone());
    } else {
        for child in &node.children {
            collect_paths(child, prefix, out);
        }
    }
    prefix.pop();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ids whose hex spells the letter in the worked example, so a failure prints something a
    /// human can read against the diagram.
    fn nid(letter: char) -> NoteId {
        let n = letter as u8 - b'a';
        format!("01a03d6{n:x}-0000-7000-8000-00000000000{n:x}")
            .parse()
            .unwrap()
    }

    fn meta(letter: char, reply_to: Option<char>) -> NoteMeta {
        let id = nid(letter);
        NoteMeta {
            id,
            created_at: id.created_at(),
            title: Some(letter.to_string()),
            root: Some(nid('a')),
            reply_to: reply_to.map(nid),
            quote: None,
        }
    }

    /// Build a tree from `(child, parent)` edges, rooted at `root`.
    fn tree(root: char, edges: &[(char, char)]) -> TreeNode {
        let mut children_of: BTreeMap<NoteId, Vec<NoteMeta>> = BTreeMap::new();
        for &(child, parent) in edges {
            children_of
                .entry(nid(parent))
                .or_default()
                .push(meta(child, Some(parent)));
        }
        for replies in children_of.values_mut() {
            replies.sort_by_key(|m| m.id);
        }
        TreeNode::assemble(meta(root, None), &children_of)
    }

    /// The worked example from `stage3.md`: `A→B`, `B→C`, `C→E`, `C→D`, `A→F`.
    fn worked_example() -> TreeNode {
        tree(
            'a',
            &[('b', 'a'), ('c', 'b'), ('e', 'c'), ('d', 'c'), ('f', 'a')],
        )
    }

    fn spell(ids: &[NoteId]) -> String {
        ids.iter()
            .map(|id| {
                ('a'..='z')
                    .find(|&c| nid(c) == *id)
                    .expect("id from the fixture alphabet")
            })
            .collect()
    }

    fn spelled_paths(node: &TreeNode) -> Vec<String> {
        node.paths().iter().map(|p| spell(p)).collect()
    }

    fn spelled_segments(node: &TreeNode) -> Vec<String> {
        node.segments().iter().map(|s| spell(&s.nodes)).collect()
    }

    // ------------------------------------------------------------------- the worked example

    #[test]
    fn the_worked_example_projects_to_the_paths_the_plan_documents() {
        assert_eq!(spelled_paths(&worked_example()), ["abcd", "abce", "af"]);
    }

    #[test]
    fn the_worked_example_projects_to_the_segments_the_plan_documents() {
        assert_eq!(
            spelled_segments(&worked_example()),
            ["abc", "af", "cd", "ce"]
        );
    }

    #[test]
    fn the_worked_example_has_the_shape_the_diagram_draws() {
        let tree = worked_example();
        assert_eq!(tree.len(), 6);
        assert_eq!(spell(&[tree.id()]), "a");
        assert_eq!(tree.reply_count(), 2);
    }

    // ------------------------------------------------------------------------ sibling order

    #[test]
    fn siblings_come_back_in_creation_order_whatever_order_they_were_supplied_in() {
        // `e` is inserted before `d` in the worked example, and comes back after it.
        let tree = worked_example();
        let c = tree.iter().find(|n| spell(&[n.id()]) == "c").unwrap();
        assert_eq!(
            c.children
                .iter()
                .map(|n| spell(&[n.id()]))
                .collect::<Vec<_>>(),
            ["d", "e"]
        );
    }

    #[test]
    fn depth_first_iteration_visits_parents_before_children_and_siblings_in_order() {
        let visited: String = worked_example()
            .iter()
            .map(|n| spell(&[n.id()]))
            .collect::<Vec<_>>()
            .concat();
        assert_eq!(visited, "abcdef");
    }

    // --------------------------------------------------------------------- degenerate shapes

    #[test]
    fn a_lone_note_is_one_path_of_itself_and_no_segments() {
        let tree = tree('a', &[]);
        assert_eq!(spelled_paths(&tree), ["a"]);
        assert!(tree.segments().is_empty());
        assert_eq!(tree.len(), 1);
    }

    #[test]
    fn an_unbranched_chain_is_one_path_and_one_segment() {
        let tree = tree('a', &[('b', 'a'), ('c', 'b'), ('d', 'c')]);
        assert_eq!(spelled_paths(&tree), ["abcd"]);
        assert_eq!(spelled_segments(&tree), ["abcd"]);
    }

    #[test]
    fn a_root_that_forks_immediately_is_two_segments_of_one_edge_each() {
        let tree = tree('a', &[('b', 'a'), ('c', 'a')]);
        assert_eq!(spelled_segments(&tree), ["ab", "ac"]);
        assert_eq!(spelled_paths(&tree), ["ab", "ac"]);
    }

    #[test]
    fn a_three_way_fork_is_three_segments_not_a_binary_split() {
        let tree = tree('a', &[('b', 'a'), ('c', 'a'), ('d', 'a')]);
        assert_eq!(spelled_segments(&tree), ["ab", "ac", "ad"]);
    }

    #[test]
    fn nested_branch_points_each_start_their_own_segments() {
        //        b - c - d
        //       /     \
        //      a       e
        //       \
        //        f - g
        //             \
        //              h
        let tree = tree(
            'a',
            &[
                ('b', 'a'),
                ('c', 'b'),
                ('d', 'c'),
                ('e', 'c'),
                ('f', 'a'),
                ('g', 'f'),
                ('h', 'g'),
            ],
        );
        assert_eq!(spelled_segments(&tree), ["abc", "afgh", "cd", "ce"]);
    }

    // ------------------------------------------------------------------------ the invariants
    //
    // `stage3.md` lists these as properties to generate trees against. They are asserted here
    // over a fixed corpus of shapes; `assert_invariants` is the body a generator would call.

    fn shapes() -> Vec<TreeNode> {
        vec![
            tree('a', &[]),
            tree('a', &[('b', 'a')]),
            tree('a', &[('b', 'a'), ('c', 'b'), ('d', 'c')]),
            tree('a', &[('b', 'a'), ('c', 'a')]),
            tree('a', &[('b', 'a'), ('c', 'a'), ('d', 'a')]),
            worked_example(),
            tree(
                'a',
                &[
                    ('b', 'a'),
                    ('c', 'b'),
                    ('d', 'c'),
                    ('e', 'c'),
                    ('f', 'a'),
                    ('g', 'f'),
                    ('h', 'g'),
                ],
            ),
            tree(
                'a',
                &[
                    ('b', 'a'),
                    ('c', 'b'),
                    ('d', 'b'),
                    ('e', 'd'),
                    ('f', 'd'),
                    ('g', 'f'),
                ],
            ),
        ]
    }

    fn leaves(node: &TreeNode) -> usize {
        node.iter().filter(|n| n.children.is_empty()).count()
    }

    fn branch_points(node: &TreeNode) -> Vec<&TreeNode> {
        node.iter().filter(|n| n.children.len() > 1).collect()
    }

    #[test]
    fn segments_partition_the_edges() {
        for tree in shapes() {
            let segments = tree.segments();
            let total: usize = segments.iter().map(Segment::edges).sum();
            assert_eq!(total, tree.len() - 1, "edge count");

            // Every edge exactly once, not merely the right number of them.
            let mut edges: Vec<(NoteId, NoteId)> = segments
                .iter()
                .flat_map(|s| s.nodes.windows(2).map(|w| (w[0], w[1])))
                .collect();
            let before = edges.len();
            edges.sort();
            edges.dedup();
            assert_eq!(edges.len(), before, "an edge appeared in two segments");
        }
    }

    #[test]
    fn segment_count_is_the_root_and_every_branch_points_children() {
        for tree in shapes() {
            let mut expected = tree.children.len();
            for branch in branch_points(&tree) {
                // The root is already counted; counting it again here would double it.
                if branch.id() != tree.id() {
                    expected += branch.children.len();
                }
            }
            assert_eq!(tree.segments().len(), expected);
        }
    }

    #[test]
    fn segments_cover_every_node_exactly_once_as_a_non_first_element() {
        for tree in shapes() {
            let mut covered: Vec<NoteId> = tree
                .segments()
                .iter()
                .flat_map(|s| s.body().to_vec())
                .collect();
            let before = covered.len();
            covered.sort();
            covered.dedup();
            assert_eq!(covered.len(), before, "a node was covered twice");
            assert_eq!(covered.len(), tree.len() - 1, "a node was not covered");
            assert!(!covered.contains(&tree.id()), "the root was covered");
        }
    }

    #[test]
    fn paths_cover_every_node_and_there_is_one_per_leaf() {
        for tree in shapes() {
            let paths = tree.paths();
            assert_eq!(paths.len(), leaves(&tree), "one path per leaf");

            let mut seen: Vec<NoteId> = paths.iter().flatten().copied().collect();
            seen.sort();
            seen.dedup();
            assert_eq!(seen.len(), tree.len(), "a node appeared in no path");
        }
    }

    #[test]
    fn every_path_starts_at_the_root_and_ends_at_a_leaf() {
        for tree in shapes() {
            let leaf_ids: Vec<NoteId> = tree
                .iter()
                .filter(|n| n.children.is_empty())
                .map(TreeNode::id)
                .collect();
            for path in tree.paths() {
                assert_eq!(path.first(), Some(&tree.id()));
                assert!(leaf_ids.contains(path.last().unwrap()));
            }
        }
    }

    #[test]
    fn every_segment_holds_at_least_one_edge() {
        for tree in shapes() {
            for segment in tree.segments() {
                assert!(segment.nodes.len() >= 2);
                assert_eq!(segment.body().len(), segment.edges());
            }
        }
    }

    // ---------------------------------------------------------------------------- cycles

    #[test]
    fn a_cycle_in_the_adjacency_map_truncates_rather_than_hanging() {
        // `a → b → a`: a hand-edited pair of files can say this, and assembling it must terminate.
        let mut children_of: BTreeMap<NoteId, Vec<NoteMeta>> = BTreeMap::new();
        children_of.insert(nid('a'), vec![meta('b', Some('a'))]);
        children_of.insert(nid('b'), vec![meta('a', Some('b'))]);

        let tree = TreeNode::assemble(meta('a', None), &children_of);
        assert_eq!(spelled_paths(&tree), ["ab"]);
        assert_eq!(tree.len(), 2);
    }

    #[test]
    fn a_note_that_replies_to_itself_gains_no_children() {
        let mut children_of: BTreeMap<NoteId, Vec<NoteMeta>> = BTreeMap::new();
        children_of.insert(nid('a'), vec![meta('a', Some('a'))]);

        let tree = TreeNode::assemble(meta('a', None), &children_of);
        assert_eq!(tree.len(), 1);
        assert!(tree.children.is_empty());
    }

    #[test]
    fn a_longer_cycle_also_terminates() {
        let mut children_of: BTreeMap<NoteId, Vec<NoteMeta>> = BTreeMap::new();
        children_of.insert(nid('a'), vec![meta('b', Some('a'))]);
        children_of.insert(nid('b'), vec![meta('c', Some('b'))]);
        children_of.insert(nid('c'), vec![meta('a', Some('c'))]);

        let tree = TreeNode::assemble(meta('a', None), &children_of);
        assert_eq!(spelled_paths(&tree), ["abc"]);
    }

    // ---------------------------------------------------------------------------- Thread

    #[test]
    fn a_threads_root_is_its_first_ancestor_and_the_focus_when_it_has_none() {
        let rooted = Thread {
            focus: nid('c'),
            ancestors: vec![meta('a', None), meta('b', Some('a'))],
            tree: TreeNode::leaf(meta('c', Some('b'))),
        };
        assert_eq!(rooted.root().id, nid('a'));
        assert_eq!(spell(&rooted.path_to_focus()), "abc");

        let orphan = Thread {
            focus: nid('a'),
            ancestors: Vec::new(),
            tree: TreeNode::leaf(meta('a', None)),
        };
        assert_eq!(orphan.root().id, nid('a'));
        assert_eq!(spell(&orphan.path_to_focus()), "a");
    }
}
