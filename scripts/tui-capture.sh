#!/usr/bin/env bash
# TUI capture helper — launch aops TUI in tmux and capture screenshots.
# Usage:
#   ./scripts/tui-capture.sh start    — build and launch TUI in tmux
#   ./scripts/tui-capture.sh capture  — capture current pane content (plain text)
#   ./scripts/tui-capture.sh key KEY  — send a key to the TUI (e.g., 'f', 'Tab', 'Enter')
#   ./scripts/tui-capture.sh stop     — kill the tmux session
#   ./scripts/tui-capture.sh restart  — rebuild, kill, and relaunch

SESSION="aops-tui"
BINARY="./target/release/aops"
WIDTH=120
HEIGHT=40

case "${1:-capture}" in
  start)
    tmux kill-session -t "$SESSION" 2>/dev/null
    cargo build --release 2>&1 | tail -3
    tmux new-session -d -s "$SESSION" -x "$WIDTH" -y "$HEIGHT" "$BINARY tui"
    sleep 1
    echo "TUI launched in tmux session '$SESSION'"
    ;;

  capture)
    tmux capture-pane -t "$SESSION" -p -e 2>/dev/null || echo "No tmux session '$SESSION' running"
    ;;

  key)
    shift
    tmux send-keys -t "$SESSION" "$@"
    sleep 0.3
    tmux capture-pane -t "$SESSION" -p -e
    ;;

  stop)
    tmux kill-session -t "$SESSION" 2>/dev/null
    echo "Session stopped"
    ;;

  restart)
    tmux kill-session -t "$SESSION" 2>/dev/null
    cargo build --release 2>&1 | tail -3
    tmux new-session -d -s "$SESSION" -x "$WIDTH" -y "$HEIGHT" "$BINARY tui"
    sleep 1
    echo "TUI restarted"
    tmux capture-pane -t "$SESSION" -p -e
    ;;

  *)
    echo "Usage: $0 {start|capture|key KEY|stop|restart}"
    ;;
esac
