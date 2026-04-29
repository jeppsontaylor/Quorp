use super::*;
use crossterm::event::KeyModifiers;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

#[test]
fn slash_opens_command_suggestions() {
    let registry = Registry::new();
    let mut state = ComposerState::default();

    assert_eq!(
        state.handle_key(key(KeyCode::Char('/')), &registry),
        ComposerAction::Continue
    );

    let suggestions = state.suggestions(&registry);
    assert!(suggestions.iter().any(|entry| entry.value == "/help"));
    assert!(suggestions.len() >= 8);
}

#[test]
fn slash_prefix_filters_suggestions() {
    let registry = Registry::new();
    let mut state = ComposerState::default();

    for value in ['/', 's', 'a'] {
        state.handle_key(key(KeyCode::Char(value)), &registry);
    }

    let suggestions = state.suggestions(&registry);
    assert_eq!(
        suggestions.first().map(|entry| entry.value.as_str()),
        Some("/sandbox")
    );
}

#[test]
fn tab_completes_selected_command() {
    let registry = Registry::new();
    let mut state = ComposerState::default();

    for value in ['/', 's', 'a'] {
        state.handle_key(key(KeyCode::Char(value)), &registry);
    }
    state.handle_key(key(KeyCode::Tab), &registry);

    assert_eq!(state.buffer(), "/sandbox ");
    assert_eq!(state.cursor(), "/sandbox ".len());
}

#[test]
fn down_selects_next_suggestion() {
    let registry = Registry::new();
    let mut state = ComposerState::default();

    state.handle_key(key(KeyCode::Char('/')), &registry);
    state.handle_key(key(KeyCode::Down), &registry);

    assert_eq!(state.selected(), 1);
}

#[test]
fn argument_suggestions_complete_mode_values() {
    let registry = Registry::new();
    let mut state = ComposerState::default();

    for value in "/sandbox t".chars() {
        state.handle_key(key(KeyCode::Char(value)), &registry);
    }

    let suggestions = state.suggestions(&registry);
    assert_eq!(
        suggestions.first().map(|entry| entry.value.as_str()),
        Some("/sandbox tmp-copy")
    );
}

#[test]
fn no_color_panel_has_no_ansi_escapes() {
    let entries = vec![PaletteEntry {
        value: "/help".to_string(),
        description: "Show help".to_string(),
        detail: "alias h".to_string(),
    }];
    let rendered = render_suggestion_panel(&entries, 0, ColorCapability::NoColor).join("\n");
    assert!(rendered.contains("/help"));
    assert!(!rendered.contains("\x1b["));
}
