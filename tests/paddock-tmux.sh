#!/usr/bin/env bash
# paddock.tmux against disposable tmux servers: the key and click bindings,
# reloads, which binary gets bound (only one reporting the plugin's version,
# else the installer), and a real click on the session name. A stub stands in
# for the paddock binary. Needs tmux 3.8+; skips otherwise, except in CI.
# Run: bash tests/paddock-tmux.sh
set -u
unset CDPATH # the relative cd below must not be steered; the checks set their own
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
# skip REASON: a pass locally, a failure in CI (which must run the test).
skip() { echo "skip: $1"; [ -z "${CI:-}" ]; exit; }
command -v tmux >/dev/null || skip "no tmux"

TMP=$(mktemp -d "${TMPDIR:-/tmp}/paddock-tmux-test.XXXXXX")
TMP=$(cd "$TMP" && pwd) # TMPDIR's trailing / would differ from the paths the plugin computes
IN=paddock-test-in-$$
OUT=paddock-test-out-$$
# The test servers' sockets go in a private dir: tmux 3.8 leaves socket files
# behind on exit. Under /tmp, as socket paths are limited to 104 bytes on macOS.
# Its path has no "paddock" in it, for a binary called something else.
TMUX_TMPDIR=$(mktemp -d /tmp/pdk-tmux-sock.XXXXXX)
export TMUX_TMPDIR
cleanup() {
  chmod -R u+w "$TMP" 2>/dev/null # an interrupt inside the unwritable-dir check
  tmux -L "$OUT" kill-server 2>/dev/null
  tmux -L "$IN" kill-server 2>/dev/null
  rm -rf "$TMP" "$TMUX_TMPDIR"
}
trap cleanup EXIT
unset TMUX
# Only tmux, not its directory (Homebrew's may hold a newer bash): the server
# gets /usr/bin:/bin, so the scripts run under the system bash (3.2 on macOS).
mkdir "$TMP/tools"
ln -s "$(command -v tmux)" "$TMP/tools/tmux"
PATH="$TMP/tools:/usr/bin:/bin"
export HOME="$TMP" PATH

# A checkout at a made-up version, so no real paddock can match it.
V=0.0.0-test
mkdir -p "$TMP/plugin/scripts"
cp "$ROOT/paddock.tmux" "$TMP/plugin/"
# The decoy table: its version must not be mistaken for the package's.
cat >"$TMP/plugin/Cargo.toml" <<EOF
[package]
name = "paddock"
version = "$V"
rust-version = "1.88"
repository = "https://github.com/plopezlpz/paddock"

[dependencies.x]
version = "9.9.9"
EOF
# The real installer when there is one (it is pressed at the end), else a placeholder.
if [ -f "$ROOT/scripts/install.sh" ]; then
  cp "$ROOT/scripts/install.sh" "$TMP/plugin/scripts/"
else
  printf '#!/bin/sh\n' >"$TMP/plugin/scripts/install.sh"
fi

# stub PATH VERSION [EXIT]: a paddock reporting VERSION. A --version probe is
# logged apart from a real run, which records how the popup started it.
stub() {
  mkdir -p "$(dirname "$1")"
  cat >"$1" <<EOF
#!/bin/sh
if [ "\$1" = --version ]; then echo "\$0" >>"$TMP/probed"; echo "paddock $2"; exit ${3:-0}; fi
echo "\$0 \$PADDOCK_CLIENT \$PADDOCK_SESSION \$PADDOCK_WINDOW \$*" >>"$TMP/opened"
EOF
  chmod +x "$1"
}
stub "$TMP/plugin/target/release/paddock" "$V"

t() { tmux -L "$IN" "$@"; }
t -f /dev/null new -d -s main -x 60 -y 10 'sleep 600' || { echo "FAIL server start"; exit 1; }
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
lacks() { ! has "$@"; }
# tmq VALUE: VALUE single-quoted for sh, # doubled for run-shell's format expansion.
tmq() { printf "'%s'" "$(printf '%s' "$1" | sed "s/'/'\\\\''/g; s/#/##/g")"; }
reset() { rm -rf "$TMP/plugin/bin" "$TMP/plugin/target" "$TMP/probed"; }
# PADDOCK_BIN goes inline: the server's environment was copied at its start.
load_bin() { t run-shell "PADDOCK_BIN=$(tmq "$1") $(tmq "$TMP/plugin/paddock.tmux")"; }

t set -g @paddock-key a \; set -g @paddock-click on
load
check "click binding opens the popup" has "$(root_click)" "$TMP/plugin/target/release/paddock"
check "click binding carries the paddock note" has "$(t list-keys -N -T root MouseUp1StatusLeft 2>/dev/null)" "paddock"
check "the key still binds" has "$(t list-keys -T prefix a)" "$TMP/plugin/target/release/paddock"
check "the popup hands over the plugin's version" has "$(t list-keys -T prefix a)" "-e PADDOCK_PLUGIN_VERSION=$V"
check "nothing on press" test -z "$(t list-keys -T root MouseDown1StatusLeft 2>/dev/null)"

# A real click: an outer server's pane runs a client of the inner one, and
# SGR mouse bytes typed into it press and release on the status line.
tmux -L "$OUT" -f /dev/null new -d -x 60 -y 10 "env -u TMUX tmux -L $IN attach -t main"
for _ in $(seq 50); do
  [ -n "$(t list-clients)" ] && break
  sleep 0.1
done
client=$(t list-clients -F '#{client_name}')
check "nested client attached" test -n "$client"
press() { tmux -L "$OUT" send-keys -H 1b 5b 3c 30 3b 32 3b 31 30 "$1"; }
# Keys typed before the client has drawn are lost: wait for its status line.
wait_screen() { # wait_screen TEXT: poll the outer pane until TEXT is on screen.
  for _ in $(seq 50); do
    has "$(tmux -L "$OUT" capture-pane -p)" "$1" && return 0
    sleep 0.1
  done
  return 1
}
wait_gone() { # wait_gone TEXT: poll the outer pane until TEXT is gone.
  for _ in $(seq 50); do
    has "$(tmux -L "$OUT" capture-pane -p)" "$1" || return 0
    sleep 0.1
  done
  return 1
}
wait_opened() { # wait_opened: poll until the stub has recorded a run.
  for _ in $(seq 50); do
    [ -s "$TMP/opened" ] && return 0
    sleep 0.1
  done
  return 1
}
check "nested client drew its status" wait_screen "[main]"
press 4d; sleep 0.3
check "press alone opens nothing" test ! -e "$TMP/opened"
press 6d
wait_opened
expected="$TMP/plugin/target/release/paddock $client $(t display -p -t main '#{session_id} #{window_id}') --tab last"
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
check "the click alone is enough" has "$(root_click)" "$TMP/plugin/target/release/paddock"
# Messages are only logged with a client attached.
check "nested client still attached" test -n "$(t list-clients)"
check "no missing-key warning" test -z "$(t show-messages 2>/dev/null | grep 'set @paddock-key')"

# No key and no click: prefix a, but never over a binding of the user's own.
t set -gu @paddock-click
load
check "no key set: prefix a opens paddock" has "$(t list-keys -T prefix a 2>/dev/null)" "$TMP/plugin/target/release/paddock"
t unbind a
t bind a display-message mine
load
check "no key set: a user's own prefix a survives" has "$(t list-keys -T prefix a 2>/dev/null)" "display-message mine"
check "no key set: paddock says prefix a is taken" has "$(t show-messages 2>/dev/null)" "prefix a is taken"
t unbind a
# Ours by its note, whatever the binary is called: a reload keeps prefix a.
stub "$TMUX_TMPDIR/pdk" 9.9.9
taken() { t show-messages 2>/dev/null | grep -c 'prefix a is taken'; }
before=$(taken)
load_bin "$TMUX_TMPDIR/pdk"
load_bin "$TMUX_TMPDIR/pdk"
check "no key set: a reload keeps prefix a for a binary not named paddock" has "$(t list-keys -T prefix a 2>/dev/null)" "$TMUX_TMPDIR/pdk"
check "no key set: and does not call it taken" test "$(taken)" -eq "$before"
# A changed key drops the old one.
t set -g @paddock-key b
load
check "a changed key is bound" has "$(t list-keys -T prefix b 2>/dev/null)" "paddock"
check "and the old one is dropped" test -z "$(t list-keys -T prefix a 2>/dev/null)"
t unbind b
t set -gu @paddock-key
t set -g @paddock-click on

# Which binary gets bound: only one reporting this checkout's version (and
# exiting 0), from target/release or bin/ and nowhere else; else the installer.
reset
load
check "without a binary, the click offers the installer" has "$(root_click)" "install.sh"
check "the installer binding carries a paddock note" has "$(t list-keys -N -T root MouseUp1StatusLeft 2>/dev/null)" "paddock install"

stub "$TMP/plugin/bin/paddock" 9.9.9
stub "$TMP/plugin/target/release/paddock" 0.0.1
load
check "wrong-version bin/ and target/release are rejected" has "$(root_click)" "install.sh"
check "a rejected binary is not bound" lacks "$(root_click)" "bin/paddock"

reset
stub "$TMP/plugin/bin/paddock" "$V"
load
check "a matching bin/ is used" has "$(root_click)" "$TMP/plugin/bin/paddock"
check "a matching bin/ means no installer" lacks "$(root_click)" "install.sh"

stub "$TMP/plugin/target/release/paddock" "$V"
load
check "a matching target/release beats bin/" has "$(root_click)" "$TMP/plugin/target/release/paddock"

stub "$TMP/plugin/target/release/paddock" 0.0.1
load
check "a stale target/release is skipped for a matching bin/" has "$(root_click)" "$TMP/plugin/bin/paddock"

reset
stub "$TMP/plugin/bin/paddock" "$V" 1
load
check "the right version with a nonzero exit is rejected" has "$(root_click)" "install.sh"

# Installed binaries are not probed: they use paddock init, and a foreign one
# could hang tmux at startup.
reset
stub "$TMP/.cargo/bin/paddock" "$V"
load
check "a matching ~/.cargo/bin is not bound" has "$(root_click)" "install.sh"
check "and was never run" test ! -e "$TMP/probed"

reset
stub "$TMP/custom/paddock" 9.9.9
load_bin "$TMP/custom/paddock"
check "PADDOCK_BIN is bound as given" has "$(root_click)" "$TMP/custom/paddock"
check "PADDOCK_BIN is not probed" test ! -e "$TMP/probed"
printf '#!/bin/sh\n' >"$TMP/notexec"
load_bin "$TMP/notexec"
check "a non-executable PADDOCK_BIN says so" has "$(root_click)" "$TMP/notexec is not executable"
load_bin "$TMP/missing"
check "a missing PADDOCK_BIN says so" has "$(root_click)" "$TMP/missing is not executable"

stub "$TMP/plugin/bin/paddock" "$V"
mv "$TMP/plugin/Cargo.toml" "$TMP/Cargo.toml.away"
out=$(load 2>&1); rc=$?
mv "$TMP/Cargo.toml.away" "$TMP/plugin/Cargo.toml"
check "without a Cargo.toml the script does not abort" test "$rc" -eq 0 -a -z "$out"
check "without a Cargo.toml the key says no paddock was found" has "$(root_click)" "no paddock found"

# An exported CDPATH must not steer the scripts' own cd: a relative script path
# plus a CDPATH holding a decoy of the same name would otherwise land in the decoy.
mkdir -p "$TMP/decoy/plugin/scripts"
t run-shell "cd $(tmq "$TMP") && CDPATH=$(tmq "$TMP/decoy") bash plugin/paddock.tmux"
check "CDPATH does not steer paddock.tmux" has "$(root_click)" "$TMP/plugin/bin/paddock"

# paddock init runs the script under bash -c: no BASH_SOURCE, no checkout.
mkdir "$TMP/initdir"
stub "$TMP/initbin/paddock" 9.9.9
cat >"$TMP/init" <<EOF
#!/bin/sh
cd "$TMP/initdir" && PADDOCK_VERSION=$V PADDOCK_BIN="$TMP/initbin/paddock" bash -c "\$(cat "$ROOT/paddock.tmux")" paddock.tmux
EOF
chmod +x "$TMP/init"
rm -f "$TMP/probed"
out=$(t run-shell "$TMP/init" 2>&1)
check "init mode runs silently" test -z "$out"
check "init mode binds the given binary" has "$(root_click)" "$TMP/initbin/paddock"
check "init mode hands over the version" has "$(root_click)" "-e PADDOCK_PLUGIN_VERSION=$V"
check "init mode does not probe" test ! -e "$TMP/probed"

# The real binary, if built: paddock init end to end, and the version handshake.
# Locally a missing build is a skip; in CI (which builds first) a failure.
PB=$ROOT/target/debug/paddock
if [ -x "$PB" ]; then
  # Cross-checks the shell's Cargo.toml parser against cargo's.
  cargo_v=$(awk -F'"' '/^\[/ { p = ($0 == "[package]") } p && /^version *=/ { print $2; exit }' "$ROOT/Cargo.toml")
  check "real paddock: --version is the Cargo.toml version" test "$("$PB" --version 2>&1)" = "paddock $cargo_v"
  t set -g @paddock-key a
  out=$(t run-shell "$(tmq "$PB") init" 2>&1)
  check "real paddock init runs silently" test -z "$out"
  check "real paddock init binds the binary that ran it" has "$(t list-keys -T prefix a)" "target/debug/paddock"
  check "real paddock init hands over its version" has "$(t list-keys -T prefix a)" "PADDOCK_PLUGIN_VERSION=$cargo_v"
  # Opened for real from the key: it attaches to a _paddock session it makes
  # (gone once closed), without running the user's pane-focus-in for it.
  t set-hook -g pane-focus-in "run-shell 'echo #{session_name} >>$(tmq "$TMP/focused")'"
  wait_screen "[main]"
  tmux -L "$OUT" send-keys C-b a
  check "real paddock: the key opens it" wait_screen "agents"
  check "real paddock: on its own session" t has-session -t =_paddock
  check "real paddock: no pane-focus-in hook for its session" lacks "$(cat "$TMP/focused" 2>/dev/null)" _paddock
  tmux -L "$OUT" send-keys Escape
  check "real paddock: closed, its session is gone" wait_gone "agents"
  check "real paddock: and so is its session" test -z "$(t list-sessions -F '#{session_name}' | grep _paddock)"
  t set-hook -gu pane-focus-in
  t set -gu @paddock-key
  # The binary refuses bindings from another version, with no tmux needed.
  out=$(PADDOCK_PLUGIN_VERSION=0.0.0-nope "$PB" 2>&1); rc=$?
  check "a stale binding exits 1" test "$rc" -eq 1
  check "a stale binding says to reload tmux" has "$out" "reload tmux"
elif [ -n "${CI:-}" ]; then
  echo "FAIL real paddock: $PB is not built (CI must cargo build first)"
  fails=$((fails + 1))
else
  echo "skip real paddock init: $PB is not built"
fi

# A plugin dir with a space, a ', a # and an = in its name, opened for real: a
# lone popup command goes through sh, which breaks on these; with its --tab
# tmux execs paddock directly. Both the click and the agents key.
Q="$TMP/it's a #dir=x"
mkdir -p "$Q/scripts"
cp "$TMP/plugin/paddock.tmux" "$TMP/plugin/Cargo.toml" "$Q/"
cp "$TMP/plugin/scripts/install.sh" "$Q/scripts/"
stub "$Q/bin/paddock" "$V"
t set -g @paddock-agents-key g
t unbind -n MouseUp1StatusLeft # else the real init's binding above would satisfy the checks
t run-shell "$(tmq "$Q/paddock.tmux")"
check "quoted dir: the click binds its binary" has "$(root_click)" "dir=x"
check "quoted dir: and it is bin/paddock, not the installer" has "$(root_click)" "bin/paddock"
rm -f "$TMP/opened"
wait_screen "[main]"
press 4d
press 6d
wait_opened
expected="$Q/bin/paddock $client $(t display -p -t main '#{session_id} #{window_id}')"
check "quoted dir: a click opens paddock" test "$(cat "$TMP/opened" 2>/dev/null)" = "$expected --tab last"
rm -f "$TMP/opened"
# The click's popup may still be closing, and a key sent into it is lost: resend until it lands.
for _ in $(seq 20); do
  tmux -L "$OUT" send-keys C-b g
  for _ in $(seq 3); do [ -s "$TMP/opened" ] || sleep 0.1; done
  [ -s "$TMP/opened" ] && break
done
check "quoted dir: the agents key opens paddock --tab agents" test "$(cat "$TMP/opened" 2>/dev/null)" = "$expected --tab agents"

# The installer, offline: releases are directories (PADDOCK_RELEASE_URL), built
# here from stubs. Every failure must say why, install nothing, leave no temp
# files and keep the previous bin/paddock.
TARGETS="aarch64-apple-darwin x86_64-apple-darwin aarch64-unknown-linux-musl x86_64-unknown-linux-musl"
sums() { # sums DIR: a .sha256 for every asset in DIR, made inside DIR as the release job does.
  local f
  for f in "$1"/*.tar.gz; do
    (cd "$1" && f=${f##*/} && { sha256sum "$f" 2>/dev/null || shasum -a 256 "$f"; } >"$f.sha256")
  done
}
release() { # release DIR STAGE: every target packed from what is in STAGE.
  local tg
  mkdir -p "$1"
  for tg in $TARGETS; do (cd "$2" && tar -czf "$1/paddock-$tg.tar.gz" -- *); done
  sums "$1"
}
stage() { # stage NAME VERSION: a stage dir with a stub paddock and an empty LICENSE.
  stub "$TMP/stage/$1/paddock" "$2"
  : >"$TMP/stage/$1/LICENSE"
}
stage good "$V"
release "$TMP/rel/good" "$TMP/stage/good"
stage oldver 9.9.9
release "$TMP/rel/oldver" "$TMP/stage/oldver"
mkdir -p "$TMP/stage/nobin" "$TMP/stage/link"
: >"$TMP/stage/nobin/LICENSE"
: >"$TMP/stage/link/LICENSE"
ln -s /bin/sh "$TMP/stage/link/paddock"
release "$TMP/rel/nobin" "$TMP/stage/nobin"
release "$TMP/rel/link" "$TMP/stage/link"
cp -R "$TMP/rel/good" "$TMP/rel/badsum"
cp -R "$TMP/rel/good" "$TMP/rel/othername"
cp -R "$TMP/rel/good" "$TMP/rel/notgz"
for tg in $TARGETS; do
  a=paddock-$tg.tar.gz
  printf '%064d  %s\n' 0 "$a" >"$TMP/rel/badsum/$a.sha256"
  printf '%s  paddock-other.tar.gz\n' "$(cut -d' ' -f1 "$TMP/rel/good/$a.sha256")" >"$TMP/rel/othername/$a.sha256"
  echo "not a tarball" >"$TMP/rel/notgz/$a"
done
sums "$TMP/rel/notgz"

# The Homebrew formula, offline from the same .sha256 files.
UF=$ROOT/scripts/update-formula.sh
if [ -f "$UF" ]; then
  formula=$(bash "$UF" --from "$TMP/rel/good" "$V" 2>&1); rc=$?
  check "formula: exits 0" test "$rc" -eq 0
  for tg in $TARGETS; do
    check "formula: $tg hash" has "$formula" "sha256 \"$(cut -d' ' -f1 "$TMP/rel/good/paddock-$tg.tar.gz.sha256")\""
    check "formula: $tg url" has "$formula" "url \"https://github.com/plopezlpz/paddock/releases/download/v$V/paddock-$tg.tar.gz\""
  done
  ruby=$(command -v ruby || { [ -x /usr/bin/ruby ] && echo /usr/bin/ruby; })
  if [ -n "$ruby" ]; then
    check "formula: ruby -c says Syntax OK" has "$(printf '%s\n' "$formula" | "$ruby" -c 2>&1)" "Syntax OK"
  else
    echo "skip formula syntax: no ruby"
  fi
fi

PLUG=$TMP/plugin
no_leftovers() { [ -z "$(find "$1/bin" -name '.install.*' 2>/dev/null)" ]; }
run_install() { # run_install RELEASE [PLUGIN]: the installer as a run-shell job, stderr included.
  t run-shell "PADDOCK_RELEASE_URL=$(tmq "$1") bash $(tmq "${2:-$PLUG}/scripts/install.sh") --yes 2>&1"
}
fails_cleanly() { # fails_cleanly NAME RELEASE MESSAGE
  reset
  load
  out=$(run_install "$2")
  check "$1: says why" has "$out" "$3"
  check "$1: installs nothing" test ! -e "$PLUG/bin/paddock"
  check "$1: leaves no temp files" no_leftovers "$PLUG"
  check "$1: the key still offers the installer" has "$(root_click)" "install.sh"
  check "$1: and the way out is printed" has "$out" "cargo build --release"
}
fails_cleanly "checksum mismatch" "$TMP/rel/badsum" "checksum mismatch -- nothing installed"
fails_cleanly "a checksum file naming another asset" "$TMP/rel/othername" "names another file"
fails_cleanly "a malformed archive" "$TMP/rel/notgz" "not a valid archive"
fails_cleanly "an archive without paddock" "$TMP/rel/nobin" "not a valid archive"
fails_cleanly "a symlinked paddock" "$TMP/rel/link" "no regular file paddock"
fails_cleanly "a wrong-version binary" "$TMP/rel/oldver" "not paddock $V"
fails_cleanly "an unpublished release" "$TMP/rel/missing" "is v$V released?"
fails_cleanly "a relative release URL" "rel/good" "release URL must be"
fails_cleanly "an http release URL" "http://example.com/rel" "release URL must be"

# A failed upgrade keeps the old binary, byte for byte.
reset
stub "$PLUG/bin/paddock" 0.0.1
cp "$PLUG/bin/paddock" "$TMP/old.copy"
load
out=$(run_install "$TMP/rel/badsum")
check "failed upgrade: says what was installed" has "$out" "paddock 0.0.1 is installed; this plugin needs paddock $V"
check "failed upgrade: the old binary is untouched" cmp -s "$PLUG/bin/paddock" "$TMP/old.copy"
check "failed upgrade: leaves no temp files" no_leftovers "$PLUG"

# Not writable: a separate copy, so the other checks keep their plugin.
mkdir -p "$TMP/ro/scripts"
cp "$PLUG/paddock.tmux" "$PLUG/Cargo.toml" "$TMP/ro/"
cp "$PLUG/scripts/install.sh" "$TMP/ro/scripts/"
if [ "$(id -u)" -eq 0 ]; then
  echo "skip unwritable plugin dir (root writes anywhere)"
else
  chmod a-w "$TMP/ro"
  out=$(run_install "$TMP/rel/good" "$TMP/ro")
  chmod u+w "$TMP/ro"
  check "unwritable plugin dir: says so" has "$out" "$TMP/ro is not writable -- use \`run-shell 'paddock init'\`"
  check "unwritable plugin dir: installs nothing" test ! -e "$TMP/ro/bin"
fi

# Outside tmux the keys cannot be rebound: install and say to reload.
reset
out=$(env -u TMUX PADDOCK_RELEASE_URL="$TMP/rel/good" bash "$PLUG/scripts/install.sh" --yes 2>&1)
check "outside tmux: installs" test "$("$PLUG/bin/paddock" --version 2>&1)" = "paddock $V"
check "outside tmux: says to reload" has "$out" "reload tmux"
check "outside tmux: leaves no temp files" no_leftovers "$PLUG"

# ...and CDPATH must not steer the installer's own cd (the decoy is made above).
reset
out=$(cd "$TMP" && env -u TMUX CDPATH="$TMP/decoy" PADDOCK_RELEASE_URL="$TMP/rel/good" bash plugin/scripts/install.sh --yes 2>&1)
check "CDPATH does not steer the installer" test "$("$PLUG/bin/paddock" --version 2>&1)" = "paddock $V"

# Good install inside tmux, over an older binary: the keys are rebound to it.
reset
stub "$PLUG/bin/paddock" 0.0.1
load
check "before the install the key offers it" has "$(root_click)" "install.sh"
out=$(run_install "$TMP/rel/good")
check "install: bin/paddock reports the plugin's version" test "$("$PLUG/bin/paddock" --version 2>&1)" = "paddock $V"
check "install: says to press the key again" has "$out" "Press the key again"
check "install: the key now opens bin/paddock" has "$(root_click)" "$PLUG/bin/paddock"
check "install: and no longer offers the installer" lacks "$(root_click)" "install.sh"
check "install: leaves no temp files" no_leftovers "$PLUG"

# End to end through the real key: the installer popup, then the real paddock.
# Keys go to the outer server, and only once the popup shows what they answer.
reset
rm -f "$TMP/opened"
load
t set-environment -g PADDOCK_RELEASE_URL "$TMP/rel/good"
wait_screen "[main]"
press 4d
press 6d
check "e2e: the installer asks first" wait_screen "[Y/n]"
check "e2e: nothing is installed before the answer" test ! -e "$PLUG/bin/paddock"
tmux -L "$OUT" send-keys Enter
check "e2e: it says to press the key again" wait_screen "Press the key again"
check "e2e: and waits for a key" wait_screen "Press any key to close."
tmux -L "$OUT" send-keys q
check "e2e: the popup closes" wait_gone "Press any key to close."
check "e2e: bin/paddock is installed" test "$("$PLUG/bin/paddock" --version 2>&1)" = "paddock $V"
wait_screen "[main]"
press 4d
press 6d
wait_opened
check "e2e: pressing the key again opens paddock" has "$(cat "$TMP/opened" 2>/dev/null)" "$PLUG/bin/paddock $client "

# A first install also offers to hook up the agents agent-status.tmux finds
# (a stub stands in for its manage-provider.sh); an upgrade doesn't ask again.
mkdir -p "$TMP/as/scripts/agent"
cat >"$TMP/as/manage-provider.sh" <<EOF
case "\$1" in
  found) exit 0 ;;
  install) printf 'agent-status: found fake.\nHook them up? [Y/n] '; read -r a; echo "\$a" >"$TMP/hooked" ;;
esac
EOF
t set -g @agent-scripts-dir "$TMP/as/scripts/agent"
t set-environment -g PADDOCK_RELEASE_URL "$TMP/rel/good"
wait_file() { for _ in $(seq 50); do [ -s "$1" ] && return 0; sleep 0.1; done; return 1; }
reset
rm -f "$TMP/hooked"
load
wait_screen "[main]"
press 4d
press 6d
check "e2e agents: the installer asks first" wait_screen "[Y/n]"
tmux -L "$OUT" send-keys Enter
check "e2e agents: then offers to hook up the agents" wait_screen "Hook them up?"
tmux -L "$OUT" send-keys y Enter
check "e2e agents: and hooks them up" wait_file "$TMP/hooked"
check "e2e agents: then says to press the key again" wait_screen "Press the key again"
tmux -L "$OUT" send-keys q
wait_gone "Press any key to close."
reset
rm -f "$TMP/hooked"
stub "$PLUG/bin/paddock" 0.0.1
load
wait_screen "[main]"
press 4d
press 6d
check "e2e upgrade: the installer asks first" wait_screen "[Y/n]"
tmux -L "$OUT" send-keys Enter
check "e2e upgrade: it installs" wait_screen "Press the key again"
check "e2e upgrade: without asking about agents" test ! -e "$TMP/hooked"
tmux -L "$OUT" send-keys q
wait_gone "Press any key to close."
t set -gu @agent-scripts-dir
t set-environment -gu PADDOCK_RELEASE_URL

# A failure stays on screen until a key is pressed.
reset
load
t set-environment -g PADDOCK_RELEASE_URL "$TMP/rel/badsum"
wait_screen "[main]"
press 4d
press 6d
check "e2e failure: the installer asks first" wait_screen "[Y/n]"
tmux -L "$OUT" send-keys Enter
check "e2e failure: the message waits for a key" wait_screen "Press any key to close."
sleep 0.5
check "e2e failure: and is still there" has "$(tmux -L "$OUT" capture-pane -p)" "Press any key to close."
tmux -L "$OUT" send-keys q
check "e2e failure: the popup closes" wait_gone "Press any key to close."
check "e2e failure: nothing is installed" test ! -e "$PLUG/bin/paddock"
check "e2e failure: leaves no temp files" no_leftovers "$PLUG"
t set-environment -gu PADDOCK_RELEASE_URL

if [ "$fails" -eq 0 ]; then echo "all passed"; else echo "$fails failed"; exit 1; fi
