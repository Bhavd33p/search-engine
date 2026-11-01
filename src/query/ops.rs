//! Set operations on sorted doc-id lists, and phrase matching on positions.
//!
//! Posting lists are sorted by doc id, so AND / OR / NOT are linear merges —
//! the same idea as the merge step of merge sort.

use crate::index::postings::PostingList;

/// Docs in both lists (AND).
pub fn intersect(a: &[u32], b: &[u32]) -> Vec<u32> {
    let (small, large) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    // When one list is much shorter, binary-searching its ids in the long list
    // (O(s log l)) beats walking both (O(s + l)).
    if small.len() * 16 < large.len() {
        return small
            .iter()
            .copied()
            .filter(|id| large.binary_search(id).is_ok())
            .collect();
    }
    let mut out = Vec::with_capacity(small.len());
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                out.push(a[i]);
                i += 1;
                j += 1;
            }
        }
    }
    out
}

/// Docs in either list (OR).
pub fn union(a: &[u32], b: &[u32]) -> Vec<u32> {
    let mut out = Vec::with_capacity(a.len() + b.len());
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Less => {
                out.push(a[i]);
                i += 1;
            }
            std::cmp::Ordering::Greater => {
                out.push(b[j]);
                j += 1;
            }
            std::cmp::Ordering::Equal => {
                out.push(a[i]);
                i += 1;
                j += 1;
            }
        }
    }
    out.extend_from_slice(&a[i..]);
    out.extend_from_slice(&b[j..]);
    out
}

/// Docs in `a` but not in `b` (AND NOT).
pub fn difference(a: &[u32], b: &[u32]) -> Vec<u32> {
    let mut out = Vec::with_capacity(a.len());
    let mut j = 0;
    for &id in a {
        while j < b.len() && b[j] < id {
            j += 1;
        }
        if j == b.len() || b[j] != id {
            out.push(id);
        }
    }
    out
}

/// One word of a phrase: its posting list and its offset from the phrase's first word.
pub struct PhrasePart<'a> {
    pub postings: &'a PostingList,
    pub offset: u32,
}

/// Docs where every part occurs at `start + offset` for some common `start`.
///
/// Positional intersection: first intersect doc ids, then, inside each shared
/// doc, check that the positions line up. Offsets come from the analyser, so
/// "wing in a slipstream" (stop words removed) is `wing@0 slipstream@3` and
/// still only matches when exactly two words sit between them.
pub fn phrase_docs(parts: &[PhrasePart<'_>]) -> Vec<u32> {
    let Some((first, rest)) = parts.split_first() else {
        return Vec::new();
    };
    let mut candidates = first.postings.doc_ids();
    for part in rest {
        candidates = intersect(&candidates, &part.postings.doc_ids());
    }
    candidates
        .into_iter()
        .filter(|&doc_id| {
            let Some(anchor) = first.postings.get(doc_id) else {
                return false;
            };
            anchor.positions.iter().any(|&position| {
                let Some(start) = position.checked_sub(first.offset) else {
                    return false;
                };
                rest.iter().all(|part| {
                    let (Some(posting), Some(expected)) = (part.postings.get(doc_id), start.checked_add(part.offset))
                    else {
                        return false;
                    };
                    posting.positions.binary_search(&expected).is_ok()
                })
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use proptest::prelude::*;

    use super::*;
    use crate::index::postings::Posting;

    #[test]
    fn merges_sorted_lists() {
        let a = [1, 3, 5, 7, 9];
        let b = [3, 4, 5, 10];
        assert_eq!(intersect(&a, &b), [3, 5]);
        assert_eq!(union(&a, &b), [1, 3, 4, 5, 7, 9, 10]);
        assert_eq!(difference(&a, &b), [1, 7, 9]);
        assert_eq!(difference(&b, &a), [4, 10]);
        assert!(intersect(&a, &[]).is_empty());
        assert_eq!(union(&[], &b), b);
    }

    #[test]
    fn skewed_intersection_uses_binary_search_and_agrees() {
        let large: Vec<u32> = (0..1000).collect();
        assert_eq!(intersect(&[5, 500, 2000], &large), [5, 500]);
        assert_eq!(intersect(&large, &[5, 500, 2000]), [5, 500]);
    }

    fn list(entries: &[(u32, &[u32])]) -> PostingList {
        PostingList::new(
            entries
                .iter()
                .map(|&(doc_id, positions)| Posting {
                    doc_id,
                    positions: positions.to_vec(),
                })
                .collect(),
        )
    }

    #[test]
    fn phrase_requires_adjacent_positions() {
        // doc 1: "memory safety" adjacent; doc 2: both words, not adjacent; doc 3: reversed.
        let memory = list(&[(1, &[4]), (2, &[0]), (3, &[9])]);
        let safety = list(&[(1, &[5]), (2, &[7]), (3, &[8])]);
        let parts = [
            PhrasePart {
                postings: &memory,
                offset: 0,
            },
            PhrasePart {
                postings: &safety,
                offset: 1,
            },
        ];
        assert_eq!(phrase_docs(&parts), [1]);
    }

    #[test]
    fn phrase_offsets_skip_removed_stop_words() {
        // "wing in a slipstream" -> wing@0, slipstream@3
        let wing = list(&[(1, &[10]), (2, &[10])]);
        let slipstream = list(&[(1, &[13]), (2, &[11])]);
        let parts = [
            PhrasePart {
                postings: &wing,
                offset: 0,
            },
            PhrasePart {
                postings: &slipstream,
                offset: 3,
            },
        ];
        assert_eq!(phrase_docs(&parts), [1]);
    }

    #[test]
    fn phrase_matches_any_occurrence_and_repeated_words() {
        // "very very" style phrase: the same list twice at offsets 0 and 1.
        let very = list(&[(1, &[2, 7, 8]), (2, &[2, 4])]);
        let parts = [
            PhrasePart {
                postings: &very,
                offset: 0,
            },
            PhrasePart {
                postings: &very,
                offset: 1,
            },
        ];
        assert_eq!(phrase_docs(&parts), [1]);
    }

    fn sorted_ids() -> impl Strategy<Value = Vec<u32>> {
        proptest::collection::btree_set(0u32..500, 0..200).prop_map(|set| set.into_iter().collect())
    }

    proptest! {
        #[test]
        fn set_operations_match_btreeset(a in sorted_ids(), b in sorted_ids()) {
            let sa: BTreeSet<u32> = a.iter().copied().collect();
            let sb: BTreeSet<u32> = b.iter().copied().collect();
            prop_assert_eq!(intersect(&a, &b), sa.intersection(&sb).copied().collect::<Vec<_>>());
            prop_assert_eq!(union(&a, &b), sa.union(&sb).copied().collect::<Vec<_>>());
            prop_assert_eq!(difference(&a, &b), sa.difference(&sb).copied().collect::<Vec<_>>());
        }
    }
}
