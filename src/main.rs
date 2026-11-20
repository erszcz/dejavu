//! dejavu finds existing code that does the same job as other code.
//!
//! `scan` reports every finding. `check` reports only the findings that include changed code.
//! Both only read: they parse source, they never run it, and they write nothing.
//!
//! There are three signals, for three kinds of overlap:
//!
//! - `duplicates`: units with the same structure (`cluster`);
//! - `related`: units that share a rare combination of calls (`related`);
//! - `concepts`: nouns that two or more areas manage in their public functions (`concepts`).

mod changes;
mod cluster;
mod concepts;
mod fingerprint;
mod index;
mod lang;
mod related;
mod report;
mod unit;

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use rayon::prelude::*;

use changes::Changes;
use report::{Options, Report, Scope, Summary};
use unit::{ApiFunction, Extractor, FileUnits, Limits, Outline, Unit, Wanted};

#[derive(Parser)]
#[command(name = "dejavu", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Report clusters of similar code in the given paths.
    Scan(Scan),
    /// Report only the clusters that include changed code.
    Check(Check),
}

#[derive(Args)]
struct Scan {
    #[command(flatten)]
    common: Common,
}

#[derive(Args)]
struct Check {
    #[command(flatten)]
    common: Common,
    /// Changed code is the working tree against this git revision, plus untracked files.
    #[arg(long, conflicts_with = "diff", required_unless_present = "diff")]
    base: Option<String>,
    /// Changed code is the added lines of this unified diff. Its paths are relative to the root.
    #[arg(long)]
    diff: Option<PathBuf>,
    /// Exit with status 1 when there is a finding.
    #[arg(long)]
    fail_on_findings: bool,
}

#[derive(Args)]
struct Common {
    /// Files or directories to index, relative to the root.
    #[arg(default_value = ".")]
    paths: Vec<PathBuf>,
    /// The directory that reported paths are relative to. For `check`, the repository root.
    #[arg(long, default_value = ".")]
    root: PathBuf,
    /// The smallest unit to compare, in lines.
    #[arg(long, default_value_t = 6)]
    min_lines: u32,
    /// The smallest unit to compare, in tokens.
    #[arg(long, default_value_t = 60)]
    min_tokens: usize,
    /// The lowest similarity (0 to 1) of a pair.
    #[arg(long, default_value_t = 0.8)]
    threshold: f64,
    /// An area is the first N directories of a path. Members in two areas get `cross-area`.
    #[arg(long, default_value_t = 3)]
    area_depth: usize,
    /// Also report clusters that are only clauses of one function in one file.
    #[arg(long)]
    include_siblings: bool,
    #[arg(long, value_enum, default_value_t = Scope::All)]
    scope: Scope,
    #[arg(long, value_enum, default_value_t = Format::Text)]
    format: Format,
    /// Show only the N highest-ranked findings of each signal.
    #[arg(long)]
    top: Option<usize>,
    /// The signals to run. Give the option more than once, or a list with commas.
    #[arg(long = "signal", value_enum, value_delimiter = ',', default_values_t = [Signal::Duplicates])]
    signals: Vec<Signal>,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Signal {
    /// Units with the same structure.
    Duplicates,
    /// Units that share a rare combination of calls.
    Related,
    /// Nouns that two or more areas manage in their public functions.
    Concepts,
    /// All three.
    All,
}

#[derive(Clone, Copy, ValueEnum)]
enum Format {
    Text,
    Json,
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("dejavu: {error:#}");
            ExitCode::from(2)
        }
    }
}

fn run(cli: Cli) -> Result<ExitCode> {
    let (command, common, changes, fail_on_findings) = match &cli.command {
        Command::Scan(scan) => ("scan", &scan.common, None, false),
        Command::Check(check) => {
            let changes = match (&check.base, &check.diff) {
                (Some(base), _) => Changes::from_git(&check.common.root, base)?,
                (None, Some(diff)) => Changes::from_diff(
                    &std::fs::read_to_string(diff)
                        .with_context(|| format!("could not read {}", diff.display()))?,
                ),
                (None, None) => bail!("check needs --base or --diff"),
            };
            (
                "check",
                &check.common,
                Some(changes),
                check.fail_on_findings,
            )
        }
    };
    if !(0.0..=1.0).contains(&common.threshold) {
        bail!("--threshold must be between 0 and 1");
    }

    let runs = |signal| common.signals.contains(&signal) || common.signals.contains(&Signal::All);
    let wanted = Wanted {
        features: runs(Signal::Related),
        api: runs(Signal::Concepts),
    };

    let files = files(&common.root, &common.paths)?;
    let limits = Limits {
        min_lines: common.min_lines,
        min_tokens: common.min_tokens,
    };
    let extracted = extract(&files, &limits, wanted)?;
    let units = &extracted.units;
    let top = common.top.unwrap_or(usize::MAX);

    // The structural check always runs: the shared-call signal says where the two agree.
    let pairs = index::similar_pairs(units, common.threshold);
    let clusters = cluster::clusters(units, &pairs);
    let changed = changes.as_ref().map(|changes| {
        let touches = |u: &Unit| changes.touches(&u.path, u.start_line, u.end_line);
        units.iter().map(touches).collect::<Vec<bool>>()
    });
    let options = Options {
        area_depth: common.area_depth,
        include_siblings: common.include_siblings,
        scope: common.scope,
    };
    let mut findings = report::findings(units, &clusters, changed.as_deref(), &options);
    let found = findings.len();

    // With outlines, the count of changed units covers units of every size.
    let changed_units = changes.as_ref().map(|changes| {
        if wanted.features {
            let touches = |o: &&Outline| changes.touches(&o.path, o.start_line, o.end_line);
            extracted.outlines.iter().filter(touches).count()
        } else {
            changed.iter().flatten().filter(|c| **c).count()
        }
    });

    let related = runs(Signal::Related).then(|| {
        let cluster_of: HashMap<(&str, u32), usize> = clusters
            .iter()
            .enumerate()
            .flat_map(|(index, cluster)| cluster.members.iter().map(move |member| (index, member)))
            .map(|(index, member)| {
                (
                    (units[*member].path.as_str(), units[*member].start_line),
                    index,
                )
            })
            .collect();
        let changed = changes.as_ref().map(|changes| {
            let touches = |o: &Outline| changes.touches(&o.path, o.start_line, o.end_line);
            extracted
                .outlines
                .iter()
                .map(touches)
                .collect::<Vec<bool>>()
        });
        let mut found = related::findings(
            &extracted.outlines,
            common.area_depth,
            changed.as_deref(),
            &|outline: &Outline| {
                cluster_of
                    .get(&(outline.path.as_str(), outline.start_line))
                    .copied()
            },
        );
        report::mark_agreement(&mut findings, &found);
        let count = found.len();
        found.truncate(top);
        (count, found)
    });
    findings.truncate(top);
    let concepts = runs(Signal::Concepts).then(|| {
        let changed = changes.as_ref().map(|changes| {
            let touches = |f: &ApiFunction| changes.touches(&f.path, f.start_line, f.end_line);
            extracted.api.iter().map(touches).collect::<Vec<bool>>()
        });
        let mut found = concepts::findings(&extracted.api, changed.as_deref());
        let count = found.len();
        found.truncate(top);
        (count, found)
    });

    let (identical_groups, identical_groups_cross_file) = identical_groups(units);
    let near: Vec<_> = pairs
        .iter()
        .filter(|pair| units[pair.a].loose_hash != units[pair.b].loose_hash)
        .collect();
    let duplicates = runs(Signal::Duplicates);
    let summary = Summary {
        files: files.len(),
        units_total: extracted.total,
        units_compared: units.len(),
        min_lines: common.min_lines,
        min_tokens: common.min_tokens,
        threshold: common.threshold,
        identical_groups,
        identical_groups_cross_file,
        near_pairs: near.len(),
        near_pairs_cross_file: near
            .iter()
            .filter(|p| units[p.a].path != units[p.b].path)
            .count(),
        changed_units,
        clusters: found,
        clusters_shown: findings.len(),
        related: related.as_ref().map(|(count, shown)| (*count, shown.len())),
        concepts: concepts
            .as_ref()
            .map(|(count, shown)| (*count, shown.len())),
    };
    let report = Report {
        version: 1,
        command,
        summary: &summary,
        clusters: duplicates.then_some(findings.as_slice()),
        related: related.as_ref().map(|(_, shown)| shown.as_slice()),
        concepts: concepts.as_ref().map(|(_, shown)| shown.as_slice()),
    };
    match common.format {
        Format::Text => print!("{}", report::text(&report)),
        Format::Json => println!("{}", report::json(&report)),
    }
    let any = (duplicates && found > 0)
        || related.is_some_and(|(count, _)| count > 0)
        || concepts.is_some_and(|(count, _)| count > 0);
    Ok(if fail_on_findings && any {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    })
}

/// Source files below the paths, as `(path to read, path to report)`, sorted by the reported path.
/// The walk honours `.gitignore`.
fn files(root: &Path, paths: &[PathBuf]) -> Result<Vec<(PathBuf, String)>> {
    let mut found = BTreeSet::new();
    for path in paths {
        let start = if path.is_absolute() {
            path.clone()
        } else {
            root.join(path)
        };
        if !start.exists() {
            bail!("no such path: {}", start.display());
        }
        for entry in ignore::WalkBuilder::new(&start).build() {
            let entry = entry?;
            let full = entry.path();
            if entry.file_type().is_some_and(|kind| kind.is_file())
                && lang::for_path(full).is_some()
            {
                let reported = full
                    .strip_prefix(root)
                    .unwrap_or(full)
                    .to_string_lossy()
                    .replace('\\', "/");
                found.insert((reported, full.to_path_buf()));
            }
        }
    }
    Ok(found
        .into_iter()
        .map(|(reported, full)| (full, reported))
        .collect())
}

/// What the files give, in file order.
struct Extracted {
    /// The number of units before the size limits.
    total: usize,
    units: Vec<Unit>,
    outlines: Vec<Outline>,
    api: Vec<ApiFunction>,
}

fn extract(files: &[(PathBuf, String)], limits: &Limits, wanted: Wanted) -> Result<Extracted> {
    for spec in lang::ALL {
        Extractor::new(spec)?;
    }
    let per_file: Vec<FileUnits> = files
        .par_iter()
        .map_init(
            HashMap::<&'static str, Extractor>::new,
            |extractors, (full, reported)| {
                let spec = lang::for_path(full).expect("files are filtered by language");
                let extractor = extractors.entry(spec.name).or_insert_with(|| {
                    Extractor::new(spec)
                        .expect("the spec was checked above")
                        .with(wanted)
                });
                match std::fs::read(full) {
                    Ok(source) => extractor.extract(reported, &source, limits),
                    Err(error) => {
                        eprintln!("dejavu: skipped {reported}: {error}");
                        FileUnits::default()
                    }
                }
            },
        )
        .collect();
    let mut extracted = Extracted {
        total: 0,
        units: Vec::new(),
        outlines: Vec::new(),
        api: Vec::new(),
    };
    for file in per_file {
        extracted.total += file.total;
        extracted.units.extend(file.units);
        extracted.outlines.extend(file.outlines);
        extracted.api.extend(file.api);
    }
    Ok(extracted)
}

/// Groups of units with the same loose tokens: how many, and how many of them span files.
fn identical_groups(units: &[Unit]) -> (usize, usize) {
    let mut groups: HashMap<u64, Vec<&Unit>> = HashMap::new();
    for unit in units {
        groups.entry(unit.loose_hash).or_default().push(unit);
    }
    let groups: Vec<_> = groups
        .into_values()
        .filter(|group| group.len() > 1)
        .collect();
    let cross_file = groups
        .iter()
        .filter(|group| group.iter().any(|unit| unit.path != group[0].path))
        .count();
    (groups.len(), cross_file)
}
