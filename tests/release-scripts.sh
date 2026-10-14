#!/usr/bin/env bash
# Offline tests for scripts/update-tap.sh and scripts/advance-master.sh, against
# local bare repos. Usage: bash tests/release-scripts.sh
set -uo pipefail

ROOT=$(CDPATH='' cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@t GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@t
unset HOMEBREW_TAP_DEPLOY_KEY
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
git clone -q "$TMP/tap.git" "$TMP/seed" && (cd "$TMP/seed" && echo tap > README.md && git add . && git commit -qm seed && git push -q origin master)
up() { TAP_REMOTE=$TMP/tap.git FORMULA_FROM=$TMP/rel bash "$ROOT/scripts/update-tap.sh" "$@"; }
formula() { git -C "$TMP/tap.git" show master:Formula/paddock.rb; }
count() { git -C "$TMP/tap.git" rev-list --count master; }

check "tap: a release adds the formula" up 1.2.3
check "tap: the formula names the release" sh -c "$(declare -f formula); TMP=$TMP; formula | grep -q 'releases/download/v1.2.3/'"
n=$(count)
check "tap: the same release again commits nothing" up v1.2.3
check "tap: ... and the tap is unchanged" test "$(count)" = "$n"
check "tap: a pre-release is skipped" up 1.3.0-rc1
check "tap: ... and the tap is unchanged" test "$(count)" = "$n"
mkdir "$TMP/empty"
check "tap: missing checksums fail" sh -c "! TAP_REMOTE=$TMP/tap.git FORMULA_FROM=$TMP/empty bash $ROOT/scripts/update-tap.sh 1.4.0"
check "tap: ... and the tap is unchanged" test "$(count)" = "$n"

# The tap moves while the update is in flight: the first push is refused, another
# project's formula lands meanwhile, and the retry replays this commit on top.
cat > "$TMP/tap.git/hooks/update" <<HOOK
#!/bin/sh
[ -e "$TMP/refused" ] && exit 0
touch "$TMP/refused"
( unset GIT_DIR GIT_QUARANTINE_PATH GIT_OBJECT_DIRECTORY GIT_ALTERNATE_OBJECT_DIRECTORIES; sleep 1; cd "$TMP/seed" && git pull -q origin master && mkdir -p Formula && echo other > Formula/other.rb && git add . && git commit -qm other && git push -q origin master ) >/dev/null 2>&1 &
exit 1
HOOK
chmod +x "$TMP/tap.git/hooks/update"
check "tap: a refused push is retried" up 1.5.0
rm "$TMP/tap.git/hooks/update"
check "tap: ... the other formula survives" sh -c "git -C $TMP/tap.git show master:Formula/other.rb | grep -q other"
check "tap: ... and this one is the new release" sh -c "git -C $TMP/tap.git show master:Formula/paddock.rb | grep -q 'download/v1.5.0/'"

# ---- advance-master.sh ----
git init -q --bare -b master "$TMP/origin.git"
git clone -q "$TMP/origin.git" "$TMP/w"
cd "$TMP/w" || exit 1
git commit -q --allow-empty -m one && git push -q origin master
git commit -q --allow-empty -m release; rel=$(git rev-parse HEAD)
adv() { bash "$ROOT/scripts/advance-master.sh" "$@"; }
check "master: fast-forwards to the release" adv "$rel"
check "master: ... and is the release" test "$(git -C "$TMP/origin.git" rev-parse master)" = "$rel"
check "master: already there is fine" adv "$rel"
git commit -q --allow-empty -m later && git push -q origin HEAD:master
check "master: behind master already is fine" adv "$rel"
check "master: ... and master is not moved back" test "$(git -C "$TMP/origin.git" rev-parse master)" = "$(git rev-parse HEAD)"
git checkout -q "$rel~1" && git commit -q --allow-empty -m side; side=$(git rev-parse HEAD)
check "master: a commit off master's line fails" sh -c "! bash $ROOT/scripts/advance-master.sh $side"

if [ "$fails" -eq 0 ]; then echo "all passed"; else echo "$fails failed"; exit 1; fi
