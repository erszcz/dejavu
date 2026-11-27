# AGENTS.md

- Before a commit, run `cargo fmt`, `cargo clippy --all-targets` and `cargo test`.
- A commit that adds or changes a feature, a default or the format of the report has a
  `Changelog:` trailer. [README.md](README.md), "Releases", has the rule and the values.
- A feature commit does not change `version` in `Cargo.toml` or `CHANGELOG.md`. The release commit
  changes them.
