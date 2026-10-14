#!/usr/bin/env bash
# Updates the Homebrew tap's Formula/paddock.rb for a published release. Run by
# the Release workflow after it publishes; works by hand too.
# Usage: scripts/update-tap.sh VERSION
# Env: HOMEBREW_TAP_DEPLOY_KEY: the private half of a write deploy key for the tap
#        (without it nothing is pushed and the script says so)
#      TAP_REMOTE: the tap's git URL or path (default git@github.com:plopezlpz/homebrew-tap.git);
#        a path needs no key, for tests
set -euo pipefail

[ $# -eq 1 ] || { echo 'usage: scripts/update-tap.sh VERSION' >&2; exit 2; }
v=${1#v}
here=$(CDPATH='' cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
remote=${TAP_REMOTE:-git@github.com:plopezlpz/homebrew-tap.git}

case "$v" in
  *-*) echo "paddock: $v is a pre-release; the tap is not updated"; exit 0 ;;
esac

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

case "$remote" in
  /*) ;;
  *)
    if [ -z "${HOMEBREW_TAP_DEPLOY_KEY:-}" ]; then
      echo "::warning::HOMEBREW_TAP_DEPLOY_KEY is not set; the tap was not updated" >&2
      exit 0
    fi
    umask 077
    printf '%s\n' "$HOMEBREW_TAP_DEPLOY_KEY" > "$tmp/key"
    export GIT_SSH_COMMAND="ssh -i $tmp/key -o IdentitiesOnly=yes -o StrictHostKeyChecking=accept-new -o UserKnownHostsFile=$tmp/known_hosts"
    ;;
esac

# The formula first: a failure here (release not fully published) stops before the tap is touched.
bash "$here/update-formula.sh" "$v" > "$tmp/paddock.rb"
grep -q "releases/download/v$v/" "$tmp/paddock.rb" || { echo "paddock: the formula does not point at v$v" >&2; exit 1; }

git clone -q "$remote" "$tmp/tap"
mkdir -p "$tmp/tap/Formula"
cp "$tmp/paddock.rb" "$tmp/tap/Formula/paddock.rb"
cd "$tmp/tap"
if git diff --quiet && [ -z "$(git status --porcelain)" ]; then
  echo "the tap already has paddock $v"
  exit 0
fi
git add Formula/paddock.rb
git -c user.name='paddock release' -c user.email='41898282+github-actions[bot]@users.noreply.github.com' commit -q -m "paddock $v"
git push -q origin HEAD
echo "the tap now has paddock $v"
