#!/usr/bin/env bash
# paddock -- tmux entrypoint (TPM or run-shell): binds @paddock-key (the tab paddock
# was last closed on) and @paddock-agents-key (agents tab) to the paddock popup,
# and with @paddock-click on, a click on the session name (status-left). With
# none of the three set, @paddock-key is a, unless the config binds prefix a to
# something else. The binary must report this plugin's version: the popup's
# flags and env are a contract between the two.
set -euf
CURRENT_DIR='' VERSION='' BIN='' install=''
w='' h='' style='' bin='' script='' # set by arg and literal, below
if [ -n "${PADDOCK_VERSION:-}" ]; then
  # paddock init (bash -c, so BASH_SOURCE is empty and there is no checkout):
  # the binary vouches for itself.
  VERSION=$PADDOCK_VERSION BIN=${PADDOCK_BIN:-}
else
  # CDPATH='' : an exported CDPATH would make cd search elsewhere and print.
  src=${BASH_SOURCE[0]}
  case $src in */*) ;; *) src=./$src ;; esac
  CURRENT_DIR="$( CDPATH='' cd -- "${src%/*}" && pwd )"
  VERSION=$(awk -F'"' '/^\[/ { p = ($0 == "[package]") } p && /^version *=/ { print $2; exit }' "$CURRENT_DIR/Cargo.toml" 2>/dev/null || true)
  if [ -n "${PADDOCK_BIN:-}" ]; then
    BIN=$PADDOCK_BIN # the user's choice: trusted as-is, no probe
  else
    # BIN is paddock $VERSION and exits 0: old builds and other tools named
    # paddock fail. Only this checkout's own builds are probed (a foreign
    # binary could hang tmux at startup); installed ones use paddock init.
    matches() {
      local out
      [ -n "$VERSION" ] && [ -x "$1" ] && out=$("$1" --version 2>/dev/null </dev/null) &&
        [ "$out" = "paddock $VERSION" ]
    }
    for c in "$CURRENT_DIR/target/release/paddock" "$CURRENT_DIR/bin/paddock"; do
      if matches "$c"; then BIN=$c; break; fi
    done
  fi
fi

# One tmux call reads everything and one writes everything: each call is a
# process, and this runs at every server start and reload.
SEP=$'\037'
# paddock needs tmux 3.8: it empties the client-closed hook (new in 3.8) on
# its session. Ask the server for the hook rather than parse version strings
# (3.8a, next-3.9, master, openbsd-7.6): before 3.8 show-hooks fails, and so
# does the whole call. The popup's defaults go first so the display shows
# them; the style is the terminal's own colours, not tmux's default popup
# style. The key tables list each key with its note, or its command if it
# has none.
if ! out=$(tmux set-option -gqo @paddock-popup-width '80%' \; \
    set-option -gqo @paddock-popup-height '80%' \; \
    set-option -gqo @paddock-popup-style 'bg=terminal,fg=terminal' \; \
    display-message -p "#{@paddock-key}$SEP#{@paddock-agents-key}$SEP#{@paddock-click}$SEP#{@paddock-popup-width}$SEP#{@paddock-popup-height}$SEP#{@paddock-popup-style}" \; \
    show-hooks -g client-closed \; \
    list-keys -aN -P prefix -T prefix \; \
    list-keys -aN -P root -T root 2>/dev/null); then
  case $'\n'$out in
    # The hook was shown, so a key table failed: one with no keys does not
    # exist (unbind -a empties prefix). The call stopped there, so the
    # root table is listed now; it is empty if it was the one missing.
    *$'\n'client-closed*) out+=$'\n'$(tmux list-keys -aN -P root -T root 2>/dev/null || true) ;;
    *)
      tmux display-message "paddock: needs tmux 3.8 or later (this server is $(tmux display -p '#{version}'))"
      exit 0
      ;;
  esac
fi

# writes: the tmux commands this run sends, each followed by its ;.
writes=()
later() { writes+=("$@" \;); }
taken=
{
  IFS=$SEP read -r key agents_key click width height style
  # Bindings with a paddock note are ours, from the last run: dropped, as the
  # options may have changed. One the config has since rebound has its own.
  while read -r table k desc; do
    case $table:$desc in
      prefix:paddock | 'prefix:paddock '* | root:paddock | 'root:paddock '*) later unbind -T "$table" -- "$k" ;;
      prefix:*) [ "$k" != a ] || taken=1 ;;
    esac
  done
} <<EOF
$out
EOF
[ "$click" = on ] || click=
if [ -z "$key$agents_key$click" ]; then
  # Nothing chosen: prefix a, but never over a binding of the user's own.
  if [ -z "$taken" ]; then key=a
  else later display-message "paddock: prefix a is taken; set @paddock-key to choose a key"; fi
fi

# arg NAME VALUE: set NAME to VALUE as one single-quoted tmux argument.
arg() { local q="'" e="'\\''"; printf -v "$1" "'%s'" "${2//$q/$e}"; }
# literal NAME VALUE: arg, with # doubled so run-shell -C's format expansion
# leaves it as written (#FFFFFF would otherwise expand #F).
literal() { arg "$1" "${2//\#/##}"; }

# Width and height are formats, expanded per client when the key is pressed
# (e.g. full size on a small terminal); style and paths are literal.
arg w "$width"
arg h "$height"
literal style "$style"
# Always 2+ args after the flags (BIN --tab TAB): tmux execs those directly,
# while a lone command goes through a shell, which breaks on a space or ' in
# the path.
literal bin "$BIN"
if [ -z "$BIN" ] && [ -n "$VERSION" ] && [ -n "$CURRENT_DIR" ] && [ -f "$CURRENT_DIR/scripts/install.sh" ]; then
  # No -B: the installer gets tmux's border and title. -E, not -EE: it waits
  # for a key itself.
  literal script "$CURRENT_DIR/scripts/install.sh"
  install="display-popup -E -c #{q:client_name} -w $w -h $h -s $style -T paddock 'bash' $script"
fi

# Borderless popup (paddock draws its own frame) sized per invoking client.
# run-shell -C expands the formats, so -e hands paddock the invoking client
# and location as literal values. -EE: the popup closes when paddock exits
# cleanly, and stays open showing the error when it fails.
popup="display-popup -B -EE -c #{q:client_name} -w $w -h $h -s $style"
popup="$popup -e PADDOCK_CLIENT=#{q:client_name} -e PADDOCK_SESSION=#{q:session_id} -e PADDOCK_WINDOW=#{q:window_id}"
# The binary refuses to run against bindings from another version. Unquoted:
# a version is semver, so no space, quote or #.
[ -z "$VERSION" ] || popup="$popup -e PADDOCK_PLUGIN_VERSION=$VERSION"

# bind_tab TABLE KEY [TAB]: without TAB, paddock reopens the tab it was closed on.
bind_tab() {
  [ -n "$2" ] || return 0
  if [ -n "$BIN" ] && [ -x "$BIN" ]; then
    later bind -N "paddock${3:+ $3}" -T "$1" "$2" run-shell -C "$popup $bin --tab ${3:-last}"
  elif [ -n "$install" ]; then
    later bind -N "paddock install" -T "$1" "$2" run-shell -C "$install"
  else
    # Messages are formats too, so a # in a path is doubled.
    if [ -n "$BIN" ]; then msg="$BIN is not executable"
    else msg="no paddock${VERSION:+ $VERSION} found -- see the README's Install section"; fi
    later bind -N "paddock${3:+ $3}" -T "$1" "$2" display-message "paddock: ${msg//\#/##}"
  fi
}
bind_tab prefix "$key"
bind_tab prefix "$agents_key" agents
# On release, not press: the popup opens once the click is over, so the
# release can't land on a paddock row. Copy mode lets it through too.
bind_tab root "${click:+MouseUp1StatusLeft}"
[ ${#writes[@]} -eq 0 ] || tmux "${writes[@]}"
