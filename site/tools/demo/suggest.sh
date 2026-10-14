#!/bin/sh
# Demo stub for @agent-suggest-command (paddock's suggestion contract: run with the
# pane id appended, print a title on stdout, or fail with the reason on stderr).
# It stands in for agent-status.tmux's describe.sh, which asks pi; it waits a
# moment like a real model would, then answers from a canned map. The ids are
# the ones the recorder's fake sessions get (feat-rename checks them).
sleep 0.8
case "$1" in
  %0) echo "Token-bucket rate limiter for checkout" ;;
  %2) echo "Payment element on the cart page" ;;
  %3) echo "Staging onto the new VPC module" ;;
  %5) echo "Cosine LR schedule with warmup" ;;
  %6) echo "Rewrite the getting-started guide" ;;
  *) echo "no demo title for $1" >&2; exit 1 ;;
esac
