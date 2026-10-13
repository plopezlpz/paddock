#!/usr/bin/env bash
# paddock -- tmux entrypoint (TPM or run-shell): binds @paddock-key (the tab paddock
# was last closed on) and @paddock-agents-key (agents tab) to the paddock popup.
# Both unbound unless set.
set -euf
CURRENT_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"
# A build in this checkout, else an installed binary. The tmux server's PATH
# often lacks ~/.cargo/bin, so cargo install's default location is checked too.
BIN="$CURRENT_DIR/target/release/paddock"
[ -x "$BIN" ] || BIN=$(command -v paddock 2>/dev/null || printf '%s' "$HOME/.cargo/bin/paddock")

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
if [ -z "$key$agents_key" ]; then
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
bin=$(literal "$BIN")

# Borderless popup (paddock draws its own frame) sized per invoking client.
# run-shell -C expands the formats, so -e hands paddock the invoking client
# and location as literal values. -EE: the popup closes when paddock exits
# cleanly, and stays open showing the error when it fails.
popup="display-popup -B -EE -c #{q:client_name} -w $w -h $h -s $style"
popup="$popup -e PADDOCK_CLIENT=#{q:client_name} -e PADDOCK_SESSION=#{q:session_id} -e PADDOCK_WINDOW=#{q:window_id}"

# bind_tab KEY [TAB]: without TAB, paddock reopens the tab it was closed on.
bind_tab() {
  [ -n "$1" ] || return 0
  if [ -x "$BIN" ]; then
    tmux bind -N "paddock${2:+ $2}" "$1" run-shell -C "$popup $bin${2:+ --tab $2}"
  else
    tmux bind -N "paddock${2:+ $2}" "$1" display-message "paddock: not built -- run cargo build --release in $CURRENT_DIR, then reload tmux"
  fi
}
bind_tab "$key"
bind_tab "$agents_key" agents
