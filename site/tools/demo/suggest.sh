#!/bin/sh
# Demo stub for @agent-suggest-command (paddock's suggestion contract: run with the
# pane id appended, print a title on stdout, or fail with the reason on stderr).
# It stands in for agent-status.tmux's describe.sh, which asks pi; it waits a
# moment like a real model would, then answers from a canned map. The ids are
# the ones the recorder's fake sessions get (feat-rename checks them).
sleep 0.8
case "$1" in
  %0) echo "Rate limiting" ;;
  %2) echo "Cart payments" ;;
  %3) echo "Staging network" ;;
  %5) echo "Learning schedule" ;;
  %6) echo "Getting started" ;;
  *) echo "no demo title for $1" >&2; exit 1 ;;
esac
