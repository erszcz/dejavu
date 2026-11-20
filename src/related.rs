//! The shared-call signal: units that use the same rare combination of calls and types.
//!
//! Two units can do the same job with code of a different shape, so the structural check gives
//! them no link. They still call the same few functions. A group here is a set of at least three
//! rare features that units in several files and areas all have. One shared call is reuse, and is
//! not reported. A combination that many places build again is a mechanism without a home.
//!
//! A group is a candidate for a reader to judge. The places can be the same job written again,
//! or different jobs that use the same interface correctly.

use std::collections::{BTreeSet, HashMap};

use serde::Serialize;

use crate::report::{area, round, Place};
use crate::unit::Outline;

/// A feature is rare when it is in at least 3 files and at most 60. A feature in fewer files
/// cannot make a group. A feature in more files is common vocabulary.
const RARE_MIN_FILES: usize = 3;
const RARE_MAX_FILES: usize = 60;
/// A group needs units in this many files and areas.
const MIN_FILES: usize = 3;
const MIN_AREAS: usize = 2;
/// Only the rarest features of a unit make combinations, which bounds the work for a large unit.
const MAX_UNIT_FEATURES: usize = 14;

#[derive(Debug, PartialEq, Serialize)]
pub struct Finding {
    /// The features that every member has, rarest first.
    pub shared: Vec<String>,
    pub files: usize,
    pub areas: usize,
    pub score: f64,
    /// How many of the members the `duplicates` signal also links to another member. When two
    /// signals agree on a pair, the finding is stronger.
    pub also_duplicates: usize,
    pub members: Vec<Place>,
}

struct Group {
    features: BTreeSet<usize>,
    members: Vec<usize>,
    files: usize,
    areas: usize,
    also_duplicates: usize,
    score: f64,
}

/// Groups in rank order: the rarity of the shared features × files × areas, highest first.
///
/// With `changed`, only groups that have a changed unit are findings. `duplicate_cluster` gives
/// the structural cluster of a unit. Members that are also in one structural cluster raise the
/// score of a group by their share of the members, up to twice the score.
pub fn findings(
    outlines: &[Outline],
    area_depth: usize,
    changed: Option<&[bool]>,
    duplicate_cluster: &dyn Fn(&Outline) -> Option<usize>,
) -> Vec<Finding> {
    let mut names: Vec<&str> = Vec::new();
    let mut files_of: Vec<BTreeSet<&str>> = Vec::new();
    let mut ids: HashMap<&str, usize> = HashMap::new();
    let features: Vec<Vec<usize>> = outlines
        .iter()
        .map(|outline| {
            outline
                .features
                .iter()
                .map(|feature| {
                    let id = *ids.entry(feature).or_insert_with(|| {
                        names.push(feature);
                        files_of.push(BTreeSet::new());
                        names.len() - 1
                    });
                    files_of[id].insert(&outline.path);
                    id
                })
                .collect()
        })
        .collect();
    let all_files: BTreeSet<&str> = outlines.iter().map(|o| o.path.as_str()).collect();
    let rarity = |id: usize| (all_files.len() as f64 / files_of[id].len() as f64).ln();
    let rare = |id: usize| (RARE_MIN_FILES..=RARE_MAX_FILES).contains(&files_of[id].len());
    let spread = |members: &[usize]| {
        let files: BTreeSet<&str> = members.iter().map(|m| outlines[*m].path.as_str()).collect();
        let areas: BTreeSet<&str> = files.iter().map(|path| area(path, area_depth)).collect();
        (files.len(), areas.len())
    };

    // The units that have each combination of three rare features.
    let mut support: HashMap<[usize; 3], Vec<usize>> = HashMap::new();
    for (unit, features) in features.iter().enumerate() {
        let mut rarest: Vec<usize> = features.iter().copied().filter(|id| rare(*id)).collect();
        rarest.sort_by(|a, b| {
            rarity(*b)
                .total_cmp(&rarity(*a))
                .then(names[*a].cmp(names[*b]))
        });
        rarest.truncate(MAX_UNIT_FEATURES);
        rarest.sort_unstable();
        for (i, a) in rarest.iter().enumerate() {
            for (j, b) in rarest.iter().enumerate().skip(i + 1) {
                for c in &rarest[j + 1..] {
                    support.entry([*a, *b, *c]).or_default().push(unit);
                }
            }
        }
    }

    // Combinations with the same units are one group that shares all their features.
    let mut by_members: HashMap<Vec<usize>, BTreeSet<usize>> = HashMap::new();
    for (combination, members) in support {
        let (files, areas) = spread(&members);
        if files >= MIN_FILES && areas >= MIN_AREAS {
            by_members.entry(members).or_default().extend(combination);
        }
    }
    let mut groups: Vec<Group> = by_members
        .into_iter()
        .filter(|(members, _)| changed.is_none_or(|changed| members.iter().any(|m| changed[*m])))
        .map(|(members, features)| {
            let (files, areas) = spread(&members);
            let weight: f64 = features.iter().map(|id| rarity(*id)).sum();
            let also_duplicates = also_duplicates(&members, outlines, duplicate_cluster);
            let agreement = 1.0 + also_duplicates as f64 / members.len() as f64;
            Group {
                score: weight * files as f64 * areas as f64 * agreement,
                also_duplicates,
                features,
                members,
                files,
                areas,
            }
        })
        .collect();
    groups.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.members.cmp(&b.members))
    });

    // A lower group that says little new is left out: its features are all in a higher group, or
    // more than half of its members are.
    let mut kept: Vec<Group> = Vec::new();
    for group in groups {
        let repeats = kept.iter().any(|higher| {
            let common = group
                .members
                .iter()
                .filter(|m| higher.members.contains(m))
                .count();
            group.features.is_subset(&higher.features) || 2 * common > group.members.len()
        });
        if !repeats {
            kept.push(group);
        }
    }

    kept.into_iter()
        .map(|group| {
            let mut shared: Vec<usize> = group.features.into_iter().collect();
            shared.sort_by(|a, b| {
                rarity(*b)
                    .total_cmp(&rarity(*a))
                    .then(names[*a].cmp(names[*b]))
            });
            Finding {
                shared: shared.into_iter().map(|id| names[id].to_string()).collect(),
                files: group.files,
                areas: group.areas,
                score: round(group.score),
                also_duplicates: group.also_duplicates,
                members: group
                    .members
                    .iter()
                    .map(|member| {
                        let outline = &outlines[*member];
                        Place {
                            path: outline.path.clone(),
                            start_line: outline.start_line,
                            end_line: outline.end_line,
                            name: outline.name.clone(),
                            changed: changed.is_some_and(|changed| changed[*member]),
                        }
                    })
                    .collect(),
            }
        })
        .collect()
}

/// The number of members that share a structural cluster with another member.
fn also_duplicates(
    members: &[usize],
    outlines: &[Outline],
    duplicate_cluster: &dyn Fn(&Outline) -> Option<usize>,
) -> usize {
    let clusters: Vec<usize> = members
        .iter()
        .filter_map(|member| duplicate_cluster(&outlines[*member]))
        .collect();
    clusters
        .iter()
        .filter(|cluster| clusters.iter().filter(|other| other == cluster).count() > 1)
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outline(path: &str, features: &[&str]) -> Outline {
        Outline {
            path: path.to_string(),
            start_line: 1,
            end_line: 9,
            name: "f".to_string(),
            features: features.iter().map(|feature| feature.to_string()).collect(),
        }
    }

    fn none(_: &Outline) -> Option<usize> {
        None
    }

    /// Three files in two areas share `A.a`, `B.b` and `C.c`. `Common.x` is in every file.
    fn corpus() -> Vec<Outline> {
        let mut outlines = vec![
            outline("lib/app/one/a.ex", &["A.a", "B.b", "C.c", "Common.x"]),
            outline("lib/app/one/b.ex", &["A.a", "B.b", "C.c", "Common.x"]),
            outline("lib/app/two/c.ex", &["A.a", "B.b", "C.c", "Common.x"]),
        ];
        for index in 0..70 {
            outlines.push(outline(
                &format!("lib/app/other/f{index}.ex"),
                &["Common.x"],
            ));
        }
        outlines
    }

    fn members(finding: &Finding) -> Vec<&str> {
        finding.members.iter().map(|m| m.path.as_str()).collect()
    }

    #[test]
    fn units_that_share_three_rare_features_are_a_group() {
        let found = findings(&corpus(), 3, None, &none);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].shared, ["A.a", "B.b", "C.c"]);
        assert_eq!((found[0].files, found[0].areas), (3, 2));
        assert_eq!(
            members(&found[0]),
            ["lib/app/one/a.ex", "lib/app/one/b.ex", "lib/app/two/c.ex"]
        );
    }

    #[test]
    fn two_shared_features_are_not_a_group() {
        let mut outlines = corpus();
        for outline in outlines.iter_mut().take(3) {
            outline.features.retain(|feature| feature != "C.c");
        }
        assert_eq!(findings(&outlines, 3, None, &none), []);
    }

    #[test]
    fn a_group_needs_three_files() {
        let mut outlines = corpus();
        outlines[1].path = "lib/app/one/a.ex".to_string();
        assert_eq!(findings(&outlines, 3, None, &none), []);
    }

    #[test]
    fn a_group_needs_two_areas() {
        let mut outlines = corpus();
        outlines[2].path = "lib/app/one/c.ex".to_string();
        assert_eq!(findings(&outlines, 3, None, &none), []);
    }

    #[test]
    fn a_common_feature_does_not_count() {
        let mut outlines = corpus();
        for outline in outlines.iter_mut().take(3) {
            outline.features = vec!["A.a".into(), "B.b".into(), "Common.x".into()];
        }
        assert_eq!(findings(&outlines, 3, None, &none), []);
    }

    #[test]
    fn with_changes_only_a_group_with_a_changed_unit_is_a_finding() {
        let outlines = corpus();
        let mut changed = vec![false; outlines.len()];
        assert_eq!(findings(&outlines, 3, Some(&changed), &none), []);
        changed[2] = true;
        let found = findings(&outlines, 3, Some(&changed), &none);
        let flags: Vec<bool> = found[0].members.iter().map(|m| m.changed).collect();
        assert_eq!(flags, [false, false, true]);
    }

    #[test]
    fn members_that_are_also_structural_duplicates_are_counted_and_raise_the_score() {
        let outlines = corpus();
        let alone = findings(&outlines, 3, None, &none);
        assert_eq!(alone[0].also_duplicates, 0);

        let one = |outline: &Outline| (outline.path == "lib/app/one/a.ex").then_some(7);
        assert_eq!(findings(&outlines, 3, None, &one), alone);

        let two = |outline: &Outline| outline.path.starts_with("lib/app/one/").then_some(7);
        let agreed = findings(&outlines, 3, None, &two);
        assert_eq!(agreed[0].also_duplicates, 2);
        assert_eq!(agreed[0].score, round(alone[0].score * (1.0 + 2.0 / 3.0)));

        let apart = |outline: &Outline| match outline.path.as_str() {
            "lib/app/one/a.ex" => Some(7),
            "lib/app/one/b.ex" => Some(8),
            _ => None,
        };
        assert_eq!(findings(&outlines, 3, None, &apart), alone);
    }
}
