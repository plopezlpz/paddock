#!/usr/bin/env bash
# paddock.tmux against disposable tmux servers: the key and click bindings,
# reloads, and a real click on the session name. A stub stands in for the
# paddock binary. Needs tmux 3.8+; skips otherwise, except in CI.
# Run: bash tests/paddock-tmux.sh
set -u
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
# skip REASON: a pass locally, a failure in CI (which must run the test).
skip() { echo "skip: $1"; [ -z "${CI:-}" ]; exit; }
command -v tmux >/dev/null || skip "no tmux"

TMP=$(mktemp -d "${TMPDIR:-/tmp}/paddock-tmux-test.XXXXXX")
IN=paddock-test-in-$$
OUT=paddock-test-out-$$
cleanup() {
  tmux -L "$OUT" kill-server 2>/dev/null
  tmux -L "$IN" kill-server 2>/dev/null
  rm -rf "$TMP"
}
trap cleanup EXIT
unset TMUX

# A checkout with a stub build: it records how the popup started it.
mkdir -p "$TMP/plugin/target/release"
cp "$ROOT/paddock.tmux" "$TMP/plugin/"
cat >"$TMP/plugin/target/release/paddock" <<EOF
#!/bin/sh
echo "\$PADDOCK_CLIENT \$PADDOCK_SESSION \$PADDOCK_WINDOW \$*" >"$TMP/opened"
EOF
chmod +x "$TMP/plugin/target/release/paddock"

t() { tmux -L "$IN" "$@"; }
t -f /dev/null new -d -s main -x 60 -y 10 'sleep 600'
t show-hooks -g client-closed >/dev/null 2>&1 || skip "tmux 3.8+ needed"
t set -g mouse on \; set -g status-left '[main] '

fails=0
check() { # check NAME COMMAND...: report and count a failed command.
  local name=$1; shift
  if "$@"; then echo "ok   $name"; else echo "FAIL $name"; fails=$((fails + 1)); fi
}
load() { t run-shell "$TMP/plugin/paddock.tmux"; }
root_click() { t list-keys -T root MouseUp1StatusLeft 2>/dev/null; }
has() { case $1 in *"$2"*) return 0 ;; esac; return 1; }

t set -g @paddock-key a \; set -g @paddock-click on
load
check "click binding opens the popup" has "$(root_click)" "display-popup"
check "click binding carries the paddock note" has "$(t list-keys -N -T root MouseUp1StatusLeft 2>/dev/null)" "paddock"
check "the key still binds" has "$(t list-keys -T prefix a)" "display-popup"
check "nothing on press" test -z "$(t list-keys -T root MouseDown1StatusLeft 2>/dev/null)"

# A real click: an outer server's pane runs a client of the inner one, and
# SGR mouse bytes typed into it press and release on the status line.
tmux -L "$OUT" -f /dev/null new -d -x 60 -y 10 "env -u TMUX tmux -L $IN attach -t main"
for _ in 1 2 3 4 5 6 7 8 9 10; do
  [ -n "$(t list-clients)" ] && break
  sleep 0.1
done
client=$(t list-clients -F '#{client_name}')
press() { tmux -L "$OUT" send-keys -H 1b 5b 3c 30 3b 32 3b 31 30 "$1"; }
press 4d; sleep 0.3
check "press alone opens nothing" test ! -e "$TMP/opened"
press 6d
for _ in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do
  [ -e "$TMP/opened" ] && break
  sleep 0.1
done
expected="$client $(t display -p -t main '#{session_id} #{window_id}') "
check "a click opens paddock for the clicking client" test "$(cat "$TMP/opened" 2>/dev/null)" = "$expected"

# Reloads: switching the click off removes paddock's binding, never a user's.
t set -gu @paddock-click
load
check "click off unbinds it" test -z "$(root_click)"
t bind -n MouseUp1StatusLeft display-message mine
load
check "a user's own click binding survives" has "$(root_click)" "display-message mine"
t unbind -n MouseUp1StatusLeft

t set -gu @paddock-key \; set -g @paddock-click on
load
check "the click alone is enough" has "$(root_click)" "display-popup"
check "no missing-key warning" test -z "$(t show-messages 2>/dev/null | grep 'set @paddock-key')"

rm "$TMP/plugin/target/release/paddock"
load
check "without a build, the click explains" has "$(root_click)" "not built"

[ "$fails" -eq 0 ] && echo "all passed" || { echo "$fails failed"; exit 1; }
