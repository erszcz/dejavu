//! dejavu finds existing code that does the same job as other code.
//!
//! `scan` reports every cluster of similar units. `check` reports only the clusters that include
//! changed code. Both only read: they parse source, they never run it, and they write nothing.

mod changes;
mod cluster;
mod fingerprint;
mod index;
mod lang;
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
use unit::{Extractor, FileUnits, Limits, Unit};

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
    /// Show only the N highest-ranked clusters.
    #[arg(long)]
    top: Option<usize>,
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

    let files = files(&common.root, &common.paths)?;
    let limits = Limits {
        min_lines: common.min_lines,
        min_tokens: common.min_tokens,
    };
    let (units_total, units) = extract(&files, &limits)?;

    let pairs = index::similar_pairs(&units, common.threshold);
    let clusters = cluster::clusters(&units, &pairs);
    let changed: Option<Vec<bool>> = changes.as_ref().map(|changes| {
        units
            .iter()
            .map(|u| changes.touches(&u.path, u.start_line, u.end_line))
            .collect()
    });
    let options = Options {
        area_depth: common.area_depth,
        include_siblings: common.include_siblings,
        scope: common.scope,
    };
    let mut findings = report::findings(&units, &clusters, changed.as_deref(), &options);
    let found = findings.len();
    findings.truncate(common.top.unwrap_or(usize::MAX));

    let (identical_groups, identical_groups_cross_file) = identical_groups(&units);
    let near: Vec<_> = pairs
        .iter()
        .filter(|pair| units[pair.a].loose_hash != units[pair.b].loose_hash)
        .collect();
    let summary = Summary {
        files: files.len(),
        units_total,
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
        changed_units: changed
            .as_ref()
            .map(|changed| changed.iter().filter(|c| **c).count()),
        clusters: found,
        clusters_shown: findings.len(),
    };
    let report = Report {
        version: 1,
        command,
        summary: &summary,
        clusters: &findings,
    };
    match common.format {
        Format::Text => print!("{}", report::text(&report)),
        Format::Json => println!("{}", report::json(&report)),
    }
    Ok(if fail_on_findings && found > 0 {
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

/// All units of the files, in file order, and the number of units before the size limits.
fn extract(files: &[(PathBuf, String)], limits: &Limits) -> Result<(usize, Vec<Unit>)> {
    for spec in lang::ALL {
        Extractor::new(spec)?;
    }
    let per_file: Vec<FileUnits> = files
        .par_iter()
        .map_init(
            HashMap::<&'static str, Extractor>::new,
            |extractors, (full, reported)| {
                let spec = lang::for_path(full).expect("files are filtered by language");
                let extractor = extractors
                    .entry(spec.name)
                    .or_insert_with(|| Extractor::new(spec).expect("the spec was checked above"));
                match std::fs::read(full) {
                    Ok(source) => extractor.extract(reported, &source, limits),
                    Err(error) => {
                        eprintln!("dejavu: skipped {reported}: {error}");
                        FileUnits {
                            total: 0,
                            units: Vec::new(),
                        }
                    }
                }
            },
        )
        .collect();
    let total = per_file.iter().map(|file| file.total).sum();
    Ok((
        total,
        per_file.into_iter().flat_map(|file| file.units).collect(),
    ))
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
