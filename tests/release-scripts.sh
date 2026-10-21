#!/usr/bin/env bash
# Offline tests for scripts/update-tap.sh and scripts/advance-master.sh, against
# local bare repos. Usage: bash tests/release-scripts.sh
set -uo pipefail

ROOT=$(CDPATH='' cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
# No git identity anywhere, as on a bare CI runner: the scripts must bring their own.
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
unset GIT_AUTHOR_NAME GIT_AUTHOR_EMAIL GIT_COMMITTER_NAME GIT_COMMITTER_EMAIL HOMEBREW_TAP_DEPLOY_KEY GITHUB_ACTIONS GITHUB_REPOSITORY
# The tests' own commits get an identity per command.
g() { git -c user.name=t -c user.email=t@t "$@"; }
fails=0
check() { # check NAME CMD...: CMD must succeed.
  local name=$1; shift
  if "$@" >/dev/null 2>&1; then echo "ok   $name"; else echo "FAIL $name"; fails=$((fails + 1)); fi
}

# ---- update-tap.sh ----
mkdir "$TMP/rel"
for t in aarch64-apple-darwin x86_64-apple-darwin aarch64-unknown-linux-musl x86_64-unknown-linux-musl; do
  printf '%064d  paddock-%s.tar.gz\n' 0 "$t" > "$TMP/rel/paddock-$t.tar.gz.sha256"
done
git init -q --bare -b master "$TMP/tap.git"
git clone -q "$TMP/tap.git" "$TMP/seed" 2>/dev/null
(cd "$TMP/seed" && echo tap > README.md && git add . && g commit -qm seed && git push -q origin master)
up() { TAP_REMOTE=$TMP/tap.git FORMULA_FROM=$TMP/rel bash "$ROOT/scripts/update-tap.sh" "$@"; }
count() { git -C "$TMP/tap.git" rev-list --count master; }
has() { git -C "$TMP/tap.git" show "master:$1" 2>/dev/null | grep -q "$2"; }

check "tap: a release adds the formula" up 1.2.3
check "tap: the formula names the release" has Formula/paddock.rb 'download/v1.2.3/'
check "tap: the commit is the release bot's" sh -c "git -C $TMP/tap.git log -1 --format=%cn master | grep -qx 'paddock release'"
n=$(count)
check "tap: the same release again commits nothing" up v1.2.3
check "tap: ... and the tap is unchanged" test "$(count)" = "$n"
check "tap: a pre-release is skipped" up 1.3.0-rc1
check "tap: ... and the tap is unchanged" test "$(count)" = "$n"
check "tap: an older release is not a downgrade" up 1.2.0
check "tap: ... and the tap is unchanged" test "$(count)" = "$n"
mkdir "$TMP/empty"
check "tap: missing checksums fail" sh -c "! TAP_REMOTE=$TMP/tap.git FORMULA_FROM=$TMP/empty bash $ROOT/scripts/update-tap.sh 1.4.0"
check "tap: ... and the tap is unchanged" test "$(count)" = "$n"
nokey() { TAP_REMOTE=git@github.com:example/none.git FORMULA_FROM=$TMP/rel GITHUB_ACTIONS=true GITHUB_REPOSITORY=$1 bash "$ROOT/scripts/update-tap.sh" 1.4.0; }
check "tap: in CI without the key, paddock's own run fails" sh -c "$(declare -f nokey); ROOT=$ROOT TMP=$TMP; ! nokey plopezlpz/paddock"
check "tap: in CI without the key, a fork skips" sh -c "$(declare -f nokey); ROOT=$ROOT TMP=$TMP; nokey someone/paddock"

# The tap moves after the update has committed and before it pushes (another
# project's release): the push is refused, and the retry replays this commit on top.
other="cd $TMP/seed && git pull -q origin master && mkdir -p Formula && echo other > Formula/other.rb && git add . && git -c user.name=t -c user.email=t@t commit -qm other && git push -q origin master"
check "tap: a refused push is retried" env TAP_BEFORE_PUSH="$other" TAP_REMOTE="$TMP/tap.git" FORMULA_FROM="$TMP/rel" bash "$ROOT/scripts/update-tap.sh" 1.5.0
check "tap: ... the other formula survives" has Formula/other.rb other
check "tap: ... and this one is the new release" has Formula/paddock.rb 'download/v1.5.0/'

# ---- advance-master.sh ----
git init -q --bare -b master "$TMP/origin.git"
git clone -q "$TMP/origin.git" "$TMP/w" 2>/dev/null
cd "$TMP/w" || exit 1
g commit -q --allow-empty -m one && git push -q origin master
g commit -q --allow-empty -m release; rel=$(git rev-parse HEAD)
g tag -a -m v9.9.9 v9.9.9
adv() { bash "$ROOT/scripts/advance-master.sh" "$@"; }
check "master: fast-forwards to an annotated tag's commit" adv v9.9.9
check "master: ... and is the release" test "$(git -C "$TMP/origin.git" rev-parse master)" = "$rel"
check "master: already there is fine" adv "$rel"
g commit -q --allow-empty -m later && git push -q origin HEAD:master
check "master: behind master already is fine" adv "$rel"
check "master: ... and master is not moved back" test "$(git -C "$TMP/origin.git" rev-parse master)" = "$(git rev-parse HEAD)"
git checkout -q "$rel~1" && g commit -q --allow-empty -m side; side=$(git rev-parse HEAD)
check "master: a commit off master's line fails" sh -c "! bash $ROOT/scripts/advance-master.sh $side"
check "master: ... and says to merge" sh -c "bash $ROOT/scripts/advance-master.sh $side 2>&1 | grep -q 'merge'"

# ---- advance-master.sh --check (before publishing) ----
git init -q --bare -b master "$TMP/check.git"
git clone -q "$TMP/check.git" "$TMP/c" 2>/dev/null
cd "$TMP/c" || exit 1
ver() { # ver VERSION [BASE]: a new commit (on BASE) whose Cargo.toml has VERSION; prints its sha.
  [ $# -lt 2 ] || git checkout -q "$2"
  printf '[package]\nname = "paddock"\nversion = "%s"\n' "$1" > Cargo.toml
  # Numbered: the same version on the same base in the same second is still a new commit.
  n=$(($(cat "$TMP/n" 2>/dev/null || echo 0) + 1)) && echo "$n" > "$TMP/n"
  git add Cargo.toml && g commit -q --allow-empty -m "$1 ($n)" && git rev-parse HEAD
}
on_master() { git push -q -f origin "$1:refs/heads/master"; }
says() { [ "$(bash "$ROOT/scripts/advance-master.sh" --check "$1" 2>/dev/null)" = "$2" ]; }
refused() { ! bash "$ROOT/scripts/advance-master.sh" --check "$1"; }
base=$(ver 0.9.0); on_master "$base"
bump=$(ver 0.10.0)
check "check: a fast-forward advances" says "$bump" advance
check "check: ... and moves nothing" test "$(git -C "$TMP/check.git" rev-parse master)" = "$base"
on_master "$bump"
check "check: a commit master has advances" says "$base" advance
# A commit lands on master between the version bump and the tag.
rel=$(ver 0.11.0 "$bump"); later=$(ver 0.10.0 "$bump"); on_master "$later"
check "check: a newer version off master's line is refused" refused "$rel"
check "check: ... and says to merge" sh -c "bash $ROOT/scripts/advance-master.sh --check $rel 2>&1 | grep -q 'merge'"
check "check: the same version off master's line is refused" refused "$(ver 0.10.0 "$base")"
check "check: an older version off master's line is a backport" says "$(ver 0.9.1 "$base")" backport
on_master "$(ver 1.0.0-rc.10 "$later")"
check "check: a backport is older than master's pre-release" says "$(ver 0.10.1 "$later")" backport
check "check: ... a pre-release by its fields" says "$(ver 1.0.0-rc.2 "$later")" backport
check "check: the release of master's pre-release is not a backport" refused "$(ver 1.0.0 "$later")"

if [ "$fails" -eq 0 ]; then echo "all passed"; else echo "$fails failed"; exit 1; fi
