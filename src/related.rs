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

// The three factors below come from a reading of the 30 highest groups of one codebase. Each
// group was judged as a real repeated job, a partial one, or not one.

/// Each member that the `duplicates` signal also links adds this much of its share to the score.
/// With agreement, 8 of 10 judged groups were real and none was false.
const AGREEMENT_WEIGHT: f64 = 2.0;
/// The factor for a group with no feature from the scanned code, for example three functions of
/// a standard library. 1 of 6 such judged groups was real.
const NOT_LOCAL: f64 = 0.25;
/// The factor for a group that shares only types and no call. Neither of the 2 such judged
/// groups was real: units that build the same request type often do different jobs.
const TYPES_ONLY: f64 = 0.5;

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

/// What `findings` needs to know apart from the outlines.
#[derive(Clone, Copy)]
pub struct Input<'a> {
    /// An area is the first `area_depth` directories of a path.
    pub area_depth: usize,
    /// For each outline, whether it overlaps a changed line. With this, only groups that have a
    /// changed unit are findings.
    pub changed: Option<&'a [bool]>,
    /// The structural cluster of a unit, when it is in one.
    pub duplicate_cluster: &'a dyn Fn(&Outline) -> Option<usize>,
    /// True for a feature that the scanned code defines.
    pub is_local: &'a dyn Fn(&str) -> bool,
}

/// A feature that names a type starts with `%`. Each other feature names a call.
fn is_type(feature: &str) -> bool {
    feature.starts_with('%')
}

struct Group {
    features: BTreeSet<usize>,
    members: Vec<usize>,
    files: usize,
    areas: usize,
    also_duplicates: usize,
    score: f64,
}

/// Groups in rank order, highest score first. The score is the rarity of the shared features ×
/// files × areas, with three factors:
///
/// - members that are also in one structural cluster raise it, by `AGREEMENT_WEIGHT` × their
///   share of the members;
/// - a group with no local feature gets `NOT_LOCAL`;
/// - a group that shares only types gets `TYPES_ONLY`.
///
/// The last two do not apply to a group with agreement.
pub fn findings(outlines: &[Outline], input: &Input) -> Vec<Finding> {
    let Input {
        area_depth,
        changed,
        duplicate_cluster,
        is_local,
    } = *input;
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
            let share = also_duplicates as f64 / members.len() as f64;
            // Agreement is independent evidence, so it removes the two lower factors.
            let agreed = also_duplicates > 0;
            let local = features.iter().any(|id| is_local(names[*id]));
            let calls = features.iter().any(|id| !is_type(names[*id]));
            let score = weight
                * files as f64
                * areas as f64
                * (1.0 + AGREEMENT_WEIGHT * share)
                * if local || agreed { 1.0 } else { NOT_LOCAL }
                * if calls || agreed { 1.0 } else { TYPES_ONLY };
            Group {
                score,
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
    // at least half of its members are. The judged groups with half of their members in a higher
    // group reported the same repeated job as that group.
    let mut kept: Vec<Group> = Vec::new();
    for group in groups {
        let repeats = kept.iter().any(|higher| {
            let common = group
                .members
                .iter()
                .filter(|m| higher.members.contains(m))
                .count();
            group.features.is_subset(&higher.features) || 2 * common >= group.members.len()
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

    /// Scores are rounded for the report, so a factor is compared with a tolerance.
    fn assert_factor(score: f64, base: f64, factor: f64) {
        assert!(
            (score / base - factor).abs() < 0.001,
            "{score} / {base} is not {factor}"
        );
    }

    /// Every feature is local, as in a codebase that calls only its own modules.
    fn run(
        outlines: &[Outline],
        changed: Option<&[bool]>,
        duplicate_cluster: &dyn Fn(&Outline) -> Option<usize>,
    ) -> Vec<Finding> {
        let input = Input {
            area_depth: 3,
            changed,
            duplicate_cluster,
            is_local: &|_| true,
        };
        findings(outlines, &input)
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
        let found = run(&corpus(), None, &none);
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
        assert_eq!(run(&outlines, None, &none), []);
    }

    #[test]
    fn a_group_needs_three_files() {
        let mut outlines = corpus();
        outlines[1].path = "lib/app/one/a.ex".to_string();
        assert_eq!(run(&outlines, None, &none), []);
    }

    #[test]
    fn a_group_needs_two_areas() {
        let mut outlines = corpus();
        outlines[2].path = "lib/app/one/c.ex".to_string();
        assert_eq!(run(&outlines, None, &none), []);
    }

    #[test]
    fn a_common_feature_does_not_count() {
        let mut outlines = corpus();
        for outline in outlines.iter_mut().take(3) {
            outline.features = vec!["A.a".into(), "B.b".into(), "Common.x".into()];
        }
        assert_eq!(run(&outlines, None, &none), []);
    }

    #[test]
    fn with_changes_only_a_group_with_a_changed_unit_is_a_finding() {
        let outlines = corpus();
        let mut changed = vec![false; outlines.len()];
        assert_eq!(run(&outlines, Some(&changed), &none), []);
        changed[2] = true;
        let found = run(&outlines, Some(&changed), &none);
        let flags: Vec<bool> = found[0].members.iter().map(|m| m.changed).collect();
        assert_eq!(flags, [false, false, true]);
    }

    #[test]
    fn members_that_are_also_structural_duplicates_are_counted_and_raise_the_score() {
        let outlines = corpus();
        let alone = run(&outlines, None, &none);
        assert_eq!(alone[0].also_duplicates, 0);

        let one = |outline: &Outline| (outline.path == "lib/app/one/a.ex").then_some(7);
        assert_eq!(run(&outlines, None, &one), alone);

        let two = |outline: &Outline| outline.path.starts_with("lib/app/one/").then_some(7);
        let agreed = run(&outlines, None, &two);
        assert_eq!(agreed[0].also_duplicates, 2);
        assert_factor(agreed[0].score, alone[0].score, 1.0 + 2.0 * 2.0 / 3.0);

        let apart = |outline: &Outline| match outline.path.as_str() {
            "lib/app/one/a.ex" => Some(7),
            "lib/app/one/b.ex" => Some(8),
            _ => None,
        };
        assert_eq!(run(&outlines, None, &apart), alone);
    }

    #[test]
    fn a_group_with_no_local_feature_has_a_quarter_of_the_score() {
        let outlines = corpus();
        let local = run(&outlines, None, &none);
        let input = |is_local: &'static dyn Fn(&str) -> bool| Input {
            area_depth: 3,
            changed: None,
            duplicate_cluster: &none,
            is_local,
        };
        let one_local = findings(&outlines, &input(&|feature| feature == "B.b"));
        assert_eq!(one_local, local);
        let not_local = findings(&outlines, &input(&|_| false));
        assert_factor(not_local[0].score, local[0].score, 0.25);
    }

    #[test]
    fn a_group_that_shares_only_types_has_half_of_the_score() {
        let mut outlines = corpus();
        let with_call = run(&outlines, None, &none);
        for outline in outlines.iter_mut().take(3) {
            outline.features = vec!["%A".into(), "%B".into(), "%C".into(), "Common.x".into()];
        }
        let types_only = run(&outlines, None, &none);
        assert_eq!(types_only[0].shared, ["%A", "%B", "%C"]);
        assert_factor(types_only[0].score, with_call[0].score, 0.5);
    }

    #[test]
    fn a_lower_group_with_half_of_its_members_in_a_higher_group_is_left_out() {
        // Files a and b have both combinations. `D.d`, `E.e` and `F.f` are also in e and f, so
        // that group has 4 members and the higher score. The `A.a` group has a, b, c and d: half
        // of its members are in the higher group.
        let all = ["A.a", "B.b", "C.c", "D.d", "E.e", "F.f"];
        let mut outlines = vec![
            outline("lib/app/one/a.ex", &all),
            outline("lib/app/two/b.ex", &all),
            outline("lib/app/one/c.ex", &all[..3]),
            outline("lib/app/two/d.ex", &all[..3]),
            outline("lib/app/three/e.ex", &all[3..]),
            outline("lib/app/four/f.ex", &all[3..]),
        ];
        for index in 0..70 {
            outlines.push(outline(&format!("lib/app/other/f{index}.ex"), &[]));
        }
        let found = run(&outlines, None, &none);
        let shared: Vec<_> = found
            .iter()
            .map(|finding| finding.shared.join(" "))
            .collect();
        assert_eq!(shared, ["D.d E.e F.f"]);
    }

    #[test]
    fn agreement_removes_the_lower_factors() {
        let mut outlines = corpus();
        for outline in outlines.iter_mut().take(3) {
            outline.features = vec!["%A".into(), "%B".into(), "%C".into()];
        }
        let two = |outline: &Outline| outline.path.starts_with("lib/app/one/").then_some(7);
        let lowered = findings(
            &outlines,
            &Input {
                area_depth: 3,
                changed: None,
                duplicate_cluster: &none,
                is_local: &|_| false,
            },
        );
        let agreed = findings(
            &outlines,
            &Input {
                area_depth: 3,
                changed: None,
                duplicate_cluster: &two,
                is_local: &|_| false,
            },
        );
        let both_factors = 0.25 * 0.5;
        assert_factor(
            agreed[0].score * both_factors,
            lowered[0].score,
            1.0 + 2.0 * 2.0 / 3.0,
        );
    }
}
