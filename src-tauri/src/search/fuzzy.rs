use crate::indexer::walker::FileEntry;
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use rayon::prelude::*;

/// Computes a 64-bit character presence bitmask for fast prefiltering.
pub fn compute_char_mask(text: &str) -> u64 {
    let mut mask = 0u64;
    for ch in text.chars() {
        let ch_lower = ch.to_ascii_lowercase();
        let bit = match ch_lower {
            'a'..='z' => ch_lower as u8 - b'a',
            '0'..='9' => 26 + (ch_lower as u8 - b'0'),
            '_' => 36,
            '-' => 37,
            '.' => 38,
            '/' | '\\' => 39,
            ' ' => 40,
            _ => 41 + ((ch as u32 % 23) as u8),
        };
        mask |= 1u64 << (bit & 63);
    }
    mask
}

/// Scores one entry for a parsed pattern. Callers provide the matcher and
/// scratch buffer so parallel workers can keep their own state.
fn score_entry(
    pattern: &Pattern,
    matcher: &mut Matcher,
    char_buf: &mut Vec<char>,
    entry: &FileEntry,
    query_mask: u64,
    match_path: bool,
) -> Option<u32> {
    // Char-bloom prefilter: 64-bit letter mask per name.
    if !match_path && (entry.char_bloom & query_mask) != query_mask {
        return None;
    }
    let path_str = entry.path.to_string_lossy();
    let utf32 = Utf32Str::new(&path_str, char_buf);
    let mut final_score = pattern.score(utf32, matcher)?;
    let name_utf32 = Utf32Str::new(&entry.name, char_buf);
    if let Some(name_score) = pattern.score(name_utf32, matcher) {
        final_score += name_score * 2;
    }
    Some(final_score)
}

pub struct FuzzySearcher {
    matcher: Matcher,
}

impl Default for FuzzySearcher {
    fn default() -> Self {
        Self::new()
    }
}

impl FuzzySearcher {
    pub fn new() -> Self {
        Self {
            matcher: Matcher::new(Config::DEFAULT.match_paths()),
        }
    }

    pub fn search(
        &mut self,
        query: &str,
        all_entries: &[FileEntry],
        candidate_indices: &[usize],
        limit: usize,
    ) -> Vec<(usize, u32)> {
        self.search_with_total(query, all_entries, candidate_indices, limit)
            .0
    }

    /// Same scan as [`Self::search`], but also reports how many entries
    /// matched before the top-k cut.
    ///
    /// The count is free: every candidate is already scored, and the truncation
    /// happens afterwards. Callers use it to answer "how many files match?"
    /// honestly instead of reporting the size of their own result window.
    pub fn search_with_total(
        &mut self,
        query: &str,
        all_entries: &[FileEntry],
        candidate_indices: &[usize],
        limit: usize,
    ) -> (Vec<(usize, u32)>, usize) {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return (Vec::new(), 0);
        }

        let pattern = Pattern::parse(trimmed, CaseMatching::Ignore, Normalization::Smart);
        let query_mask = compute_char_mask(trimmed);
        let match_path = trimmed.contains('/') || trimmed.contains('\\');

        // Large candidate sets (1M-file corpora) are scored across the rayon
        // pool; small ones stay sequential so a keystroke never pays
        // scheduling overhead.
        const PARALLEL_THRESHOLD: usize = 20_000;
        let mut scored_indices: Vec<(usize, u32)> = if candidate_indices.len() >= PARALLEL_THRESHOLD
        {
            candidate_indices
                .par_iter()
                .map_init(
                    || (Matcher::new(Config::DEFAULT.match_paths()), Vec::new()),
                    |(matcher, char_buf), &orig_idx| {
                        all_entries.get(orig_idx).and_then(|entry| {
                            score_entry(&pattern, matcher, char_buf, entry, query_mask, match_path)
                                .map(|score| (orig_idx, score))
                        })
                    },
                )
                .flatten()
                .collect()
        } else {
            let mut char_buf = Vec::new();
            let mut scored = Vec::new();
            for &orig_idx in candidate_indices {
                let Some(entry) = all_entries.get(orig_idx) else {
                    continue;
                };
                if let Some(score) = score_entry(
                    &pattern,
                    &mut self.matcher,
                    &mut char_buf,
                    entry,
                    query_mask,
                    match_path,
                ) {
                    scored.push((orig_idx, score));
                }
            }
            scored
        };

        // Top-k heap/selection using select_nth_unstable: O(N) instead of O(N log N)
        //
        // The comparator is a *total* order (score, then path) on purpose.
        // Scores tie constantly here — every file in a screenshot folder
        // matches a one-word query equally well — and a partial order let the
        // cut keep an arbitrary subset of the ties, which changed with the
        // caller's limit. That made offset paging incoherent: page 2 could
        // repeat rows page 1 had already shown and skip others entirely.
        // Ordering by path breaks the ties the same way on every call, so the
        // top-N of a wider window is a superset of the top-N of a narrower one.
        let total_order = |a: &(usize, u32), b: &(usize, u32)| {
            b.1.cmp(&a.1).then_with(|| {
                let left = all_entries.get(a.0).map(|entry| &entry.path);
                let right = all_entries.get(b.0).map(|entry| &entry.path);
                left.cmp(&right)
            })
        };
        let total_matched = scored_indices.len();
        if scored_indices.len() > limit {
            scored_indices.select_nth_unstable_by(limit, total_order);
            scored_indices.truncate(limit);
        }
        scored_indices.sort_unstable_by(total_order);

        (scored_indices, total_matched)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    /// (query, expected file) pairs used by the ranking tests. The expected
    /// value is matched against the entry path suffix.
    const CASES: &[(&str, &str)] = &[
        ("cargo", "Cargo.toml"),
        ("app", "App.tsx"),
        ("search", "search/mod.rs"),
        ("readme", "README.md"),
        ("fuzzy", "fuzzy.rs"),
        ("settings", "settings.json"),
    ];

    /// Builds an entry from a repo-relative path. The display name mirrors the
    /// relative path, so the name-based char-bloom prefilter sees the same
    /// characters as the path that nucleo scores.
    fn entry(relative: &str) -> FileEntry {
        let path = PathBuf::from(relative);
        let extension =
            crate::indexer::walker::intern_extension(path.extension().and_then(|e| e.to_str()));
        FileEntry {
            id: 0,
            char_bloom: compute_char_mask(relative),
            path,
            name: relative.to_string(),
            extension,
            size: 0,
            modified: 0,
            is_dir: false,
            dev: 0,
            ino: 0,
            content_pending: false,
        }
    }

    fn fixture() -> Vec<FileEntry> {
        [
            "repo/Cargo.toml",
            "repo/app/App.tsx",
            "repo/src/search/mod.rs",
            "repo/README.md",
            "repo/src/main.rs",
            "repo/src/search/fuzzy.rs",
            "repo/config/settings.json",
            "repo/notes.md",
            "repo/package.json",
            "repo/tsconfig.json",
            "repo/vite.config.ts",
            "repo/src/lib.rs",
            "repo/index.html",
            "repo/styles.css",
            "repo/src/embed/model.rs",
        ]
        .into_iter()
        .map(entry)
        .collect()
    }

    fn index_of(entries: &[FileEntry], expected: &str) -> usize {
        entries
            .iter()
            .position(|e| Path::new(expected) == e.path || e.path.ends_with(expected))
            .unwrap_or_else(|| panic!("fixture has no entry matching {expected}"))
    }

    #[test]
    fn expected_files_rank_in_top_three() {
        let entries = fixture();
        let candidates: Vec<usize> = (0..entries.len()).collect();
        let mut searcher = FuzzySearcher::new();

        for &(query, expected) in CASES {
            let results = searcher.search(query, &entries, &candidates, 50);
            assert!(!results.is_empty(), "query {query:?} returned nothing");

            let top_three: Vec<&Path> = results
                .iter()
                .take(3)
                .map(|&(idx, _)| entries[idx].path.as_path())
                .collect();
            assert!(
                top_three.iter().any(|path| path.ends_with(expected)),
                "query {query:?}: expected {expected} in top 3, got {top_three:?}"
            );
        }
    }

    #[test]
    fn char_bloom_prefilter_keeps_valid_matches() {
        let entries = fixture();
        let candidates: Vec<usize> = (0..entries.len()).collect();
        let mut searcher = FuzzySearcher::new();

        for &(query, expected) in CASES {
            let expected_idx = index_of(&entries, expected);
            let query_mask = compute_char_mask(query);

            // The expected file must pass the char-bloom prefilter ...
            assert_eq!(
                entries[expected_idx].char_bloom & query_mask,
                query_mask,
                "query {query:?}: {expected} fails the char-bloom prefilter"
            );

            // ... and therefore survive into the search results.
            let results = searcher.search(query, &entries, &candidates, 50);
            assert!(
                results.iter().any(|&(idx, _)| idx == expected_idx),
                "query {query:?}: {expected} was dropped before scoring"
            );
        }
    }

    #[test]
    fn prefilter_drops_only_files_without_query_chars() {
        let entries = fixture();
        let candidates: Vec<usize> = (0..entries.len()).collect();
        let mut searcher = FuzzySearcher::new();

        // Reference scan without the prefilter: every entry the matcher would
        // accept is expected to survive the char-bloom check and reach scoring.
        let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
        let mut buf = Vec::new();

        for &(query, _) in CASES {
            let query_mask = compute_char_mask(query);
            let pattern = Pattern::parse(query, CaseMatching::Ignore, Normalization::Smart);

            let mut expected_matches = Vec::new();
            for (idx, entry) in entries.iter().enumerate() {
                let path = entry.path.to_string_lossy();
                let accepted = pattern
                    .score(Utf32Str::new(&path, &mut buf), &mut matcher)
                    .is_some()
                    || pattern
                        .score(Utf32Str::new(&entry.name, &mut buf), &mut matcher)
                        .is_some();
                if accepted && entry.char_bloom & query_mask == query_mask {
                    expected_matches.push(idx);
                }
            }

            let results = searcher.search(query, &entries, &candidates, 50);
            for idx in expected_matches {
                assert!(
                    results.iter().any(|&(result_idx, _)| result_idx == idx),
                    "query {query:?}: entry {} was dropped by the prefilter",
                    entries[idx].name
                );
            }
        }
    }
}
