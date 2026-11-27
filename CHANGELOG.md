# Changelog

All notable changes to dejavu are in this file. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). The versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). Before 1.0.0, a minor version can
change a default or the format of the report.

## 0.4.0 - 2026-11-27

### Added

- Elixir: an ExUnit block is a unit. A `test`, `property`, `setup` or `setup_all` call with a `do`
  block is compared like a function clause. `describe` is not a unit: each test in it is compared
  on its own.
- The name of a block in the report is what it is and its title, for example
  `test creates a user`. Only letters, digits, spaces and `_-./:#` of a title stay.
- This change log, and a version for each release.

### Changed

- A scan of a directory with tests reports many more clusters, also clusters that are only `near`.
  Tests in one file often share their first steps.
- Two blocks in one file are never clauses of one function, so they show without
  `--include-siblings`.

## 0.3.0 - 2026-11-23

### Added

- Erlang support for the `duplicates` signal: `erl`, `hrl`, `escript` and `yrl` files.

## 0.2.0 - 2026-11-20

### Added

- The `related` signal: units that share a rare combination of calls, also when their code has a
  different shape.
- The `concepts` signal: nouns that two or more areas each manage in their public functions.
- `--signal` selects the signals to run. `duplicates` stays the default.
- The report shows where two signals agree. A related group has `also_duplicates`, and a
  duplicates cluster has the label `also-related`.

### Changed

- The score of a related group uses what a reading of judged groups showed. Agreement with the
  duplicates signal counts more. A group with no feature from the scanned code, or with types
  only, counts less.

## 0.1.0 - 2026-11-20

### Added

- `scan` reports ranked clusters of similar units.
- `check` reports only the clusters that include changed code, from a git revision (`--base`) or
  from a unified diff (`--diff`).
- Text and JSON output. The JSON has locations, names and scores, and no code text.
- Elixir support. The unit is the function clause.
