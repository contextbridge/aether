use nucleo::pattern::{CaseMatching, Normalization, Pattern};
use nucleo::{Config, Matcher, Utf32Str};

/// The indices of the `keys` matching `query`, best match first, and in their order among equals.
pub(crate) fn filter(query: &str, keys: impl Iterator<Item = String>) -> Vec<usize> {
    let mut matcher = Matcher::new(Config::DEFAULT);
    let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
    let mut buf = Vec::new();
    let mut scored: Vec<(u32, usize)> = keys
        .enumerate()
        .filter_map(|(index, key)| {
            pattern.score(Utf32Str::new(&key, &mut buf), &mut matcher).map(|score| (score, index))
        })
        .collect();
    scored.sort_by_key(|&(score, _)| std::cmp::Reverse(score));
    scored.into_iter().map(|(_, index)| index).collect()
}
