#!/usr/bin/env bash
# Updates the Homebrew tap's Formula/paddock.rb for a published release. Run by
# the Release workflow after it publishes; works by hand too.
# Usage: scripts/update-tap.sh VERSION
# Env: HOMEBREW_TAP_DEPLOY_KEY: the private half of a write deploy key for the tap.
#        In CI it is required (in plopezlpz/paddock; a fork without it skips).
#        By hand, without it, git uses your own SSH credentials.
#      TAP_REMOTE: the tap's git URL or path (default git@github.com:plopezlpz/homebrew-tap.git);
#        a path needs no key, for tests
#      FORMULA_FROM: a directory holding the release's .sha256 files instead of
#        downloading them (passed to update-formula.sh --from, for tests)
#      TAP_BEFORE_PUSH: a command run just before the first push (tests use it to
#        move the tap under the update)
set -euo pipefail

[ $# -eq 1 ] || { echo 'usage: scripts/update-tap.sh VERSION' >&2; exit 2; }
v=${1#v}
here=$(CDPATH='' cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
remote=${TAP_REMOTE:-git@github.com:plopezlpz/homebrew-tap.git}

case "$v" in
  *-*) echo "paddock: $v is a pre-release; the tap is not updated"; exit 0 ;;
esac

# Every commit this script makes, the rebase's included, is by the release bot.
export GIT_AUTHOR_NAME='paddock release' GIT_COMMITTER_NAME='paddock release'
export GIT_AUTHOR_EMAIL='41898282+github-actions[bot]@users.noreply.github.com'
export GIT_COMMITTER_EMAIL=$GIT_AUTHOR_EMAIL

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

case "$remote" in
  /*) ;;
  *)
    if [ -n "${HOMEBREW_TAP_DEPLOY_KEY:-}" ]; then
      umask 077
      printf '%s\n' "$HOMEBREW_TAP_DEPLOY_KEY" > "$tmp/key"
      # GitHub's published host key (api.github.com/meta), so the key is only ever offered to GitHub.
      echo 'github.com ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl' > "$tmp/known_hosts"
      export GIT_SSH_COMMAND="ssh -i $tmp/key -o IdentitiesOnly=yes -o StrictHostKeyChecking=yes -o UserKnownHostsFile=$tmp/known_hosts"
    elif [ -n "${GITHUB_ACTIONS:-}" ]; then
      if [ "${GITHUB_REPOSITORY:-}" = plopezlpz/paddock ]; then
        echo "::error::the HOMEBREW_TAP_DEPLOY_KEY secret is not set; the tap was not updated" >&2
        exit 1
      fi
      echo "::warning::no HOMEBREW_TAP_DEPLOY_KEY in ${GITHUB_REPOSITORY:-this repo}; the tap was not updated" >&2
      exit 0
    fi
    ;;
esac

# The formula first: a failure here (release not fully published) stops before the tap is touched.
bash "$here/update-formula.sh" ${FORMULA_FROM:+--from "$FORMULA_FROM"} "$v" > "$tmp/paddock.rb"
grep -q "releases/download/v$v/" "$tmp/paddock.rb" || { echo "paddock: the formula does not point at v$v" >&2; exit 1; }

git clone -q "$remote" "$tmp/tap"
mkdir -p "$tmp/tap/Formula"
# Never go back: re-running an old release's workflow must not downgrade brew users.
have=
if [ -f "$tmp/tap/Formula/paddock.rb" ]; then
  have=$(sed -n 's|.*releases/download/v\([^/]*\)/.*|\1|p' "$tmp/tap/Formula/paddock.rb" | head -n 1)
fi
if [ -n "$have" ] && [ "$have" != "$v" ] &&
   [ "$(printf '%s\n%s\n' "$have" "$v" | sort -V | tail -n 1)" = "$have" ]; then
  echo "the tap has paddock $have, newer than $v; left alone"
  exit 0
fi
cp "$tmp/paddock.rb" "$tmp/tap/Formula/paddock.rb"
cd "$tmp/tap"
if [ -z "$(git status --porcelain)" ]; then
  echo "the tap already has paddock $v"
  exit 0
fi
git add Formula/paddock.rb
git commit -q -m "paddock $v"

[ -z "${TAP_BEFORE_PUSH:-}" ] || bash -c "$TAP_BEFORE_PUSH"

# Another project's release may push to the tap at the same time: replay this
# one commit on top and try again. Only Formula/paddock.rb changes, so the
# rebase cannot conflict unless someone else edited this formula.
branch=$(git rev-parse --abbrev-ref HEAD)
for try in 1 2 3; do
  if git push -q origin HEAD; then
    echo "the tap now has paddock $v"
    exit 0
  fi
  [ "$try" -lt 3 ] || break
  echo "the tap moved; retrying ($try)" >&2
  sleep "$try"
  git pull -q --rebase origin "$branch"
done
echo "paddock: could not push to the tap" >&2
exit 1
