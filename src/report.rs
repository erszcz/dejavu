//! Turns clusters into ranked findings, and prints all findings as text or JSON.
//!
//! JSON has locations, scores and the names of modules and functions only. It never has other
//! code text, so a caller can pass it to a model and the scanned code has little room to add
//! instructions.

use std::collections::BTreeSet;
use std::fmt::Write;

use clap::ValueEnum;
use serde::Serialize;

use crate::cluster::{Cluster, Kind};
use crate::unit::Unit;
use crate::{concepts, related};

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Scope {
    /// Every cluster.
    All,
    /// Clusters with members in more than one file.
    CrossFile,
    /// Clusters with members in more than one area.
    CrossArea,
}

pub struct Options {
    pub area_depth: usize,
    pub include_siblings: bool,
    pub scope: Scope,
}

#[derive(Debug, PartialEq, Serialize)]
pub struct Member {
    pub path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub name: String,
    pub tokens: usize,
    /// True when the unit overlaps a changed line. Always false for `scan`.
    pub changed: bool,
}

/// A place in the code that a shared-call or concept finding points to.
#[derive(Debug, PartialEq, Serialize)]
pub struct Place {
    pub path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub name: String,
    /// True when the place overlaps a changed line. Always false for `scan`.
    pub changed: bool,
}

#[derive(Debug, PartialEq, Serialize)]
pub struct Finding {
    pub kind: &'static str,
    pub similarity: f64,
    pub weakest: f64,
    pub score: f64,
    pub labels: Vec<&'static str>,
    pub members: Vec<Member>,
    /// For `identical-modulo-names`: how many distinct substitutions turn one member into another.
    pub differing_tokens: usize,
    /// The substitutions, one value per member. Text output only.
    #[serde(skip)]
    pub differences: Vec<Vec<String>>,
}

#[derive(Debug, Default, PartialEq, Serialize)]
pub struct Summary {
    pub files: usize,
    pub units_total: usize,
    pub units_compared: usize,
    pub min_lines: u32,
    pub min_tokens: usize,
    pub threshold: f64,
    pub identical_groups: usize,
    pub identical_groups_cross_file: usize,
    pub near_pairs: usize,
    pub near_pairs_cross_file: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub changed_units: Option<usize>,
    pub clusters: usize,
    pub clusters_shown: usize,
    /// Shared-call groups: found and shown. Absent when the signal did not run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub related: Option<(usize, usize)>,
    /// Concepts: found and shown. Absent when the signal did not run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub concepts: Option<(usize, usize)>,
}

#[derive(Serialize)]
pub struct Report<'a> {
    pub version: u32,
    pub command: &'a str,
    pub summary: &'a Summary,
    /// Structural duplicates. Absent when the signal did not run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clusters: Option<&'a [Finding]>,
    /// Shared-call groups. Absent when the signal did not run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub related: Option<&'a [related::Finding]>,
    /// Concepts that two or more areas manage. Absent when the signal did not run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub concepts: Option<&'a [concepts::Finding]>,
}

/// Findings in rank order: similarity × mean size × members, highest first. With `changed`, only
/// clusters that have a changed unit are findings.
pub fn findings(
    units: &[Unit],
    clusters: &[Cluster],
    changed: Option<&[bool]>,
    options: &Options,
) -> Vec<Finding> {
    let mut findings: Vec<Finding> = clusters
        .iter()
        .filter(|cluster| changed.is_none_or(|changed| cluster.members.iter().any(|m| changed[*m])))
        .filter_map(|cluster| finding(units, cluster, changed, options))
        .collect();
    findings.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| key(&a.members[0]).cmp(&key(&b.members[0])))
    });
    findings
}

fn key(member: &Member) -> (&str, u32) {
    (&member.path, member.start_line)
}

fn finding(
    units: &[Unit],
    cluster: &Cluster,
    changed: Option<&[bool]>,
    options: &Options,
) -> Option<Finding> {
    let members: Vec<&Unit> = cluster
        .members
        .iter()
        .map(|member| &units[*member])
        .collect();
    let paths: BTreeSet<&str> = members.iter().map(|unit| unit.path.as_str()).collect();
    let names: BTreeSet<&str> = members.iter().map(|unit| unit.name.as_str()).collect();
    let areas: BTreeSet<&str> = paths
        .iter()
        .map(|path| area(path, options.area_depth))
        .collect();

    let clauses = members.iter().all(|unit| !unit.block);
    let siblings = clauses && paths.len() == 1 && names.len() == 1 && !names.contains("?");
    let in_scope = match options.scope {
        Scope::All => true,
        Scope::CrossFile => paths.len() > 1,
        Scope::CrossArea => areas.len() > 1,
    };
    if !in_scope || (siblings && !options.include_siblings) {
        return None;
    }
    let mut labels = Vec::new();
    match (paths.len(), siblings) {
        (1, true) => labels.push("same-function"),
        (1, false) => labels.push("same-file"),
        _ => labels.push("cross-file"),
    }
    if areas.len() > 1 {
        labels.push("cross-area");
    }

    let differences = match cluster.kind {
        Kind::IdenticalModuloNames => differences(&members),
        Kind::Identical | Kind::Near => Vec::new(),
    };
    let mean_tokens =
        members.iter().map(|unit| unit.tokens()).sum::<usize>() as f64 / members.len() as f64;
    Some(Finding {
        kind: cluster.kind.label(),
        similarity: round(cluster.similarity),
        weakest: round(cluster.weakest),
        score: round(cluster.similarity * mean_tokens * members.len() as f64),
        labels,
        members: cluster
            .members
            .iter()
            .zip(&members)
            .map(|(index, unit)| Member {
                path: unit.path.clone(),
                start_line: unit.start_line,
                end_line: unit.end_line,
                name: unit.name.clone(),
                tokens: unit.tokens(),
                changed: changed.is_some_and(|changed| changed[*index]),
            })
            .collect(),
        differing_tokens: differences.len(),
        differences,
    })
}

/// Adds the label `also-related` to each cluster that has two or more members in one shared-call
/// group. When two signals agree on a pair, the finding is stronger.
pub fn mark_agreement(clusters: &mut [Finding], related: &[related::Finding]) {
    for cluster in clusters {
        let agrees = related.iter().any(|group| {
            let in_group = |member: &&Member| {
                group.members.iter().any(|place| {
                    (&place.path, place.start_line) == (&member.path, member.start_line)
                })
            };
            cluster.members.iter().filter(in_group).count() > 1
        });
        if agrees {
            cluster.labels.push("also-related");
        }
    }
}

/// The directory of `path`, cut to its first `depth` components.
pub fn area(path: &str, depth: usize) -> &str {
    let directory = path
        .rsplit_once('/')
        .map_or("", |(directory, _file)| directory);
    match directory.match_indices('/').nth(depth.saturating_sub(1)) {
        Some((end, _)) if depth > 0 => &directory[..end],
        _ if depth > 0 => directory,
        _ => "",
    }
}

/// Positions where the members' strict tokens differ, as distinct tuples in first-seen order.
/// The members have the same loose tokens, so their strict tokens line up one to one.
fn differences(members: &[&Unit]) -> Vec<Vec<String>> {
    let length = members
        .iter()
        .map(|unit| unit.strict.len())
        .min()
        .unwrap_or(0);
    let mut seen = BTreeSet::new();
    (0..length)
        .map(|position| {
            members
                .iter()
                .map(|unit| unit.strict[position].clone())
                .collect::<Vec<_>>()
        })
        .filter(|tuple| tuple.iter().any(|token| token != &tuple[0]))
        .filter(|tuple| seen.insert(tuple.clone()))
        .collect()
}

pub fn round(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

pub fn json(report: &Report) -> String {
    serde_json::to_string_pretty(report).expect("a report is always valid JSON")
}

const SHOWN_DIFFERENCES: usize = 6;

pub fn text(report: &Report) -> String {
    let summary = report.summary;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "dejavu {}: {} files, {} units, {} compared (at least {} lines and {} tokens)",
        report.command,
        summary.files,
        summary.units_total,
        summary.units_compared,
        summary.min_lines,
        summary.min_tokens
    );
    if let Some(changed) = summary.changed_units {
        let _ = writeln!(out, "  {changed} changed units");
    }
    if report.clusters.is_some() {
        let _ = writeln!(
            out,
            "  {} identical groups ({} cross-file), {} near pairs ({} cross-file) at similarity >= {}",
            summary.identical_groups,
            summary.identical_groups_cross_file,
            summary.near_pairs,
            summary.near_pairs_cross_file,
            summary.threshold
        );
        let _ = writeln!(
            out,
            "  {} clusters, {} shown",
            summary.clusters, summary.clusters_shown
        );
    }

    for (index, finding) in report.clusters.unwrap_or_default().iter().enumerate() {
        let _ = writeln!(
            out,
            "\n#{}  {}  {:.0}%{}  {} members  {}  score {:.0}",
            index + 1,
            finding.kind,
            finding.similarity * 100.0,
            if finding.weakest < finding.similarity {
                format!(" (weakest pair {:.0}%)", finding.weakest * 100.0)
            } else {
                String::new()
            },
            finding.members.len(),
            finding.labels.join(" "),
            finding.score
        );
        for member in &finding.members {
            let _ = writeln!(
                out,
                "    {}{}:{}-{}  {}  ({} tokens)",
                if member.changed { "* " } else { "" },
                member.path,
                member.start_line,
                member.end_line,
                member.name,
                member.tokens
            );
        }
        if !finding.differences.is_empty() {
            let shown: Vec<String> = finding
                .differences
                .iter()
                .take(SHOWN_DIFFERENCES)
                .map(|tuple| shown_values(tuple).join(" | "))
                .collect();
            let more = finding.differences.len().saturating_sub(SHOWN_DIFFERENCES);
            let _ = writeln!(
                out,
                "    differs in: {}{}",
                shown.join(";  "),
                if more > 0 {
                    format!(";  and {more} more")
                } else {
                    String::new()
                }
            );
        }
    }
    if let (Some(findings), Some((found, shown))) = (report.related, summary.related) {
        related_text(&mut out, findings, found, shown);
    }
    if let (Some(findings), Some((found, shown))) = (report.concepts, summary.concepts) {
        concepts_text(&mut out, findings, found, shown);
    }
    out
}

const SHOWN_MEMBERS: usize = 8;

fn related_text(out: &mut String, findings: &[related::Finding], found: usize, shown: usize) {
    let _ = writeln!(
        out,
        "\nrelated: {found} groups of units that share rare calls, {shown} shown"
    );
    for (index, finding) in findings.iter().enumerate() {
        let _ = writeln!(
            out,
            "\nR{}  {} files  {} areas  score {:.0}",
            index + 1,
            finding.files,
            finding.areas,
            finding.score
        );
        let _ = writeln!(out, "    shares: {}", finding.shared.join(", "));
        if finding.also_duplicates > 0 {
            let _ = writeln!(
                out,
                "    also structural duplicates: {} of {} members",
                finding.also_duplicates,
                finding.members.len()
            );
        }
        // A changed place is always shown, also when the list is cut.
        let (first, rest) = finding
            .members
            .split_at(SHOWN_MEMBERS.min(finding.members.len()));
        for place in first
            .iter()
            .chain(rest.iter().filter(|place| place.changed))
        {
            let _ = writeln!(out, "    {}", place_text(place));
        }
        let hidden = rest.iter().filter(|place| !place.changed).count();
        if hidden > 0 {
            let _ = writeln!(out, "    and {hidden} more");
        }
    }
}

fn concepts_text(out: &mut String, findings: &[concepts::Finding], found: usize, shown: usize) {
    let _ = writeln!(
        out,
        "\nconcepts: {found} nouns that two or more areas manage, {shown} shown"
    );
    for (index, finding) in findings.iter().enumerate() {
        let _ = writeln!(
            out,
            "\nC{}  {}  shared verbs: {}",
            index + 1,
            finding.noun,
            finding.shared_verbs.join(" ")
        );
        for context in &finding.contexts {
            let path = context
                .members
                .first()
                .map_or("", |place| place.path.as_str());
            let _ = writeln!(
                out,
                "    {}{}  {}: {}",
                if context.members.iter().any(|place| place.changed) {
                    "* "
                } else {
                    ""
                },
                path,
                context.noun,
                context.verbs.join(" ")
            );
        }
    }
}

fn place_text(place: &Place) -> String {
    format!(
        "{}{}:{}-{}  {}",
        if place.changed { "* " } else { "" },
        place.path,
        place.start_line,
        place.end_line,
        place.name
    )
}

const SHOWN_TOKEN_CHARS: usize = 40;

/// The distinct values of one substitution, in first-seen order, each on one line and cut short.
fn shown_values(tuple: &[String]) -> Vec<String> {
    let mut values: Vec<String> = Vec::new();
    for token in tuple {
        let one_line = token.split_whitespace().collect::<Vec<_>>().join(" ");
        let value = if one_line.chars().count() > SHOWN_TOKEN_CHARS {
            format!(
                "{}…",
                one_line.chars().take(SHOWN_TOKEN_CHARS).collect::<String>()
            )
        } else {
            one_line
        };
        if !values.contains(&value) {
            values.push(value);
        }
    }
    values
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shown_values_are_distinct_one_line_and_short() {
        let tuple = ["r", "r", "r0", "a\n  b", &"x".repeat(50)].map(String::from);
        assert_eq!(
            shown_values(&tuple),
            ["r", "r0", "a b", &format!("{}…", "x".repeat(40))]
        );
    }

    fn unit(path: &str, name: &str, strict: &[&str]) -> Unit {
        Unit {
            path: path.to_string(),
            start_line: 1,
            end_line: 9,
            name: name.to_string(),
            block: false,
            strict: strict.iter().map(|token| token.to_string()).collect(),
            strict_hash: 0,
            loose_hash: 0,
            shingles: Vec::new(),
            signature: Vec::new(),
        }
    }

    fn cluster(members: &[usize], kind: Kind, similarity: f64) -> Cluster {
        Cluster {
            members: members.to_vec(),
            kind,
            similarity,
            weakest: similarity,
        }
    }

    const ALL: Options = Options {
        area_depth: 3,
        include_siblings: false,
        scope: Scope::All,
    };

    #[test]
    fn area_is_the_first_components_of_the_directory() {
        assert_eq!(area("lib/app/cases/internal/x.ex", 3), "lib/app/cases");
        assert_eq!(area("lib/app/cases/x.ex", 3), "lib/app/cases");
        assert_eq!(area("lib/app/x.ex", 3), "lib/app");
        assert_eq!(area("x.ex", 3), "");
        assert_eq!(area("lib/app/cases/x.ex", 0), "");
    }

    #[test]
    fn blocks_in_one_file_with_one_name_are_not_clauses_of_one_function() {
        let block = |path: &str| Unit {
            block: true,
            ..unit(path, "setup", &[])
        };
        let units = [
            block("test/a_test.exs"),
            block("test/a_test.exs"),
            block("test/b_test.exs"),
        ];
        let labels = |members: &[usize]| {
            findings(
                &units,
                &[cluster(members, Kind::Identical, 1.0)],
                None,
                &ALL,
            )
            .pop()
            .map(|finding| finding.labels)
        };
        assert_eq!(labels(&[0, 1]), Some(vec!["same-file"]));
        assert_eq!(labels(&[0, 2]), Some(vec!["cross-file"]));
    }

    #[test]
    fn labels_say_where_the_members_are() {
        let units = [
            unit("lib/app/a/x.ex", "f", &[]),
            unit("lib/app/a/x.ex", "f", &[]),
            unit("lib/app/a/x.ex", "g", &[]),
            unit("lib/app/a/y.ex", "f", &[]),
            unit("lib/app/b/z.ex", "f", &[]),
        ];
        let labels = |members: &[usize], options: &Options| {
            findings(&units, &[cluster(members, Kind::Near, 0.9)], None, options)
                .pop()
                .map(|finding| finding.labels)
        };
        let siblings = Options {
            include_siblings: true,
            ..ALL
        };
        assert_eq!(labels(&[0, 1], &ALL), None);
        assert_eq!(labels(&[0, 1], &siblings), Some(vec!["same-function"]));
        assert_eq!(labels(&[0, 2], &ALL), Some(vec!["same-file"]));
        assert_eq!(labels(&[0, 3], &ALL), Some(vec!["cross-file"]));
        assert_eq!(
            labels(&[0, 4], &ALL),
            Some(vec!["cross-file", "cross-area"])
        );
    }

    #[test]
    fn unnamed_units_in_the_same_file_are_not_siblings() {
        let units = [
            unit("lib/app/a/x.erl", "?", &[]),
            unit("lib/app/a/x.erl", "?", &[]),
        ];
        let found = findings(&units, &[cluster(&[0, 1], Kind::Near, 0.9)], None, &ALL);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].labels, ["same-file"]);
    }

    #[test]
    fn scope_filters_clusters() {
        let units = [
            unit("lib/app/a/x.ex", "f", &[]),
            unit("lib/app/a/x.ex", "g", &[]),
            unit("lib/app/a/y.ex", "f", &[]),
            unit("lib/app/b/z.ex", "f", &[]),
        ];
        let clusters = [
            cluster(&[0, 1], Kind::Near, 0.9),
            cluster(&[0, 2], Kind::Near, 0.9),
            cluster(&[0, 3], Kind::Near, 0.9),
        ];
        let count = |scope| findings(&units, &clusters, None, &Options { scope, ..ALL }).len();
        assert_eq!(
            (
                count(Scope::All),
                count(Scope::CrossFile),
                count(Scope::CrossArea)
            ),
            (3, 2, 1)
        );
    }

    #[test]
    fn findings_are_ranked_by_similarity_size_and_members() {
        let units = [
            unit("a.ex", "small", &["t"; 10]),
            unit("b.ex", "small", &["t"; 10]),
            unit("c.ex", "big", &["t"; 100]),
            unit("d.ex", "big", &["t"; 100]),
        ];
        let clusters = [
            cluster(&[0, 1], Kind::Identical, 1.0),
            cluster(&[2, 3], Kind::Near, 0.8),
        ];
        let ranked = findings(&units, &clusters, None, &ALL);
        let scores: Vec<_> = ranked
            .iter()
            .map(|f| (f.members[0].name.as_str(), f.score))
            .collect();
        assert_eq!(scores, [("big", 160.0), ("small", 20.0)]);
    }

    #[test]
    fn with_changes_only_clusters_with_a_changed_unit_are_findings() {
        let units = [
            unit("a.ex", "f", &[]),
            unit("b.ex", "f", &[]),
            unit("c.ex", "g", &[]),
            unit("d.ex", "g", &[]),
        ];
        let clusters = [
            cluster(&[0, 1], Kind::Near, 0.9),
            cluster(&[2, 3], Kind::Near, 0.9),
        ];
        let changed = [false, false, false, true];
        let found = findings(&units, &clusters, Some(&changed), &ALL);
        let members: Vec<Vec<_>> = found
            .iter()
            .map(|f| {
                f.members
                    .iter()
                    .map(|m| (m.path.as_str(), m.changed))
                    .collect()
            })
            .collect();
        assert_eq!(members, [[("c.ex", false), ("d.ex", true)]]);
    }

    #[test]
    fn differences_are_the_distinct_substitutions() {
        let units = [
            unit("a.ex", "f", &["def", "FUNC", "x", "Textract", "x", "1"]),
            unit("b.ex", "g", &["defp", "FUNC", "y", "Azure", "y", "1"]),
        ];
        let found = findings(
            &units,
            &[cluster(&[0, 1], Kind::IdenticalModuloNames, 1.0)],
            None,
            &ALL,
        );
        let expected: Vec<Vec<String>> = [["def", "defp"], ["x", "y"], ["Textract", "Azure"]]
            .iter()
            .map(|tuple| tuple.iter().map(|token| token.to_string()).collect())
            .collect();
        assert_eq!(found[0].differences, expected);
        assert_eq!(found[0].differing_tokens, 3);
    }

    #[test]
    fn a_cluster_with_two_members_in_one_related_group_is_marked() {
        let units = [
            unit("a.ex", "f", &[]),
            unit("b.ex", "f", &[]),
            unit("c.ex", "g", &[]),
            unit("d.ex", "g", &[]),
        ];
        let clusters = [
            cluster(&[0, 1], Kind::Near, 0.9),
            cluster(&[2, 3], Kind::Near, 0.9),
        ];
        let mut found = findings(&units, &clusters, None, &ALL);
        let place = |path: &str| Place {
            path: path.to_string(),
            start_line: 1,
            end_line: 9,
            name: "f".to_string(),
            changed: false,
        };
        let group = related::Finding {
            shared: Vec::new(),
            files: 3,
            areas: 2,
            score: 1.0,
            also_duplicates: 2,
            members: vec![place("a.ex"), place("b.ex"), place("c.ex")],
        };
        mark_agreement(&mut found, &[group]);
        let labels: Vec<_> = found
            .iter()
            .map(|f| (f.members[0].path.as_str(), f.labels.clone()))
            .collect();
        assert_eq!(
            labels,
            [
                ("a.ex", vec!["cross-file", "also-related"]),
                ("c.ex", vec!["cross-file"])
            ]
        );
    }

    #[test]
    fn json_has_no_token_text() {
        let units = [
            unit("a.ex", "f", &["secret_a"]),
            unit("b.ex", "g", &["secret_b"]),
        ];
        let found = findings(
            &units,
            &[cluster(&[0, 1], Kind::IdenticalModuloNames, 1.0)],
            None,
            &ALL,
        );
        let summary = Summary::default();
        let rendered = json(&Report {
            version: 1,
            command: "scan",
            summary: &summary,
            clusters: Some(&found),
            related: None,
            concepts: None,
        });
        assert!(!rendered.contains("secret"), "{rendered}");
        assert!(rendered.contains("\"differing_tokens\": 1"), "{rendered}");
    }
}
