use crate::embed::model::ModelRegistry;
use crate::embed::units::{Unit, UnitStore};
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone)]
pub struct SemanticHit {
    pub node_id: u64,
    pub score: f32,
    pub best_unit: Unit,
}

pub struct ExactScanner;

/// Below this share of the corpus, scanning the candidate set beats scanning
/// every vector. Above it the extra indirection stops paying for itself.
const ALLOWED_SWEEP_MAX_DENSITY: f32 = 0.30;

impl ExactScanner {
    /// Exact parallel scan over memory-mapped / in-memory i8 vectors.
    /// Returns top-k hits grouped by node_id with max score and best unit range.
    ///
    /// When `allowed` is `Some`, units whose `node_id` is not in the set are
    /// skipped before computing cosine similarity. This keeps scoped/filtered
    /// queries from scoring the entire corpus.
    pub fn scan(
        query_vec: &[i8],
        store: &UnitStore,
        allowed: Option<&HashSet<u64>>,
        limit: usize,
    ) -> Vec<SemanticHit> {
        Self::scan_with_model(query_vec, store, allowed, limit, None)
    }

    /// Exact parallel scan over memory-mapped / in-memory i8 vectors.
    /// Returns top-k hits grouped by node_id with max score and best unit range.
    ///
    /// When `allowed` is `Some`, units whose `node_id` is not in the set are
    /// skipped before computing cosine similarity. This keeps scoped/filtered
    /// queries from scoring the entire corpus.
    ///
    /// When `expected_model_id` is `Some`, units created by a different model
    /// are skipped so vectors from different models/spaces are never compared.
    pub fn scan_with_model(
        query_vec: &[i8],
        store: &UnitStore,
        allowed: Option<&HashSet<u64>>,
        limit: usize,
        expected_model_id: Option<u32>,
    ) -> Vec<SemanticHit> {
        if store.is_empty() || query_vec.is_empty() || query_vec.len() != store.dim {
            return Vec::new();
        }

        let dim = store.dim;
        let units = &store.units;
        let vectors = &store.vectors;

        // Cost of the membership test in the `allowed` set. The caller's set
        // holds one entry per candidate, so it is 8x denser than the units it
        // filters here: a query narrowed to `type:image` lets ~2% of the
        // corpus through, and paying for a full scan of every vector just to
        // discard 98% of the results cost ~2s per keystroke on a 1.6M-unit
        // index. `par_iter` over the candidate set instead is 8x smaller and
        // does no discarding at all.
        //
        // `store.node_to_units` already records which unit indices belong to a
        // node, so no ordering assumption is needed and the membership check
        // below becomes a no-op for anything gathered this way.
        let allowed_density = match allowed {
            Some(set) if !set.is_empty() => set.len() as f32 / units.len() as f32,
            _ => 1.0,
        };
        let unit_indices: Vec<usize> = match allowed {
            Some(set) if allowed_density <= ALLOWED_SWEEP_MAX_DENSITY => set
                .iter()
                .filter_map(|node_id| store.node_to_units.get(node_id))
                .flat_map(|indices| indices.iter().copied())
                .filter(|&idx| idx < units.len())
                .collect(),
            _ => (0..units.len()).collect(),
        };

        let scored_units: Vec<(usize, f32)> = unit_indices
            .into_par_iter()
            .filter_map(|idx| {
                let unit = &units[idx];
                if let Some(expected_id) = expected_model_id
                    && unit.model_id != expected_id
                {
                    return None;
                }

                let node_id = unit.node_id;
                if let Some(allowed) = allowed
                    && !allowed.contains(&node_id)
                {
                    return None;
                }

                let start = idx * dim;
                let end = start + dim;
                let unit_vec = &vectors[start..end];
                let sim = ModelRegistry::cosine_similarity_i8(query_vec, unit_vec);
                Some((idx, sim))
            })
            .filter(|&(_, sim)| sim > 0.05)
            .collect();

        // Group by node_id, keeping highest scoring unit per node
        let mut node_best: HashMap<u64, (f32, usize)> = HashMap::new();
        for (unit_idx, sim) in scored_units {
            let node_id = units[unit_idx].node_id;
            node_best
                .entry(node_id)
                .and_modify(|existing| {
                    if sim > existing.0 {
                        *existing = (sim, unit_idx);
                    }
                })
                .or_insert((sim, unit_idx));
        }

        let mut hits: Vec<SemanticHit> = node_best
            .into_iter()
            .map(|(node_id, (score, best_idx))| SemanticHit {
                node_id,
                score,
                best_unit: units[best_idx].clone(),
            })
            .collect();

        // Ties are broken by node id. Equal scores are common (a synthetic
        // corpus gives every hit 1.0), and an unordered tie makes the result
        // order depend on HashMap iteration -- which changes between runs and
        // makes paging skip or repeat rows.
        let rank = |a: &SemanticHit, b: &SemanticHit| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.node_id.cmp(&b.node_id))
        };
        if hits.len() > limit {
            hits.select_nth_unstable_by(limit, rank);
            hits.truncate(limit);
        }
        hits.sort_by(rank);

        hits
    }
}

#[cfg(test)]
mod sweep_tests {
    use super::*;
    use crate::embed::units::Unit;

    fn unit(id: u64, node_id: u64) -> Unit {
        Unit {
            id,
            node_id,
            byte_start: 0,
            byte_end: 10,
            model_id: 1,
        }
    }

    /// A filtered scan must return exactly what the exhaustive scan returns
    /// once the filter is applied. The candidate-set sweep is an optimisation,
    /// so any difference is a bug, not a tuning choice.
    #[test]
    fn candidate_sweep_matches_full_scan_with_filter() {
        let mut store = UnitStore::new(4);
        for node in 1..=40u64 {
            // Three units per node, so the per-node grouping is exercised.
            for k in 0..3u64 {
                let vector = if node % 7 == 0 {
                    [127i8, 0, 0, 0]
                } else {
                    [0, 60, 60, 0]
                };
                store.add_unit(unit(node * 10 + k, node), &vector);
            }
        }

        let query = [127i8, 0, 0, 0];
        // Sparse filter: only every 7th node is a candidate, well under the
        // density threshold, so the candidate sweep runs.
        let allowed: HashSet<u64> = (1..=40u64).filter(|n| n % 7 == 0).collect();
        let swept = ExactScanner::scan_with_model(&query, &store, Some(&allowed), 10, None);
        assert!(!swept.is_empty(), "the sweep must find the matching nodes");
        assert!(swept.iter().all(|hit| allowed.contains(&hit.node_id)));

        // Reference: score everything, then filter.
        let everything = ExactScanner::scan_with_model(&query, &store, None, 1000, None);
        let expected: Vec<u64> = everything
            .iter()
            .filter(|hit| allowed.contains(&hit.node_id))
            .map(|hit| hit.node_id)
            .take(10)
            .collect();
        let got: Vec<u64> = swept.iter().map(|hit| hit.node_id).collect();
        assert_eq!(got, expected, "candidate sweep changed the ranking");
    }

    /// Dense filters must keep using the full scan and still be correct.
    #[test]
    fn dense_filter_still_scores_everything() {
        let mut store = UnitStore::new(4);
        for node in 1..=20u64 {
            store.add_unit(unit(node, node), &[127i8, 0, 0, 0]);
        }
        let allowed: HashSet<u64> = (1..=20u64).collect();
        let hits = ExactScanner::scan_with_model(&[127i8, 0, 0, 0], &store, Some(&allowed), 5, None);
        assert_eq!(hits.len(), 5);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(id: u64, node_id: u64) -> Unit {
        Unit {
            id,
            node_id,
            byte_start: 0,
            byte_end: 10,
            model_id: 1,
        }
    }

    #[test]
    fn prefilter_skips_non_candidate_nodes_before_scoring() {
        let mut store = UnitStore::new(4);
        store.add_unit(unit(0, 1), &[127, 0, 0, 0]);
        store.add_unit(unit(1, 2), &[0, 127, 0, 0]);

        // Query matches node 2 exactly.
        let query = [0i8, 127, 0, 0];

        let all = ExactScanner::scan(&query, &store, None, 10);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].node_id, 2);

        let allowed: HashSet<u64> = [1u64].into_iter().collect();
        let filtered = ExactScanner::scan(&query, &store, Some(&allowed), 10);
        assert!(filtered.is_empty(), "node 2 should be skipped by prefilter");

        let allowed: HashSet<u64> = [1u64, 2u64].into_iter().collect();
        let both = ExactScanner::scan(&query, &store, Some(&allowed), 10);
        assert_eq!(both.len(), 1);
        assert_eq!(both[0].node_id, 2);
    }

    #[test]
    fn model_id_isolation_skips_units_from_different_models() {
        let mut store = UnitStore::new(4);
        let mut u1 = unit(0, 1);
        u1.model_id = 1;
        let mut u2 = unit(1, 2);
        u2.model_id = 2; // Different model

        store.add_unit(u1, &[127, 0, 0, 0]);
        store.add_unit(u2, &[127, 0, 0, 0]);

        let query = [127i8, 0, 0, 0];

        // Active model is 1: unit 2 must be ignored
        let hits = ExactScanner::scan_with_model(&query, &store, None, 10, Some(1));
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].node_id, 1);

        // Active model is 2: unit 1 must be ignored
        let hits = ExactScanner::scan_with_model(&query, &store, None, 10, Some(2));
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].node_id, 2);

        // Active model is 3: neither matches
        let hits = ExactScanner::scan_with_model(&query, &store, None, 10, Some(3));
        assert!(hits.is_empty());
    }
    // ── Additional coverage ───────────────────────────────────────────────────

    #[test]
    fn limit_is_respected_when_many_nodes_match() {
        // Create 10 nodes that all perfectly match the query vector.
        let mut store = UnitStore::new(4);
        for node_id in 1u64..=10 {
            store.add_unit(unit(node_id, node_id), &[127i8, 0, 0, 0]);
        }
        let query = [127i8, 0, 0, 0];
        let hits = ExactScanner::scan(&query, &store, None, 5);
        assert_eq!(hits.len(), 5, "limit must cap results to 5");
    }

    #[test]
    fn results_are_ordered_by_score_descending() {
        let mut store = UnitStore::new(4);
        // node 1: perfect match for [127,0,0,0].
        store.add_unit(unit(0, 1), &[127i8, 0, 0, 0]);
        // node 2: partial match.
        store.add_unit(unit(1, 2), &[100i8, 50, 0, 0]);
        // node 3: weaker match.
        store.add_unit(unit(2, 3), &[60i8, 60, 60, 0]);

        let query = [127i8, 0, 0, 0];
        let hits = ExactScanner::scan(&query, &store, None, 10);

        // Must be sorted descending by score.
        for window in hits.windows(2) {
            assert!(
                window[0].score >= window[1].score,
                "results not in descending score order"
            );
        }
        // Perfect match must be first.
        assert_eq!(hits[0].node_id, 1);
    }

    #[test]
    fn score_threshold_filters_low_similarity_units() {
        let mut store = UnitStore::new(4);
        // node 1: perfectly orthogonal to the query — similarity ~0, below 0.05 threshold.
        store.add_unit(unit(0, 1), &[0i8, 127, 0, 0]);
        // node 2: strongly aligned.
        store.add_unit(unit(1, 2), &[127i8, 0, 0, 0]);

        let query = [127i8, 0, 0, 0];
        let hits = ExactScanner::scan(&query, &store, None, 10);

        // node 1 has cosine ≈ 0 with [127,0,0,0] so it should be filtered.
        assert!(
            hits.iter().all(|h| h.node_id != 1),
            "node 1 should be filtered by the 0.05 score threshold"
        );
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].node_id, 2);
    }

    #[test]
    fn dim_mismatch_returns_empty() {
        let mut store = UnitStore::new(4);
        store.add_unit(unit(0, 1), &[127i8, 0, 0, 0]);

        // query vec length 3 ≠ store.dim 4
        let query = [127i8, 0, 0];
        let hits = ExactScanner::scan(&query, &store, None, 10);
        assert!(hits.is_empty(), "dim mismatch must return empty");
    }

    #[test]
    fn empty_store_returns_empty() {
        let store = UnitStore::new(4);
        let query = [127i8, 0, 0, 0];
        let hits = ExactScanner::scan(&query, &store, None, 10);
        assert!(hits.is_empty());
    }

    #[test]
    fn allowed_filter_with_no_matches_returns_empty() {
        let mut store = UnitStore::new(4);
        store.add_unit(unit(0, 1), &[127i8, 0, 0, 0]);
        store.add_unit(unit(1, 2), &[127i8, 0, 0, 0]);

        // allowed set contains node 99 which isn't in the store.
        let allowed: HashSet<u64> = [99u64].into_iter().collect();
        let hits = ExactScanner::scan(&query_vec(), &store, Some(&allowed), 10);
        assert!(hits.is_empty());
    }

    fn query_vec() -> [i8; 4] {
        [127i8, 0, 0, 0]
    }
}
