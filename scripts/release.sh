#!/usr/bin/env bash
# Makes a release: sets the version, writes its section in CHANGELOG.md, commits and tags.
# The three always come from one run, so they cannot disagree.
#
# Usage: scripts/release.sh <version>      for example: scripts/release.sh 0.5.0
#
# The script does not push. It prints the push command when it is done.
set -euo pipefail

die() {
  echo "release: $*" >&2
  exit 1
}

version="${1:-}"
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "give a version as the only argument, for example 0.5.0"
tag="v$version"

cd "$(git rev-parse --show-toplevel)"
command -v git-cliff >/dev/null || die "git-cliff is not installed: brew install git-cliff"

[[ "$(git branch --show-current)" == "main" ]] || die "a release is made on main"
[[ -z "$(git status --porcelain)" ]] || die "the working tree has changes: commit them or remove them first"
if git rev-parse --quiet --verify "refs/tags/$tag" >/dev/null; then
  die "the tag $tag exists"
fi

last="$(git describe --tags --abbrev=0 --match 'v[0-9]*' 2>/dev/null || true)"
if [[ -n "$last" ]]; then
  newest="$(printf '%s\n%s\n' "${last#v}" "$version" | sort -V | tail -n 1)"
  [[ "$newest" == "$version" && "$version" != "${last#v}" ]] || die "$version is not after the last version, ${last#v}"
fi

# A release with no entry is a mistake: a commit has no `Changelog:` trailer, or nothing changed.
section="$(git cliff --unreleased --tag "$tag" --strip all 2>/dev/null)"
grep --quiet '^- ' <<<"$section" || die "no commit after ${last:-the start} has a Changelog: trailer"

# From here the files change. If a step fails, they go back to what they were.
changed=1
restore() {
  if [[ -n "$changed" ]]; then
    git checkout --quiet -- Cargo.toml Cargo.lock CHANGELOG.md
    echo "release: stopped. Cargo.toml, Cargo.lock and CHANGELOG.md are as before." >&2
  fi
}
trap restore EXIT

perl -pi -e 'if (!$done && s/^version = "[^"]*"$/version = "'"$version"'"/) { $done = 1 }' Cargo.toml
grep --quiet "^version = \"$version\"$" Cargo.toml || die "could not set the version in Cargo.toml"
git cliff --unreleased --tag "$tag" --prepend CHANGELOG.md 2>/dev/null

# The build updates Cargo.lock. The tests include the check that Cargo.toml and CHANGELOG.md agree.
cargo test --quiet

git commit --quiet --all --message "Release $version"
changed=""
git tag "$tag"

echo "$section"
echo
echo "Committed and tagged $tag. Nothing is pushed."
echo "To change the text of the section: edit CHANGELOG.md, then"
echo "  git commit --amend --all --no-edit && git tag --force $tag"
echo "To publish:"
echo "  git push origin main $tag"
echo "To undo:"
echo "  git tag --delete $tag && git reset --hard HEAD~1"
