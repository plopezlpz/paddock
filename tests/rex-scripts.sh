#!/usr/bin/env bash
# rex/*.lua against a running Rex server: the worker's snapshots (whole, a
# layout's with the panes kept, a block's), an edit, an unknown request; and
# the listener's lines (ready, a pane's news, a new tab's) and its end once
# nobody reads it. cargo test covers what paddock makes of them, with a
# scripted rex; nothing else runs the Lua.
#
# It makes a session of its own on the server, paddock-test-PID, and kills it
# when done. Needs the rex CLI and a server already running (none is
# started); skips otherwise, in CI too, which has no Rex.
# Run: bash tests/rex-scripts.sh
set -u
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
skip() { echo "skip: $1"; exit 0; }
command -v rex >/dev/null || skip "no rex"
rex --autostart=false whoami >/dev/null 2>&1 || skip "no Rex server running"

TMP=$(mktemp -d "${TMPDIR:-/tmp}/paddock-rex-test.XXXXXX")
NAME=paddock-test-$$
SESSION='' LISTENER=''
cleanup() {
  [ -z "$LISTENER" ] || kill "$LISTENER" 2>/dev/null
  [ -z "$SESSION" ] || rex kill "$SESSION" >/dev/null 2>&1
  rm -rf "$TMP"
}
trap cleanup EXIT

fails=0
check() { # check NAME COMMAND...: COMMAND succeeds
  local name=$1
  shift
  if "$@"; then echo "ok   $name"; else echo "FAIL $name"; fails=$((fails + 1)); fi
}
# until COMMAND...: COMMAND succeeds within 5 s
until_() {
  local n=0
  until "$@"; do
    n=$((n + 1))
    [ "$n" -lt 100 ] || return 1
    sleep 0.05
  done
}
# The first ID of KIND (session, window, block) in JSON text on stdin.
id_of() { grep -o "\"$1:[0-9a-f-]*\"" | head -n 1 | tr -d '"'; }

made=$(rex new "$NAME" --json 2>"$TMP/err") || { cat "$TMP/err"; echo "FAIL cannot make a session"; exit 1; }
SESSION=$(printf %s "$made" | id_of session)
BLOCK=$(printf %s "$made" | id_of block)
[ -n "$SESSION" ] && [ -n "$BLOCK" ] || { echo "FAIL no session or block in: $made"; exit 1; }
# Its shell takes a moment to start: until then the pane has no program.
running() { rex do -e "rex.session.attach { session_id = '$SESSION' } return rex.block.call('com.superlogical.terminal', 'process', { session_id = '$SESSION', block_id = '$BLOCK' })" 2>/dev/null | grep -Eq '"(argv0|name)"'; }
until_ running || echo "note: the session's shell did not start; the pane's program will be missing"

# The worker, as src/backend/rex.rs starts it, answering a line a request.
{
  printf 'PADDOCK_WORKER = true\nPADDOCK_ME = ""\nPADDOCK_HERE = ""\nPADDOCK_FROM = ""\n'
  cat "$ROOT/rex/snapshot.lua" "$ROOT/rex/worker.lua"
} >"$TMP/worker.lua"
cat >"$TMP/requests" <<EOF
{"op":"snapshot","skip":""}
{"op":"snapshot","skip":"","layout":true,"blocks":[]}
{"op":"snapshot","skip":"","layout":false,"blocks":["block:none","$BLOCK"]}
{"op":"rename_pane","block":"$BLOCK","name":"$NAME-label"}
{"op":"snapshot","skip":"","layout":true,"blocks":[]}
{"op":"snapshot","skip":"$BLOCK","layout":true,"blocks":[]}
{"op":"nonsense"}
EOF
rex do "$TMP/worker.lua" <"$TMP/requests" >"$TMP/answers" 2>"$TMP/worker.err"
answer() { sed -n "${1}p" "$TMP/answers"; }
# Our session's part of answer N: from its ID to the next session's.
ours() { answer "$1" | sed "s/.*\"id\":\"$SESSION\"//; s/\"id\":\"session:.*//"; }
check "worker: an answer for each request" [ "$(wc -l <"$TMP/answers" | tr -d ' ')" = 7 ]
check "worker: a whole snapshot has the session and its pane" eval 'answer 1 | grep -q "^{\"ok\":" && ours 1 | grep -q "$BLOCK"'
check "worker: its pane's program and folder are read" eval 'ours 1 | grep -q "\"argv0\":" && ours 1 | grep -q "\"cwd\":"'
check "worker: a layout's snapshot keeps the panes as read" [ "$(ours 1)" = "$(ours 2)" ]
check "worker: a block read again, and one unknown, change nothing" [ "$(ours 1)" = "$(ours 3)" ]
check "worker: an edit answers" eval 'answer 4 | grep -q "^{\"ok\":"'
check "worker: a kept pane takes the label the layout has" eval 'ours 5 | grep -q "\"label\":\"$NAME-label\""'
check "worker: the block to skip is left out" eval '! ours 6 | grep -q "$BLOCK"'
check "worker: an unknown request is an error, not the end" eval 'answer 7 | grep -q "^{\"err\":\"unknown request nonsense\"}"'

# The listener, alive every second for the last check.
sed 's/^local ALIVE = 10$/local ALIVE = 1/' "$ROOT/rex/events.lua" >"$TMP/events.lua"
check "listener: the test's copy differs only in ALIVE" grep -q '^local ALIVE = 1$' "$TMP/events.lua"
rex do "$TMP/events.lua" --all >"$TMP/heard" 2>"$TMP/listener.err" &
LISTENER=$!
check "listener: says when it listens" until_ grep -qx ready "$TMP/heard"
rex send -s "$SESSION" -b "$BLOCK" 'cd /' >/dev/null 2>&1
rex send-key -s "$SESSION" -b "$BLOCK" Enter >/dev/null 2>&1
check "listener: a pane's news names it" until_ grep -Eq "^(blocks|changed) .*$BLOCK" "$TMP/heard"
rex window new -s "$SESSION" "$NAME-tab" --focus=false >/dev/null 2>&1
check "listener: a new tab is a change of layout" until_ grep -q '^changed' "$TMP/heard"
check "listener: and says it lives" until_ grep -qx alive "$TMP/heard"
kill "$LISTENER" 2>/dev/null
LISTENER=''

# Left behind by a paddock that was killed, it ends when it next writes.
(
  rex do "$TMP/events.lua" --all &
  echo $! >"$TMP/pid"
  wait
) 2>/dev/null | head -n 1 >/dev/null
gone() { ! kill -0 "$(cat "$TMP/pid")" 2>/dev/null; }
check "listener: ends once nobody reads it" until_ gone

if [ "$fails" -eq 0 ]; then echo "all passed"; else echo "$fails failed"; cat "$TMP/worker.err" "$TMP/listener.err" 2>/dev/null; exit 1; fi
