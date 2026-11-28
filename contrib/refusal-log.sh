# refusal-log.sh: sourced by contrib/slot-wrapper. refusal_log <kind> <text> appends one
# tab-separated line: time, kind, the agent's session id, working directory, text, to
# ~/.local/state/slot/refusals.log.
refusal_log() {
  printf '%s\t%s\t%s\t%s\t%s\n' "$(date '+%F %T')" "$1" \
    "${CLAUDE_CODE_SESSION_ID:-${CODEX_SESSION_ID:-no session id}}" "$PWD" "$2" \
    >> "$HOME/.local/state/slot/refusals.log" 2>/dev/null
}
