#!/usr/bin/env bash
# paddock installer: downloads the release binary matching this checkout's
# version, verifies it, installs it to bin/paddock and rebinds the keys. Run by
# paddock.tmux's installer popup, or by hand (works outside tmux too).
# Usage: scripts/install.sh [--yes|-y]
# Env: PADDOCK_RELEASE_URL: absolute directory or https:// URL holding the assets.
set -euf

yes=
case "${1:-}" in
  '') ;;
  --yes | -y) yes=1 ;;
  *) echo 'usage: scripts/install.sh [--yes|-y]' >&2; exit 2 ;;
esac
[ $# -le 1 ] || { echo 'usage: scripts/install.sh [--yes|-y]' >&2; exit 2; }

# pause: the popup closes the moment we exit, so hold the message on screen.
pause() {
  [ -n "$yes" ] && return 0
  printf 'Press any key to close.'
  read -rsn1 _ || true
}
# The one exit path: clean up, and on any failure (ours or set -e's) keep the
# message visible. A failure never touches the previously installed bin/paddock.
tmp=
on_exit() {
  local rc=$?
  [ -z "$tmp" ] || rm -rf "$tmp"
  [ "$rc" -eq 0 ] || pause
}
trap on_exit EXIT

# CDPATH='' : an exported CDPATH would make cd search elsewhere and print.
PLUGIN_DIR=$(CDPATH='' cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)

die() { echo "paddock: $1" >&2; exit 1; }
fail() { # fail MSG: die, with the way out that needs no download.
  echo "paddock: $1" >&2
  echo "Or build it: cd $(printf %q "$PLUGIN_DIR") && cargo build --release, then reload tmux." >&2
  exit 1
}

# Duplicated from paddock.tmux on purpose, so the installer stands alone.
toml() { # toml KEY: a string value from Cargo.toml's [package].
  awk -F'"' -v k="$1" '/^\[/ { p = ($0 == "[package]") } p && $0 ~ "^" k " *=" { print $2; exit }' "$PLUGIN_DIR/Cargo.toml" 2>/dev/null || true
}
VERSION=$(toml version)
[ -n "$VERSION" ] || fail "cannot read the version from Cargo.toml"

os=$(uname -s) arch=$(uname -m)
case "$os/$arch" in
  Darwin/arm64 | Darwin/aarch64) target=aarch64-apple-darwin ;;
  Darwin/x86_64)
    # An x86_64 shell on Apple silicon (Rosetta) wants the native binary.
    if [ "$(/usr/sbin/sysctl -n sysctl.proc_translated 2>/dev/null || true)" = 1 ]; then
      target=aarch64-apple-darwin
    else
      target=x86_64-apple-darwin
    fi ;;
  Linux/aarch64 | Linux/arm64) target=aarch64-unknown-linux-musl ;;
  Linux/x86_64 | Linux/amd64) target=x86_64-unknown-linux-musl ;;
  *) fail "no prebuilt paddock for $os $arch" ;;
esac
asset=paddock-$target.tar.gz

# The directory holding the assets. The default follows Cargo.toml's
# repository, so a fork downloads from itself.
base=${PADDOCK_RELEASE_URL:-}
if [ -z "$base" ]; then
  repo=$(toml repository)
  [ -n "$repo" ] || fail "cannot read the repository from Cargo.toml"
  base=${repo%/}/releases/download/v$VERSION
fi
case "$base" in
  /* | https://*) base=${base%/} ;;
  *) fail "the release URL must be an absolute directory or an https:// URL, not '$base'" ;;
esac

old=
if [ -x "$PLUGIN_DIR/bin/paddock" ]; then
  old=$("$PLUGIN_DIR/bin/paddock" --version 2>/dev/null </dev/null | head -n 1 || true)
fi
case "$old" in
  "paddock $VERSION") ;;
  "paddock "*) echo "$old is installed; this plugin needs paddock $VERSION" ;;
esac

if [ -z "$yes" ]; then
  # [Y/n] last on a short line: a long source URL wraps in a narrow popup.
  printf 'Download paddock %s for %s from\n  %s\nProceed? [Y/n] ' "$VERSION" "$target" "$base"
  read -r ans || ans=n
  case "$ans" in
    '' | y* | Y*) ;;
    *) echo "Not installed."; pause; exit 0 ;;
  esac
fi

# In bin/, so the final mv is a rename on one filesystem, never a half-written
# bin/paddock.
{ mkdir -p "$PLUGIN_DIR/bin" && tmp=$(mktemp -d "$PLUGIN_DIR/bin/.install.XXXXXX"); } 2>/dev/null ||
  die "$PLUGIN_DIR is not writable -- use \`run-shell 'paddock init'\` with an installed paddock instead"

fetch() { # fetch NAME: into $tmp. https only, redirects included.
  case "$base" in
    /*) cp "$base/$1" "$tmp/$1" ;;
    *) curl -fsSL --proto '=https' --proto-redir '=https' -o "$tmp/$1" "$base/$1" ;;
  esac
}
case "$base" in
  /*) ;;
  *) command -v curl >/dev/null || fail "needs curl" ;;
esac
for f in "$asset" "$asset.sha256"; do
  fetch "$f" || fail "cannot download $base/$f (is v$VERSION released?)"
done

# The checksum guards against truncation and corruption, not a compromised
# release: it has the same origin as the tarball. The file is validated and the
# hashes compared here, since -c would trust whatever file names it lists.
sumfile=$tmp/$asset.sha256
[ "$(awk 'END { print NR }' "$sumfile")" = 1 ] || fail "$asset.sha256 is not a single line"
read -r want name extra <"$sumfile" || true
case "$want" in *[!0-9a-f]*) want= ;; esac
[ "${#want}" -eq 64 ] && [ -z "$extra" ] && [ "${name#\*}" = "$asset" ] ||
  fail "$asset.sha256 is malformed or names another file"
if command -v sha256sum >/dev/null; then got=$(sha256sum <"$tmp/$asset")
elif command -v shasum >/dev/null; then got=$(shasum -a 256 <"$tmp/$asset")
else fail "needs sha256sum or shasum"; fi
[ "${got%% *}" = "$want" ] || fail "checksum mismatch -- nothing installed"

# Only the paddock member comes out, and not as a link to somewhere else.
mkdir "$tmp/x"
tar -xzf "$tmp/$asset" -C "$tmp/x" paddock 2>/dev/null || fail "$asset is not a valid archive holding paddock"
{ [ -f "$tmp/x/paddock" ] && [ ! -L "$tmp/x/paddock" ]; } || fail "$asset holds no regular file paddock"
chmod 755 "$tmp/x/paddock"
got=$("$tmp/x/paddock" --version 2>/dev/null </dev/null) || got="${got:-nothing} (it failed to run)"
[ "$got" = "paddock $VERSION" ] || fail "the download is '$got', not paddock $VERSION"

mv -f "$tmp/x/paddock" "$PLUGIN_DIR/bin/paddock" || fail "cannot write $PLUGIN_DIR/bin/paddock"

# A first install, answered in the popup: offer to hook up the agents that
# agent-status.tmux finds (it asks itself). Upgrades and --yes runs don't ask.
hook_agents() {
  local dir mp rc=0
  { [ -z "$old" ] && [ -z "$yes" ] && [ -t 0 ]; } || return 0
  dir=$(tmux show-option -gqv @agent-scripts-dir 2>/dev/null) || return 0
  mp=${dir%/scripts/agent}/manage-provider.sh
  { [ -n "$dir" ] && [ -f "$mp" ]; } || return 0
  # An agent-status.tmux without "found" can't list agents: leave it to its README.
  bash "$mp" found claude >/dev/null 2>&1 || rc=$?
  [ "$rc" -ne 2 ] || return 0
  echo
  bash "$mp" install || echo "Hooking up the agents failed; later, run: bash $(printf %q "$mp") install"
}

if [ -n "${TMUX:-}" ]; then
  bash "$PLUGIN_DIR/paddock.tmux" || die "paddock $VERSION installed, but rebinding failed -- reload tmux"
  echo "paddock $VERSION installed."
  hook_agents
  echo "Press the key again to open it."
else
  echo "paddock $VERSION installed; reload tmux (outside tmux the keys cannot be rebound)."
fi
pause
