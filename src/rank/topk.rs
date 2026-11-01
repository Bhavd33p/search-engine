//! Top-k selection with a bounded min-heap: O(n log k) time, O(k) memory.
//!
//! The heap holds the best k documents seen so far, with the *worst* of them on
//! top. Each new document only has to beat that one to get in. Sorting all n
//! scores would cost O(n log n) and O(n) memory instead.

use std::cmp::{Ordering, Reverse};
use std::collections::BinaryHeap;

#[derive(Debug, Clone, Copy)]
pub struct ScoredDoc {
    pub doc_id: u32,
    pub score: f64,
}

/// Higher score ranks first; ties go to the lower doc id so results are deterministic.
impl Ord for ScoredDoc {
    fn cmp(&self, other: &Self) -> Ordering {
        self.score
            .total_cmp(&other.score)
            .then_with(|| other.doc_id.cmp(&self.doc_id))
    }
}

impl PartialOrd for ScoredDoc {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for ScoredDoc {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for ScoredDoc {}

/// Returns the `k` best documents, best first.
pub fn top_k(scored: impl IntoIterator<Item = ScoredDoc>, k: usize) -> Vec<ScoredDoc> {
    if k == 0 {
        return Vec::new();
    }
    // `BinaryHeap` is a max-heap; `Reverse` turns it into a min-heap.
    let mut heap: BinaryHeap<Reverse<ScoredDoc>> = BinaryHeap::with_capacity(k);
    for doc in scored {
        if heap.len() < k {
            heap.push(Reverse(doc));
        } else if let Some(mut worst) = heap.peek_mut()
            && doc > worst.0
        {
            // Replacing through `PeekMut` re-heapifies when `worst` is dropped.
            *worst = Reverse(doc);
        }
    }
    let mut best: Vec<ScoredDoc> = heap.into_iter().map(|Reverse(doc)| doc).collect();
    best.sort_unstable_by(|a, b| b.cmp(a));
    best
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn docs(scores: &[f64]) -> Vec<ScoredDoc> {
        scores
            .iter()
            .enumerate()
            .map(|(i, &score)| ScoredDoc {
                doc_id: i as u32,
                score,
            })
            .collect()
    }

    fn ids(result: &[ScoredDoc]) -> Vec<u32> {
        result.iter().map(|d| d.doc_id).collect()
    }

    #[test]
    fn keeps_the_k_highest_scores_best_first() {
        let result = top_k(docs(&[0.5, 2.0, 0.1, 3.0, 1.0]), 3);
        assert_eq!(ids(&result), [3, 1, 4]);
    }

    #[test]
    fn ties_are_broken_by_lower_doc_id() {
        let result = top_k(docs(&[1.0, 1.0, 1.0, 1.0]), 2);
        assert_eq!(ids(&result), [0, 1]);
    }

    #[test]
    fn handles_k_larger_than_input_and_zero() {
        assert_eq!(ids(&top_k(docs(&[1.0, 2.0]), 10)), [1, 0]);
        assert!(top_k(docs(&[1.0, 2.0]), 0).is_empty());
        assert!(top_k(Vec::new(), 5).is_empty());
    }

    proptest! {
        #[test]
        fn agrees_with_a_full_sort(scores in proptest::collection::vec(-1000i32..1000, 0..300), k in 0usize..50) {
            let all = docs(&scores.iter().map(|&s| f64::from(s) / 7.0).collect::<Vec<_>>());
            let mut sorted = all.clone();
            sorted.sort_unstable_by(|a, b| b.cmp(a));
            sorted.truncate(k);
            prop_assert_eq!(ids(&top_k(all, k)), ids(&sorted));
        }
    }
}
