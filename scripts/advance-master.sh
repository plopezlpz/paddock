#!/usr/bin/env bash
# Moves master to a released commit, fast-forward only. Run by the Release
# workflow once the release is published, so master (which TPM follows) never
# names a version whose release doesn't exist yet.
# With --check it moves nothing, and says what it would do: the workflow asks
# before it publishes anything, so a release that cannot reach master is never
# published. It prints "advance" if master can be fast-forwarded to REV (or
# already has it), "backport" if REV is off master's line with a lower version
# than master's (cut from a release branch: master is left alone), and fails
# otherwise.
# Usage: scripts/advance-master.sh [--check] REV [REMOTE]   (REMOTE defaults to origin; run inside the repo)
set -euo pipefail

check=
if [ "${1:-}" = --check ]; then
  check=1
  shift
fi
[ $# -ge 1 ] && [ $# -le 2 ] || { echo 'usage: scripts/advance-master.sh [--check] REV [REMOTE]' >&2; exit 2; }
remote=${2:-origin}
# The commit, even if REV names an annotated tag.
sha=$(git rev-parse --verify "$1^{commit}")

version() { # version COMMIT: the [package] version in COMMIT's Cargo.toml.
  git show "$1:Cargo.toml" 2>/dev/null | awk -F'"' '/^\[/ { p = ($0 == "[package]") } p && /^version *=/ { print $2; exit }'
}

numeric() { case "$1" in '' | *[!0-9]*) return 1 ;; esac; }

precedes() { # precedes A B: whether version A comes before B (semver precedence; build metadata ignored).
  local a=${1%%+*} b=${2%%+*} i x y
  local -a p q
  # Major, minor, patch: by value.
  IFS=. read -r -a p <<<"${a%%-*}"
  IFS=. read -r -a q <<<"${b%%-*}"
  for i in 0 1 2; do
    x=${p[i]:-0} y=${q[i]:-0}
    numeric "$x" && numeric "$y" || return 1
    [ "$x" -eq "$y" ] || { [ "$x" -lt "$y" ]; return; }
  done
  # The same: a pre-release comes before the release.
  case "$a" in *-*) a=${a#*-} ;; *) return 1 ;; esac
  case "$b" in *-*) b=${b#*-} ;; *) return 0 ;; esac
  # Two pre-releases, field by field: numbers by value and before words,
  # words in ASCII order; when all fields agree, fewer fields come first.
  IFS=. read -r -a p <<<"$a"
  IFS=. read -r -a q <<<"$b"
  for ((i = 0; ; i++)); do
    [ "$i" -lt "${#p[@]}" ] || { [ "$i" -lt "${#q[@]}" ]; return; }
    [ "$i" -lt "${#q[@]}" ] || return 1
    x=${p[i]} y=${q[i]}
    [ "$x" != "$y" ] || continue
    if numeric "$x" && numeric "$y"; then
      [ "$x" -lt "$y" ]
    elif numeric "$x" || numeric "$y"; then
      numeric "$x"
    else
      [ "$(printf '%s\n%s\n' "$x" "$y" | LC_ALL=C sort | head -n 1)" = "$x" ]
    fi
    return
  done
}

git fetch -q "$remote" master
head=$(git rev-parse FETCH_HEAD)
if git merge-base --is-ancestor "$sha" "$head"; then
  [ -z "$check" ] || { echo advance; exit 0; }
  echo "master already contains $sha"
elif git merge-base --is-ancestor "$head" "$sha"; then
  [ -z "$check" ] || { echo advance; exit 0; }
  # No --force: if master moved meanwhile, the push is refused, not overwritten.
  # (A push with the workflow token is also refused if the commits change
  # .github/workflows: those must reach master before the tag.)
  git push -q "$remote" "$sha:refs/heads/master"
  echo "master is now $sha"
else
  if [ -n "$check" ]; then
    v=$(version "$sha") m=$(version "$head")
    if [ -n "$v" ] && [ -n "$m" ] && precedes "$v" "$m"; then
      echo "::notice::$v is older than master's $m: a backport, so master is left alone" >&2
      echo backport
      exit 0
    fi
  fi
  echo "::error::master ($head) has diverged from $sha; merge $sha into master, then push master" >&2
  exit 1
fi
