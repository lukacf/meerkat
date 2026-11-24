# Shared by the Linux tmux suites. Only their fixed prompts are typed into
# the editor (none equals its empty placeholder). Never clear or retry a draft.
tux_editor_is_visibly_empty() {
  local -a rows
  local empty='^│\[[^]]+\] > Type a command or /help_[ ]*│$'
  mapfile -t rows
  (( ${#rows[@]} >= 6 )) &&
    [[ ${rows[-6]} == '┌COMMAND  '* && ${rows[-5]} =~ $empty &&
       ${rows[-3]} == '└'*'┘' && ${rows[-1]} == '[Tab] mode  [Up/Down] select'* ]]
}

send_prompt() {
  local prompt="$1" pane
  if ! pane="$(tmux capture-pane -t "$session_name" -p)" ||
     ! tux_editor_is_visibly_empty <<< "$pane"; then
    printf 'Stopped before typing: cannot verify an empty TUX command field. Inspect retained pane %s; no keys sent.\n' "$session_name" >&2
    return 1
  fi
  tmux send-keys -t "$session_name" -l "$prompt" || return 1
  tmux send-keys -t "$session_name" Enter
}
