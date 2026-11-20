//! Groups similar pairs into clusters. Grouping is transitive (A~B and B~C give one cluster), so
//! each cluster also reports its weakest pair: a low value shows a chain, not a tight group.

use std::collections::BTreeMap;

use crate::fingerprint::jaccard;
use crate::index::Pair;
use crate::unit::Unit;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The same tokens, apart from the units' own names.
    Identical,
    /// The same tokens after names, numbers and strings are replaced.
    IdenticalModuloNames,
    Near,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Identical => "identical",
            Kind::IdenticalModuloNames => "identical-modulo-names",
            Kind::Near => "near",
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct Cluster {
    /// Indices into the units, in ascending order.
    pub members: Vec<usize>,
    pub kind: Kind,
    /// The mean similarity of all pairs of members.
    pub similarity: f64,
    /// The lowest similarity of any pair of members.
    pub weakest: f64,
}

pub fn clusters(units: &[Unit], pairs: &[Pair]) -> Vec<Cluster> {
    let mut parent: Vec<usize> = (0..units.len()).collect();
    for pair in pairs {
        let (a, b) = (find(&mut parent, pair.a), find(&mut parent, pair.b));
        parent[a.max(b)] = a.min(b);
    }
    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for pair in pairs {
        for member in [pair.a, pair.b] {
            groups
                .entry(find(&mut parent, member))
                .or_default()
                .push(member);
        }
    }
    groups
        .into_values()
        .map(|mut members| {
            members.sort_unstable();
            members.dedup();
            let first = &units[members[0]];
            let all =
                |same: fn(&Unit, &Unit) -> bool| members.iter().all(|m| same(&units[*m], first));
            let kind = if all(|a, b| a.strict_hash == b.strict_hash) {
                Kind::Identical
            } else if all(|a, b| a.loose_hash == b.loose_hash) {
                Kind::IdenticalModuloNames
            } else {
                Kind::Near
            };
            let mut scores = Vec::new();
            for (position, a) in members.iter().enumerate() {
                for b in &members[position + 1..] {
                    scores.push(jaccard(&units[*a].shingles, &units[*b].shingles));
                }
            }
            let similarity = scores.iter().sum::<f64>() / scores.len() as f64;
            let weakest = scores.iter().copied().fold(f64::INFINITY, f64::min);
            Cluster {
                members,
                kind,
                similarity,
                weakest,
            }
        })
        .collect()
}

fn find(parent: &mut [usize], mut node: usize) -> usize {
    while parent[node] != node {
        parent[node] = parent[parent[node]];
        node = parent[node];
    }
    node
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(strict_hash: u64, loose_hash: u64, shingles: &[u64]) -> Unit {
        Unit {
            path: "a.ex".to_string(),
            start_line: 1,
            end_line: 9,
            name: "f".to_string(),
            strict: Vec::new(),
            strict_hash,
            loose_hash,
            shingles: shingles.to_vec(),
            signature: Vec::new(),
        }
    }

    fn pair(a: usize, b: usize) -> Pair {
        Pair {
            a,
            b,
            similarity: 0.0,
        }
    }

    #[test]
    fn a_chain_is_one_cluster_and_shows_its_weakest_pair() {
        let units = [
            unit(1, 1, &[1, 2, 3, 4]),
            unit(2, 2, &[1, 2, 3, 5]),
            unit(3, 3, &[1, 2, 5, 6]),
            unit(4, 4, &[9]),
        ];
        let found = clusters(&units, &[pair(0, 1), pair(1, 2)]);
        let expected_weakest = 2.0 / 6.0;
        let expected_similarity = (3.0 / 5.0 + 3.0 / 5.0 + expected_weakest) / 3.0;
        assert_eq!(
            found,
            [Cluster {
                members: vec![0, 1, 2],
                kind: Kind::Near,
                similarity: expected_similarity,
                weakest: expected_weakest
            }]
        );
    }

    #[test]
    fn separate_pairs_are_separate_clusters() {
        let units = [
            unit(1, 1, &[1]),
            unit(1, 1, &[1]),
            unit(2, 5, &[2]),
            unit(3, 5, &[2]),
        ];
        let found = clusters(&units, &[pair(0, 1), pair(2, 3)]);
        let kinds: Vec<_> = found.iter().map(|c| (c.members.clone(), c.kind)).collect();
        assert_eq!(
            kinds,
            [
                (vec![0, 1], Kind::Identical),
                (vec![2, 3], Kind::IdenticalModuloNames)
            ]
        );
    }

    #[test]
    fn units_without_a_pair_are_in_no_cluster() {
        assert_eq!(clusters(&[unit(1, 1, &[1]), unit(2, 2, &[2])], &[]), []);
    }
}
