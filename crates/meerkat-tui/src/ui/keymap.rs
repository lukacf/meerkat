//! Central key → action mapping for the main (non-modal) view.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Lines scrolled by Shift+Up/Down and the mouse wheel.
pub const LINE_SCROLL: usize = 3;

/// Editing operations on the input editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditAction {
    /// Insert a character.
    Insert(char),
    /// Delete before the cursor.
    Backspace,
    /// Delete under the cursor.
    Delete,
    /// Cursor left.
    Left,
    /// Cursor right.
    Right,
    /// Line start.
    Home,
    /// Line end.
    End,
    /// Previous word.
    WordLeft,
    /// Next word.
    WordRight,
    /// Line up / older history.
    Up,
    /// Line down / newer history.
    Down,
    /// Delete previous word.
    DeleteWordBack,
    /// Delete to line start.
    KillToLineStart,
}

/// What a key press means in the main view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Send the prompt (or run the slash command).
    Submit,
    /// Insert a newline into the prompt.
    Newline,
    /// Ctrl-C: stop / interrupt / clear / quit depending on state.
    CtrlC,
    /// Ctrl-D: quit when the input is empty, otherwise delete forward.
    CtrlD,
    /// Esc: recall pending messages, else interrupt a running turn.
    Escape,
    /// Toggle collapse of the last reasoning block (Ctrl-O).
    ToggleReasoning,
    /// Expand or collapse the output of the most recent tool call (Ctrl-T).
    ToggleToolOutput,
    /// Copy the mouse selection again, or else the last assistant message (Ctrl-Y).
    Copy,
    /// Start a reverse history search (Ctrl-R).
    HistorySearch,
    /// Ctrl-X: the first key of a chord, resolved by the app with the next key.
    ChordPrefix,
    /// Scroll the transcript up by lines.
    ScrollUp(usize),
    /// Scroll the transcript down by lines.
    ScrollDown(usize),
    /// Page up.
    PageUp,
    /// Page down.
    PageDown,
    /// Jump to the top of the transcript.
    ScrollTop,
    /// Jump to the bottom and follow new output.
    ScrollBottom,
    /// Move the helper panel selection up (Ctrl+Up; the newest first).
    HelperPrev,
    /// Move the helper panel selection down (Ctrl+Down; the newest first).
    HelperNext,
    /// Edit the input.
    Edit(EditAction),
    /// Unbound key.
    None,
}

/// Map a key event to an action. `input_empty` decides whether plain Home/End scroll
/// the transcript (empty input) or move the cursor (non-empty input).
pub fn map_key(key: &KeyEvent, input_empty: bool) -> Action {
    let m = key.modifiers;
    let ctrl = m.contains(KeyModifiers::CONTROL);
    let alt = m.contains(KeyModifiers::ALT);
    let shift = m.contains(KeyModifiers::SHIFT);
    match key.code {
        KeyCode::Enter if shift || alt || ctrl => Action::Newline,
        KeyCode::Enter => Action::Submit,
        KeyCode::Char('j') if ctrl => Action::Newline,
        KeyCode::Char('c') if ctrl => Action::CtrlC,
        KeyCode::Char('d') if ctrl => Action::CtrlD,
        KeyCode::Char('o') if ctrl => Action::ToggleReasoning,
        KeyCode::Char('t') if ctrl => Action::ToggleToolOutput,
        KeyCode::Char('r') if ctrl => Action::HistorySearch,
        KeyCode::Char('y') if ctrl => Action::Copy,
        KeyCode::Char('x') if ctrl => Action::ChordPrefix,
        KeyCode::Char('a') if ctrl => Action::Edit(EditAction::Home),
        KeyCode::Char('e') if ctrl => Action::Edit(EditAction::End),
        KeyCode::Char('w') if ctrl => Action::Edit(EditAction::DeleteWordBack),
        KeyCode::Char('u') if ctrl => Action::Edit(EditAction::KillToLineStart),
        KeyCode::Char('b') if alt => Action::Edit(EditAction::WordLeft),
        KeyCode::Char('f') if alt => Action::Edit(EditAction::WordRight),
        KeyCode::Char(_) if ctrl || alt => Action::None,
        KeyCode::Char(c) => Action::Edit(EditAction::Insert(c)),
        KeyCode::Esc => Action::Escape,
        KeyCode::Backspace if ctrl || alt => Action::Edit(EditAction::DeleteWordBack),
        KeyCode::Backspace => Action::Edit(EditAction::Backspace),
        KeyCode::Delete => Action::Edit(EditAction::Delete),
        KeyCode::Left if ctrl || alt => Action::Edit(EditAction::WordLeft),
        KeyCode::Right if ctrl || alt => Action::Edit(EditAction::WordRight),
        KeyCode::Left => Action::Edit(EditAction::Left),
        KeyCode::Right => Action::Edit(EditAction::Right),
        KeyCode::Up if ctrl => Action::HelperPrev,
        KeyCode::Down if ctrl => Action::HelperNext,
        KeyCode::Up if shift => Action::ScrollUp(LINE_SCROLL),
        KeyCode::Down if shift => Action::ScrollDown(LINE_SCROLL),
        KeyCode::Up => Action::Edit(EditAction::Up),
        KeyCode::Down => Action::Edit(EditAction::Down),
        KeyCode::PageUp => Action::PageUp,
        KeyCode::PageDown => Action::PageDown,
        KeyCode::Home if ctrl || input_empty => Action::ScrollTop,
        KeyCode::End if ctrl || input_empty => Action::ScrollBottom,
        KeyCode::Home => Action::Edit(EditAction::Home),
        KeyCode::End => Action::Edit(EditAction::End),
        _ => Action::None,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn k(code: KeyCode, m: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, m)
    }

    #[test]
    fn enter_variants() {
        assert_eq!(
            map_key(&k(KeyCode::Enter, KeyModifiers::NONE), true),
            Action::Submit
        );
        assert_eq!(
            map_key(&k(KeyCode::Enter, KeyModifiers::SHIFT), true),
            Action::Newline
        );
        assert_eq!(
            map_key(&k(KeyCode::Enter, KeyModifiers::ALT), true),
            Action::Newline
        );
        assert_eq!(
            map_key(&k(KeyCode::Char('j'), KeyModifiers::CONTROL), true),
            Action::Newline
        );
    }

    #[test]
    fn home_end_depend_on_input() {
        assert_eq!(
            map_key(&k(KeyCode::Home, KeyModifiers::NONE), true),
            Action::ScrollTop
        );
        assert_eq!(
            map_key(&k(KeyCode::Home, KeyModifiers::NONE), false),
            Action::Edit(EditAction::Home)
        );
        assert_eq!(
            map_key(&k(KeyCode::End, KeyModifiers::CONTROL), false),
            Action::ScrollBottom
        );
    }

    #[test]
    fn control_keys() {
        assert_eq!(
            map_key(&k(KeyCode::Char('c'), KeyModifiers::CONTROL), false),
            Action::CtrlC
        );
        assert_eq!(
            map_key(&k(KeyCode::Char('o'), KeyModifiers::CONTROL), false),
            Action::ToggleReasoning
        );
        assert_eq!(
            map_key(&k(KeyCode::Char('t'), KeyModifiers::CONTROL), false),
            Action::ToggleToolOutput
        );
        assert_eq!(
            map_key(&k(KeyCode::Char('r'), KeyModifiers::CONTROL), false),
            Action::HistorySearch
        );
        assert_eq!(
            map_key(&k(KeyCode::Char('y'), KeyModifiers::CONTROL), false),
            Action::Copy
        );
        assert_eq!(
            map_key(&k(KeyCode::Char('x'), KeyModifiers::CONTROL), false),
            Action::ChordPrefix
        );
        // Ctrl-S only means something inside a Ctrl-X chord (resolved by the app).
        assert_eq!(
            map_key(&k(KeyCode::Char('s'), KeyModifiers::CONTROL), false),
            Action::None
        );
        assert_eq!(
            map_key(&k(KeyCode::Char('q'), KeyModifiers::CONTROL), false),
            Action::None
        );
        assert_eq!(
            map_key(&k(KeyCode::Char('X'), KeyModifiers::SHIFT), false),
            Action::Edit(EditAction::Insert('X'))
        );
        assert_eq!(
            map_key(&k(KeyCode::Up, KeyModifiers::CONTROL), false),
            Action::HelperPrev
        );
        assert_eq!(
            map_key(&k(KeyCode::Down, KeyModifiers::CONTROL), true),
            Action::HelperNext
        );
        assert_eq!(
            map_key(&k(KeyCode::Down, KeyModifiers::NONE), true),
            Action::Edit(EditAction::Down)
        );
    }
}
