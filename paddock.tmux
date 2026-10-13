#!/usr/bin/env bash
# paddock -- tmux entrypoint (TPM or run-shell): binds @paddock-key (the tab paddock
# was last closed on) and @paddock-agents-key (agents tab) to the paddock popup,
# and with @paddock-click on, a click on the session name (status-left). All
# unbound unless set. The binary must report this plugin's version: the popup's
# flags and env are a contract between the two.
set -euf
CURRENT_DIR='' VERSION='' BIN='' install=''
if [ -n "${PADDOCK_VERSION:-}" ]; then
  # paddock init (bash -c, so BASH_SOURCE is empty and there is no checkout):
  # the binary vouches for itself.
  VERSION=$PADDOCK_VERSION BIN=${PADDOCK_BIN:-}
else
  # CDPATH='' : an exported CDPATH would make cd search elsewhere and print.
  CURRENT_DIR="$( CDPATH='' cd -- "$( dirname -- "${BASH_SOURCE[0]}" )" && pwd )"
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

# paddock needs tmux 3.8: it empties the client-closed hook (new in 3.8) on
# its session. Ask the server for the hook rather than parse version strings
# (3.8a, next-3.9, master, openbsd-7.6).
if ! tmux show-hooks -g client-closed >/dev/null 2>&1; then
  tmux display-message "paddock: needs tmux 3.8 or later (this server is $(tmux display -p '#{version}'))"
  exit 0
fi

tmux set-option -gqo @paddock-popup-width  '80%'
tmux set-option -gqo @paddock-popup-height '80%'
# The terminal's own colours, not tmux's default popup style.
tmux set-option -gqo @paddock-popup-style  'bg=terminal,fg=terminal'

key=$(tmux show-option -gqv @paddock-key)
agents_key=$(tmux show-option -gqv @paddock-agents-key)
click=$(tmux show-option -gqv @paddock-click)
[ "$click" = on ] || click=
if [ -z "$key$agents_key$click" ]; then
  tmux display-message "paddock: set @paddock-key before loading paddock.tmux"
fi
# On reload, drop the keys bound last time (the options may have changed),
# unless the config has since bound one to something else.
for old in $(tmux show-option -gqv @paddock-bound-keys); do
  case $(tmux list-keys -N -T prefix -- "$old" 2>/dev/null) in
    *paddock*) tmux unbind -- "$old" ;;
  esac
done
tmux set-option -gq @paddock-bound-keys "$key $agents_key"
case $(tmux list-keys -N -T root MouseUp1StatusLeft 2>/dev/null) in
  *paddock*) tmux unbind -n MouseUp1StatusLeft ;;
esac

# arg VALUE: VALUE as one single-quoted tmux argument.
arg() { printf "'%s'" "$(printf '%s' "$1" | sed "s/'/'\\\\''/g")"; }
# literal VALUE: arg, with # doubled so run-shell -C's format expansion
# leaves it as written (#FFFFFF would otherwise expand #F).
literal() { arg "$(printf '%s' "$1" | sed 's/#/##/g')"; }

# Width and height are formats, expanded per client when the key is pressed
# (e.g. full size on a small terminal); style and paths are literal.
w=$(arg "$(tmux show-option -gqv @paddock-popup-width)")
h=$(arg "$(tmux show-option -gqv @paddock-popup-height)")
style=$(literal "$(tmux show-option -gqv @paddock-popup-style)")
# Always 2+ args after the flags: tmux execs those directly, while a lone
# command goes through a shell, which breaks on a space or ' in the path.
# sh -c 'exec "$0" "$@"' takes BIN as $0, never parsed (env would take a
# NAME=value path for an assignment); the single quotes keep $ from tmux.
# shellcheck disable=SC2016 # the $ are for sh, not for us
bin="$(literal /bin/sh) $(literal -c) $(literal 'exec "$0" "$@"') $(literal "$BIN")"
if [ -z "$BIN" ] && [ -n "$VERSION" ] && [ -n "$CURRENT_DIR" ] && [ -f "$CURRENT_DIR/scripts/install.sh" ]; then
  # No -B: the installer gets tmux's border and title. -E, not -EE: it waits
  # for a key itself.
  install="display-popup -E -c #{q:client_name} -w $w -h $h -s $style -T paddock $(literal bash) $(literal "$CURRENT_DIR/scripts/install.sh")"
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
    tmux bind -N "paddock${3:+ $3}" -T "$1" "$2" run-shell -C "$popup $bin${3:+ --tab $3}"
  elif [ -n "$install" ]; then
    tmux bind -N "paddock install" -T "$1" "$2" run-shell -C "$install"
  else
    # Messages are formats too, so a # in a path is doubled.
    if [ -n "$BIN" ]; then msg="$BIN is not executable"
    else msg="no paddock${VERSION:+ $VERSION} found -- see the README's Install section"; fi
    tmux bind -N "paddock${3:+ $3}" -T "$1" "$2" display-message "paddock: $(printf '%s' "$msg" | sed 's/#/##/g')"
  fi
}
bind_tab prefix "$key"
bind_tab prefix "$agents_key" agents
# On release, not press: the popup opens once the click is over, so the
# release can't land on a paddock row. Copy mode lets it through too.
bind_tab root "${click:+MouseUp1StatusLeft}"
