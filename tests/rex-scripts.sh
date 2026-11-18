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
{"op":"send","block":"$BLOCK","text":"echo typed-$NAME"}
{"op":"send","block":"$BLOCK","key":{"key":"enter","mods":"","consumed_mods":"","text":"","unshifted_codepoint":0,"composing":false,"action":"press"}}
{"op":"send","block":"$BLOCK","text":"echo pasted-$NAME"}
{"op":"send","block":"$BLOCK","key":{"key":"enter","mods":"","consumed_mods":"","text":"","unshifted_codepoint":0,"composing":false,"action":"press"}}
EOF
rex do "$TMP/worker.lua" <"$TMP/requests" >"$TMP/answers" 2>"$TMP/worker.err"
answer() { sed -n "${1}p" "$TMP/answers"; }
# Our session's part of answer N: from its ID to the next session's.
ours() { answer "$1" | sed "s/.*\"id\":\"$SESSION\"//; s/\"id\":\"session:.*//"; }
check "worker: an answer for each request" [ "$(wc -l <"$TMP/answers" | tr -d ' ')" = 11 ]
check "worker: a whole snapshot has the session and its pane" eval 'answer 1 | grep -q "^{\"ok\":" && ours 1 | grep -q "$BLOCK"'
check "worker: its pane's program and folder are read" eval 'ours 1 | grep -q "\"argv0\":" && ours 1 | grep -q "\"cwd\":"'
check "worker: a layout's snapshot keeps the panes as read" [ "$(ours 1)" = "$(ours 2)" ]
check "worker: a block read again, and one unknown, change nothing" [ "$(ours 1)" = "$(ours 3)" ]
check "worker: an edit answers" eval 'answer 4 | grep -q "^{\"ok\":"'
check "worker: a kept pane takes the label the layout has" eval 'ours 5 | grep -q "\"label\":\"$NAME-label\""'
check "worker: the block to skip is left out" eval '! ours 6 | grep -q "$BLOCK"'
check "worker: an unknown request is an error, not the end" eval 'answer 7 | grep -q "^{\"err\":\"unknown request nonsense\"}"'
check "worker: keys and pastes are sent" eval '( for n in 8 9 10 11; do answer $n | grep -q "^{\"ok\":" || exit 1; done )'
shown() { rex capture -s "$SESSION" -b "$BLOCK" 2>/dev/null | grep -q "^$1"; }
check "worker: typed text runs in the pane, with a ⏎ key event" until_ shown "typed-$NAME"
check "worker: pasted text does too, with its ⏎" until_ shown "pasted-$NAME"

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

# A sub-task that waits on the user (a subagent's permission prompt) under
# a root that works on: the snapshot shows the wait, with its message.
msg=$(printf 'Allow edit?' | base64)
reporter="printf '\\033]7501;state=working:app=paddocktest\\033\\\\'; printf '\\033]7501;id=sub:state=blocked:kind=permission:msg=$msg\\033\\\\'; sleep 30"
rex send -s "$SESSION" -b "$BLOCK" "$reporter" >/dev/null 2>&1
rex send-key -s "$SESSION" -b "$BLOCK" Enter >/dev/null 2>&1
snap() { printf '{"op":"snapshot","skip":""}\n' | rex do "$TMP/worker.lua" 2>/dev/null | sed "s/.*\"id\":\"$BLOCK\"//; s/\"id\":\"block:.*//"; }
blocked() { snap | grep -q '"state":"blocked"' && snap | grep -q 'Allow edit?'; }
check "worker: a blocked sub-task's record is the one shown" until_ blocked

# Calls that fail (a connection's hiccup), with Rex's calls swapped for ones
# that answer nil and an error, as theirs do: what was read stays, and is
# asked for again. And the action says why a popup did not open.
{
  printf 'PADDOCK_WORKER = true\nPADDOCK_ME = ""\nPADDOCK_HERE = ""\nPADDOCK_FROM = ""\n'
  printf 'SESSION, BLOCK, ROOT = "%s", "%s", "%s"\n' "$SESSION" "$BLOCK" "$ROOT"
  cat "$ROOT/rex/snapshot.lua"
  cat <<'LUA'
local call, view, new_layer = rex.block.call, rex.session.view, rex.session.new_layer
local function say(name, ok) print((ok and "ok   " or "FAIL ") .. name) end
local function ours(sessions)
  for _, s in ipairs(sessions) do if s.id == SESSION then return s end end
end
local function pane_of(s)
  for _, w in ipairs(s and s.windows or {}) do
    for _, p in ipairs(w.panes) do if p.id == BLOCK then return p end end
  end
end
local function blocked(p) return p and p.status and p.status.state == "blocked" end
local p = pane_of(ours(snapshot("")))
say("faults: the pane is read, blocked", blocked(p) and p.argv0 and p.unsure == nil)
rex.block.call = function(c, m, a)
  if m == "process" or m == "program_status" then return nil, "hiccup" end
  return call(c, m, a)
end
local q = pane_of(ours(snapshot("")))
say("faults: a pane's calls unanswered keep its status and program", blocked(q) and q.argv0 == p.argv0 and q.pid == p.pid)
say("faults: and ask for it again", q and q.unsure == true)
snapshot("")
snapshot("")
q = pane_of(ours(snapshot("")))
say("faults: but not after three failures in a row", blocked(q) and q.unsure == nil)
rex.block.call = call
rex.session.view = function() return nil, "hiccup" end
local s = ours(snapshot("", true))
say("faults: a session unread keeps its tabs, and asks again", pane_of(s) and s.unsure == true)
rex.session.view = view
s = ours(snapshot(""))
say("faults: read again, nothing is unsure", s and s.unsure == nil and blocked(pane_of(s)) and pane_of(s).unsure == nil)
local actions = {}
rex.action, rex.bind = function(a) actions[a.name] = a end, nil
dofile(ROOT .. "/rex/paddock.lua").setup {}
rex.session.new_layer = function() return nil, "no room" end
local ok, err = pcall(actions.paddock.run, { session_id = SESSION }, {})
rex.session.new_layer = new_layer
say("action: a popup that cannot open says why", not ok and tostring(err):find("no room", 1, true) ~= nil)
LUA
} >"$TMP/faults.lua"
rex do "$TMP/faults.lua" >"$TMP/faults.out" 2>&1
cat "$TMP/faults.out"
check "faults: all seven ran" [ "$(grep -c '^ok ' "$TMP/faults.out")" = 7 ]
rex send-key -s "$SESSION" -b "$BLOCK" ctrl+c >/dev/null 2>&1

# A record's owner is the process in front when it came (an agent stays the
# same process while it runs): what paddock compares with the one in front
# now, to tell an agent that died without saying.
if command -v perl >/dev/null; then
  # In a pane of its own: the earlier records outlive their programs.
  made=$(rex new "$NAME-owner" --json 2>/dev/null)
  OWNED=$(printf %s "$made" | id_of session) PANE=$(printf %s "$made" | id_of block)
  sleep 1 # its shell starts
  rex send -s "$OWNED" -b "$PANE" "perl -e '\$|=1; print \"\\e]7501;state=done:app=paddocktest\\e\\\\\"; sleep 1; exec \"sleep\", \"30\"'" >/dev/null 2>&1
  rex send-key -s "$OWNED" -b "$PANE" Enter >/dev/null 2>&1
  # The perl that reported, now sleep (exec): still the same pid in front.
  same() { local s p; s=$(printf '{"op":"snapshot","skip":""}\n' | rex do "$TMP/worker.lua" 2>/dev/null | sed "s/.*\"id\":\"$PANE\"//; s/\"id\":\"block:.*//")
    p=$(printf '%s' "$s" | grep -o '"owner":{"name":"perl","pid":[0-9]*' | grep -o '[0-9]*$')
    [ -n "$p" ] && printf '%s' "$s" | grep -q "^,\"label\":\"[^\"]*\",\"pid\":$p,"; }
  check "worker: the reporting program is the one in front, by pid" until_ same
  rex send-key -s "$OWNED" -b "$PANE" ctrl+c >/dev/null 2>&1
  sleep 0.5
  # An agent's child (a tool it runs) is in its process group: the agent
  # stays the process in front.
  rex send -s "$OWNED" -b "$PANE" "perl -e '\$|=1; print \"\\e]7501;state=done:app=paddocktest\\e\\\\\"; system(\"sleep\", \"30\")'" >/dev/null 2>&1
  rex send-key -s "$OWNED" -b "$PANE" Enter >/dev/null 2>&1
  sleep 1
  check "worker: an agent running a child is still the one in front" until_ same
  rex send-key -s "$OWNED" -b "$PANE" ctrl+c >/dev/null 2>&1
  sleep 0.5
  # A key event is encoded as the pane's program asked: ⇧⏎ to one that
  # asked for the kitty protocol (as agents do) is ⇧⏎, not ⏎.
  rex send -s "$OWNED" -b "$PANE" "printf '\\033[>1u'; stty raw -echo; perl -e '\$|=1; open(F, \">>\", \"$TMP/keys.out\") or die; select F; \$|=1; while (sysread(STDIN, my \$c, 1)) { printf \"<%02x>\", ord \$c }'" >/dev/null 2>&1
  rex send-key -s "$OWNED" -b "$PANE" Enter >/dev/null 2>&1
  sleep 1
  key='{"key":"enter","mods":"shift","consumed_mods":"","text":"","unshifted_codepoint":0,"composing":false,"action":"press"}'
  printf '{"op":"send","block":"%s","key":%s}\n' "$PANE" "$key" | rex do "$TMP/worker.lua" >/dev/null 2>&1
  shifted() { grep -q '<1b><5b><31><33><3b><32><75>' "$TMP/keys.out" 2>/dev/null; }
  check "worker: ⇧⏎ reaches a kitty pane as ⇧⏎" until_ shifted
  rex kill "$OWNED" >/dev/null 2>&1
fi

if [ "$fails" -eq 0 ]; then echo "all passed"; else echo "$fails failed"; cat "$TMP/worker.err" "$TMP/listener.err" 2>/dev/null; exit 1; fi
