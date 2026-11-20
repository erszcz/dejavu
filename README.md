# dejavu

dejavu finds existing code that does the same job as other code. It compares code by structure, so
a copy with other variable names, numbers or strings is still found.

It has three uses:

- clean up duplicates that are already in a codebase (`scan`);
- check new code in a pull request against the rest of the codebase (`check --diff`);
- let a coding agent check its uncommitted work at a checkpoint (`check --base`).

dejavu only reads. It parses source with tree-sitter, never runs it, uses no network and writes no
files.

## Build

```bash
cargo build --release
```

The binary is `target/release/dejavu`.

## Commands

Report every cluster of similar code below `lib/`, highest-ranked first:

```bash
dejavu scan lib --root /path/to/repo --scope cross-file --top 25
```

Report only the clusters that include code changed since a git revision. Untracked files count as
changed:

```bash
dejavu check lib --root /path/to/repo --base HEAD
```

Report only the clusters that include lines that a unified diff adds:

```bash
dejavu check lib --root /path/to/repo --diff full.diff --format json
```

`check` exits with status 0 when it has findings, unless `--fail-on-findings` is given. Status 2 is
an error.

| Option | Default | Meaning |
|---|---|---|
| `--root` | `.` | Reported paths are relative to this directory. For `check`, the repository root. |
| `--min-lines` | 6 | The smallest unit to compare, in lines. |
| `--min-tokens` | 60 | The smallest unit to compare, in tokens. |
| `--threshold` | 0.8 | The lowest similarity of a pair. |
| `--scope` | `all` | `cross-file` or `cross-area` keep only clusters that span files or areas. |
| `--area-depth` | 3 | An area is the first N directories of a path. |
| `--include-siblings` | off | Also report clusters that are only clauses of one function. |
| `--format` | `text` | `text` or `json`. |
| `--top` | all | Show only the N highest-ranked clusters. |

## How it works

1. **Units.** A language spec gives a tree-sitter query for the unit of comparison. For Elixir the
   unit is the function clause (`def`, `defp`, `defmacro`, `defmacrop`).
2. **Tokens.** Each unit becomes its leaf tokens, with comments removed. Two levels are kept:
   - strict: only the unit's own name is replaced;
   - loose: variable names, numbers and string contents are replaced too. Called functions,
     module names and atoms stay, because they are what the code does.
3. **Fingerprint.** The loose tokens give a set of 4-token shingles and a MinHash signature.
4. **Pairs.** Signatures give candidate pairs. The exact Jaccard similarity of the shingle sets
   decides, so the reported similarity is exact.
5. **Clusters.** Similar pairs are grouped. Grouping is transitive, so each cluster also shows its
   weakest pair. A low weakest pair means a chain, not a tight group.
6. **Rank.** Similarity × mean size × number of members.

A cluster has one of three kinds:

- `identical`: the same tokens, apart from the units' own names;
- `identical-modulo-names`: the same after names, numbers and strings are replaced. The text report
  lists what differs: these are the parameters that a shared function would take;
- `near`: similar, at or above the threshold.

## JSON

```json
{
  "version": 1,
  "command": "check",
  "summary": { "files": 4411, "units_total": 49836, "units_compared": 14239, "changed_units": 1, "clusters": 1, "clusters_shown": 1 },
  "clusters": [
    {
      "kind": "near",
      "similarity": 0.96,
      "weakest": 0.94,
      "score": 617.0,
      "labels": ["cross-file", "cross-area"],
      "differing_tokens": 0,
      "members": [
        { "path": "lib/a.ex", "start_line": 188, "end_line": 213, "name": "process_table", "tokens": 214, "changed": false },
        { "path": "lib/b.ex", "start_line": 2, "end_line": 27, "name": "copied_table", "tokens": 214, "changed": true }
      ]
    }
  ]
}
```

The JSON has locations, names and scores only, and no code text. A caller can give it to a model,
and the scanned code cannot add instructions to it.

## Limits

- Elixir only. A language is one file in `src/lang/`: node kinds, one query and one small hook.
- Similarity is structural. Two pieces of code that do the same job in different ways are not
  found.
- dejavu reports that code is similar and where it is. It does not know if the other code can be
  called from the new place, for example across a context boundary.
- There is no cache. A run on 4,400 files and 750,000 lines takes about 1 second.
  `Extractor::extract` reads only one file's content, so a cache by content hash can go in front of
  it later.

`reference/dups.py` is the Python probe that the design came from. Its counts on the same code are
the check for dejavu's counts.
