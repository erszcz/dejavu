//! Finds similar pairs of units. MinHash bands give the candidates; exact Jaccard on the shingle
//! sets decides. MinHash can therefore miss a pair, but it never changes a score.

use std::collections::{HashMap, HashSet};

use rayon::prelude::*;

use crate::fingerprint::{hash_sequence, jaccard, BANDS, ROWS};
use crate::unit::Unit;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pair {
    pub a: usize,
    pub b: usize,
    pub similarity: f64,
}

/// Pairs with a similarity of at least `threshold`, with `a < b`, in a stable order.
pub fn similar_pairs(units: &[Unit], threshold: f64) -> Vec<Pair> {
    let mut buckets: HashMap<u64, Vec<usize>> = HashMap::new();
    for (index, unit) in units.iter().enumerate() {
        for band in 0..BANDS {
            let rows = &unit.signature[band * ROWS..(band + 1) * ROWS];
            let key = hash_sequence(&[&[band as u64], rows].concat());
            buckets.entry(key).or_default().push(index);
        }
    }

    let mut candidates: HashSet<(usize, usize)> = HashSet::new();
    for members in buckets.values().filter(|members| members.len() > 1) {
        for (position, a) in members.iter().enumerate() {
            for b in &members[position + 1..] {
                candidates.insert((*a.min(b), *a.max(b)));
            }
        }
    }

    let mut pairs: Vec<Pair> = candidates
        .into_par_iter()
        .filter_map(|(a, b)| {
            let similarity = jaccard(&units[a].shingles, &units[b].shingles);
            (similarity >= threshold).then_some(Pair { a, b, similarity })
        })
        .collect();
    pairs.sort_by_key(|pair| (pair.a, pair.b));
    pairs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::unit::{Extractor, Limits};

    fn units(sources: &[&str]) -> Vec<Unit> {
        let limits = Limits {
            min_lines: 1,
            min_tokens: 1,
        };
        let mut extractor = Extractor::new(&crate::lang::elixir::SPEC).unwrap();
        sources
            .iter()
            .enumerate()
            .flat_map(|(index, source)| {
                extractor
                    .extract(&format!("{index}.ex"), source.as_bytes(), &limits)
                    .units
            })
            .collect()
    }

    const A: &str = "def a(x, y) do\n  z = Map.put(x, :k, y)\n  Enum.map(z, fn {k, v} -> {k, v + 1} end)\nend\n";
    const RENAMED: &str = "def b(p, q) do\n  r = Map.put(p, :k, q)\n  Enum.map(r, fn {m, n} -> {m, n + 7} end)\nend\n";
    const OTHER: &str = "def c(list) do\n  case Repo.one(query(list)) do\n    nil -> {:error, :not_found}\n    row -> {:ok, row}\n  end\nend\n";

    #[test]
    fn a_renamed_copy_is_a_pair_with_similarity_one() {
        let pairs = similar_pairs(&units(&[A, OTHER, RENAMED]), 0.8);
        assert_eq!(
            pairs,
            [Pair {
                a: 0,
                b: 2,
                similarity: 1.0
            }]
        );
    }

    #[test]
    fn unrelated_units_are_not_a_pair() {
        assert_eq!(similar_pairs(&units(&[A, OTHER]), 0.8), []);
    }

    #[test]
    fn the_threshold_decides() {
        let edited = RENAMED.replace("Enum.map(r, fn", "Enum.flat_map(r, fn");
        let found = units(&[A, &edited]);
        let similarity = jaccard(&found[0].shingles, &found[1].shingles);
        assert!(similarity > 0.5 && similarity < 1.0, "{similarity}");
        assert_eq!(similar_pairs(&found, similarity + 0.01), []);
        assert_eq!(
            similar_pairs(&found, similarity),
            [Pair {
                a: 0,
                b: 1,
                similarity
            }]
        );
    }
}
