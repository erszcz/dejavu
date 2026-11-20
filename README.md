# dejavu

dejavu finds existing code that does the same job as other code. It has three signals, for three
kinds of overlap:

| Signal | Finds | Example |
|---|---|---|
| `duplicates` | Units with the same structure. A copy with other variable names, numbers or strings is still found. | The same clause in ten schema modules. |
| `related` | Units that share a rare combination of calls, also when their code has a different shape. | Two contexts that each write their own "schedule and wait" around the same scheduler calls. |
| `concepts` | Nouns that two or more areas each manage in their public functions. | `create_folder` in one context and `create_journal_folder` in another. |

`duplicates` is the default. The other two give candidates for a reader to judge: a shared
mechanism or a shared name is not always a mistake.

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

Run all three signals:

```bash
dejavu scan lib --root /path/to/repo --signal all --top 10
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
| `--top` | all | Show only the N highest-ranked findings of each signal. |
| `--signal` | `duplicates` | `duplicates`, `related`, `concepts` or `all`. Repeat the option, or give a list with commas. |

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

## The `related` signal

1. **Features.** Each unit of any size gives the functions that it calls in other modules and the
   types that it builds, with full module names. For Elixir these are remote calls
   (`Module.function`) and struct literals (`%Module`), with the file's aliases resolved.
2. **Rare features.** A feature counts when it is in 3 to 60 files. A feature in more files is
   common vocabulary, for example `Enum.map`.
3. **Groups.** A group is a set of at least 3 rare features that units in at least 3 files and
   2 areas all have. One shared call is reuse and is not reported.
4. **Rank.** The rarity of the shared features × files × areas.

A group that the `duplicates` signal reports already is left out. A lower group that mostly
repeats a higher one is left out too.

## The `concepts` signal

1. **Public functions.** The public entry file of a directory gives its public function names.
   For Elixir this is the file with the name of its directory (`folders/folders.ex`), and the
   names are those of `def`, `defdelegate`, `defmacro` and `@callback`.
2. **Verb and noun.** A name is split into a verb and a noun: `create_journal_folders` is
   `create` and `journal_folder`. Only names that start with a known verb count (create, update,
   delete, remove, get, fetch, list, move, rename, archive, restore, duplicate, copy).
3. **Findings.** An area manages a noun when it has at least 3 verbs on it. A finding is a last
   word of a noun (`folder`) that two or more areas manage, with at least 2 verbs in common.

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

With `--signal`, the report has one key for each signal that ran: `clusters`, `related` and
`concepts`. A signal that did not run has no key.

```json
{
  "related": [
    {
      "shared": ["Process.alias", "Process.unalias", "%App.Scheduler.Request"],
      "files": 4,
      "areas": 4,
      "score": 285.0,
      "members": [
        { "path": "lib/a.ex", "start_line": 97, "end_line": 151, "name": "schedule_and_wait", "changed": false }
      ]
    }
  ],
  "concepts": [
    {
      "noun": "folder",
      "shared_verbs": ["create", "delete", "get", "list", "update"],
      "contexts": [
        {
          "area": "lib/app/folders",
          "noun": "folder",
          "verbs": ["create", "delete", "get", "list", "update"],
          "members": [
            { "path": "lib/app/folders/folders.ex", "start_line": 12, "end_line": 12, "name": "create_folder", "changed": false }
          ]
        }
      ]
    }
  ]
}
```

The JSON has locations, scores and the names of modules and functions only. It has no other code
text, so the scanned code has little room to add instructions for a model that reads the report.

## Limits

- Elixir only. A language is one file in `src/lang/`: node kinds, one query and one small hook.
- `duplicates` is structural. Two pieces of code that do the same job in different ways are not
  found by it. `related` finds them only when they share at least 3 rare calls.
- `related` and `concepts` are untuned. Their limits are constants in `src/related.rs` and
  `src/concepts.rs`, taken from one codebase.
- `concepts` reads names only. It cannot tell a second implementation of a concept from two
  different things with the same name.
- dejavu reports that code is similar and where it is. It does not know if the other code can be
  called from the new place, for example across a context boundary.
- There is no cache. A run on 4,400 files and 750,000 lines takes about 1 second.
  `Extractor::extract` reads only one file's content, so a cache by content hash can go in front of
  it later.

`reference/dups.py` is the Python probe that the design came from. Its counts on the same code are
the check for dejavu's counts.
