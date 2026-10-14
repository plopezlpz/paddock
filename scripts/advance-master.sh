#!/usr/bin/env bash
# Moves master to a released commit, fast-forward only. Run by the Release
# workflow once the release is published, so master (which TPM follows) never
# names a version whose release doesn't exist yet.
# Usage: scripts/advance-master.sh SHA [REMOTE]   (REMOTE defaults to origin; run inside the repo)
set -euo pipefail

[ $# -ge 1 ] && [ $# -le 2 ] || { echo 'usage: scripts/advance-master.sh SHA [REMOTE]' >&2; exit 2; }
sha=$1 remote=${2:-origin}

git fetch -q "$remote" master
head=$(git rev-parse FETCH_HEAD)
if git merge-base --is-ancestor "$sha" "$head"; then
  echo "master already contains $sha"
elif git merge-base --is-ancestor "$head" "$sha"; then
  # No --force: if master moved meanwhile, the push is refused, not overwritten.
  git push -q "$remote" "$sha:refs/heads/master"
  echo "master is now $sha"
else
  echo "::error::$sha is not on master's line (master is $head); push master by hand" >&2
  exit 1
fi
