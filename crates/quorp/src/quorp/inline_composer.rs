use anyhow::Context as _;
use crossterm::cursor::{MoveToColumn, MoveUp};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{self, Clear, ClearType};
use quorp_render::caps::ColorCapability;
use quorp_render::palette::{
    ACCENT_CYAN, ACCENT_VIOLET, ACCENT_YELLOW, BOLD, DIM, FG_TEXT, RESET, Rgb,
};
use quorp_slash::{Registry, SlashCommandSpec};
use std::io::{self, Write};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const MAX_VISIBLE_SUGGESTIONS: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaletteEntry {
    pub value: String,
    pub description: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComposerAction {
    Continue,
    Submit(String),
    Cancel,
}

#[derive(Debug, Clone)]
pub struct ComposerState {
    buffer: String,
    cursor: usize,
    selected: usize,
    suggestions_visible: bool,
}

impl Default for ComposerState {
    fn default() -> Self {
        Self {
            buffer: String::new(),
            cursor: 0,
            selected: 0,
            suggestions_visible: true,
        }
    }
}

impl ComposerState {
    pub(crate) fn with_buffer(value: &str) -> Self {
        Self {
            buffer: value.to_string(),
            cursor: value.len(),
            selected: 0,
            suggestions_visible: true,
        }
    }

    pub(crate) fn clear(&mut self) {
        self.buffer.clear();
        self.cursor = 0;
        self.selected = 0;
        self.suggestions_visible = false;
    }

    pub(crate) fn buffer(&self) -> &str {
        &self.buffer
    }

    pub(crate) fn cursor(&self) -> usize {
        self.cursor
    }

    pub(crate) fn selected(&self) -> usize {
        self.selected
    }

    pub(crate) fn suggestions_visible(&self) -> bool {
        self.suggestions_visible
    }

    pub(crate) fn set_suggestions_visible(&mut self, visible: bool) {
        self.suggestions_visible = visible;
    }

    pub fn suggestions(&self, registry: &Registry) -> Vec<PaletteEntry> {
        suggestions_for_buffer(&self.buffer, registry)
    }

    pub fn handle_key(&mut self, key: KeyEvent, registry: &Registry) -> ComposerAction {
        match (key.code, key.modifiers) {
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => return ComposerAction::Cancel,
            (KeyCode::Char('a'), KeyModifiers::CONTROL) => self.cursor = 0,
            (KeyCode::Char('e'), KeyModifiers::CONTROL) => self.cursor = self.buffer.len(),
            (KeyCode::Enter, _) => return ComposerAction::Submit(self.buffer.trim().to_string()),
            (KeyCode::Esc, _) => {
                self.suggestions_visible = false;
                self.selected = 0;
            }
            (KeyCode::Tab, _) => self.complete_selected(registry),
            (KeyCode::Backspace, _) => self.backspace(),
            (KeyCode::Delete, _) => self.delete(),
            (KeyCode::Left, _) => self.move_left(),
            (KeyCode::Right, _) => self.move_right(),
            (KeyCode::Home, _) => self.cursor = 0,
            (KeyCode::End, _) => self.cursor = self.buffer.len(),
            (KeyCode::Up, _) => self.select_previous(registry),
            (KeyCode::Down, _) => self.select_next(registry),
            (KeyCode::Char(value), _) => self.insert_char(value),
            _ => {}
        }
        self.clamp_selected(registry);
        ComposerAction::Continue
    }

    fn insert_char(&mut self, value: char) {
        self.buffer.insert(self.cursor, value);
        self.cursor += value.len_utf8();
        self.suggestions_visible = true;
    }

    fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        if let Some((index, _)) = self.buffer[..self.cursor].char_indices().next_back() {
            self.buffer.drain(index..self.cursor);
            self.cursor = index;
        }
    }

    fn delete(&mut self) {
        if self.cursor >= self.buffer.len() {
            return;
        }
        if let Some(value) = self.buffer[self.cursor..].chars().next() {
            let end = self.cursor + value.len_utf8();
            self.buffer.drain(self.cursor..end);
        }
    }

    fn move_left(&mut self) {
        if self.cursor == 0 {
            return;
        }
        if let Some((index, _)) = self.buffer[..self.cursor].char_indices().next_back() {
            self.cursor = index;
        }
    }

    fn move_right(&mut self) {
        if self.cursor >= self.buffer.len() {
            return;
        }
        if let Some(value) = self.buffer[self.cursor..].chars().next() {
            self.cursor += value.len_utf8();
        }
    }

    fn select_previous(&mut self, registry: &Registry) {
        let suggestions = self.suggestions(registry);
        if suggestions.is_empty() {
            return;
        }
        self.suggestions_visible = true;
        self.selected = if self.selected == 0 {
            suggestions.len() - 1
        } else {
            self.selected - 1
        };
    }

    fn select_next(&mut self, registry: &Registry) {
        let suggestions = self.suggestions(registry);
        if suggestions.is_empty() {
            return;
        }
        self.suggestions_visible = true;
        self.selected = (self.selected + 1) % suggestions.len();
    }

    fn complete_selected(&mut self, registry: &Registry) {
        let suggestions = self.suggestions(registry);
        let Some(entry) = suggestions.get(self.selected) else {
            return;
        };
        self.buffer = entry.value.clone();
        if !self.buffer.ends_with(' ') {
            self.buffer.push(' ');
        }
        self.cursor = self.buffer.len();
        self.suggestions_visible = false;
    }

    fn clamp_selected(&mut self, registry: &Registry) {
        let suggestions = self.suggestions(registry);
        if suggestions.is_empty() {
            self.selected = 0;
        } else if self.selected >= suggestions.len() {
            self.selected = suggestions.len() - 1;
        }
    }
}

pub struct TerminalComposer {
    registry: Registry,
    history: Vec<String>,
}

impl TerminalComposer {
    pub fn new(registry: Registry) -> Self {
        Self {
            registry,
            history: Vec::new(),
        }
    }

    pub fn read_line(
        &mut self,
        prompt: &str,
        color: ColorCapability,
    ) -> anyhow::Result<Option<String>> {
        let _raw_mode = RawModeGuard::new()?;
        let mut stdout = io::stdout();
        let mut state = ComposerState::default();
        let mut previous_panel_height = 0usize;
        render_prompt_region(
            &mut stdout,
            prompt,
            &state,
            &self.registry,
            color,
            previous_panel_height,
        )?;
        previous_panel_height = visible_panel_height(&state, &self.registry);

        loop {
            let event = event::read().context("failed to read terminal input event")?;
            let Event::Key(key) = event else {
                continue;
            };
            match state.handle_key(key, &self.registry) {
                ComposerAction::Continue => {
                    render_prompt_region(
                        &mut stdout,
                        prompt,
                        &state,
                        &self.registry,
                        color,
                        previous_panel_height,
                    )?;
                    previous_panel_height = visible_panel_height(&state, &self.registry);
                }
                ComposerAction::Cancel => {
                    clear_panel(&mut stdout, previous_panel_height)?;
                    writeln!(stdout)?;
                    return Ok(None);
                }
                ComposerAction::Submit(input) => {
                    clear_panel(&mut stdout, previous_panel_height)?;
                    writeln!(stdout)?;
                    if !input.is_empty() {
                        self.history.push(input.clone());
                    }
                    return Ok(Some(input));
                }
            }
        }
    }
}

pub fn render_quorp_loader(title: &str, color: ColorCapability) -> String {
    if matches!(color, ColorCapability::NoColor) {
        return format!("{title}\nQUORP  >_  terminal agent online\n");
    }
    let logo = [
        "  ___  _   _  ___  ____  ____",
        " / _ \\| | | |/ _ \\|  _ \\|  _ \\",
        "| | | | | | | | | | |_) | |_) |",
        "| |_| | |_| | |_| |  _ <|  __/",
        " \\__\\_\\\\___/ \\___/|_| \\_\\_|",
    ];
    let colors = [
        Rgb::new(0xFF, 0x7B, 0x00),
        Rgb::new(0xFF, 0xB0, 0x36),
        Rgb::new(0xC3, 0x76, 0xFF),
        Rgb::new(0x6F, 0xE3, 0xFF),
        Rgb::new(0x39, 0xFF, 0x88),
    ];
    let mut out = String::new();
    out.push_str(BOLD);
    out.push_str(&FG_TEXT.fg());
    out.push_str(title);
    out.push_str(RESET);
    out.push('\n');
    for (index, line) in logo.iter().enumerate() {
        out.push_str(&colors[index % colors.len()].fg());
        out.push_str(line);
        out.push_str(RESET);
        out.push('\n');
    }
    out.push_str(&ACCENT_YELLOW.fg());
    out.push_str("       >_");
    out.push_str(RESET);
    out.push_str(&DIM.to_string());
    out.push_str("  terminal agent online");
    out.push_str(RESET);
    out.push('\n');
    out
}

pub fn render_suggestion_panel(
    entries: &[PaletteEntry],
    selected: usize,
    color: ColorCapability,
) -> Vec<String> {
    if entries.is_empty() {
        return Vec::new();
    }
    let plain = matches!(color, ColorCapability::NoColor);
    entries
        .iter()
        .take(MAX_VISIBLE_SUGGESTIONS)
        .enumerate()
        .map(|(index, entry)| {
            let selector = if index == selected { ">" } else { " " };
            if plain {
                return format!(
                    "  {selector} {:<18} {:<12} {}",
                    entry.value, entry.detail, entry.description
                );
            }
            let selector_color = if index == selected {
                ACCENT_YELLOW
            } else {
                ACCENT_CYAN
            };
            format!(
                "  {}{}{} {}{:<18}{} {}{:<12}{} {}{}{}",
                selector_color.fg(),
                selector,
                RESET,
                ACCENT_CYAN.fg(),
                entry.value,
                RESET,
                ACCENT_VIOLET.fg(),
                entry.detail,
                RESET,
                FG_TEXT.fg(),
                entry.description,
                RESET
            )
        })
        .collect()
}

fn render_prompt_region(
    stdout: &mut io::Stdout,
    prompt: &str,
    state: &ComposerState,
    registry: &Registry,
    color: ColorCapability,
    previous_panel_height: usize,
) -> anyhow::Result<()> {
    let entries = if state.suggestions_visible {
        state.suggestions(registry)
    } else {
        Vec::new()
    };
    let panel_lines = render_suggestion_panel(&entries, state.selected, color);
    let panel_height = panel_lines.len().max(previous_panel_height);
    execute!(stdout, MoveToColumn(0), Clear(ClearType::CurrentLine))?;
    write!(stdout, "{prompt}{}", state.buffer)?;
    for index in 0..panel_height {
        execute!(stdout, MoveToColumn(0))?;
        write!(stdout, "\r\n")?;
        execute!(stdout, Clear(ClearType::CurrentLine))?;
        if let Some(line) = panel_lines.get(index) {
            write!(stdout, "{line}")?;
        }
    }
    if panel_height > 0 {
        execute!(stdout, MoveUp(panel_height as u16))?;
    }
    let cursor_column =
        UnicodeWidthStr::width(prompt) + buffer_width_to_cursor(&state.buffer, state.cursor);
    execute!(
        stdout,
        MoveToColumn(cursor_column.min(u16::MAX as usize) as u16)
    )?;
    stdout.flush()?;
    Ok(())
}

fn clear_panel(stdout: &mut io::Stdout, previous_panel_height: usize) -> anyhow::Result<()> {
    for _ in 0..previous_panel_height {
        execute!(stdout, MoveToColumn(0))?;
        write!(stdout, "\r\n")?;
        execute!(stdout, Clear(ClearType::CurrentLine))?;
    }
    if previous_panel_height > 0 {
        execute!(stdout, MoveUp(previous_panel_height as u16))?;
    }
    stdout.flush()?;
    Ok(())
}

fn visible_panel_height(state: &ComposerState, registry: &Registry) -> usize {
    if !state.suggestions_visible {
        return 0;
    }
    state
        .suggestions(registry)
        .len()
        .min(MAX_VISIBLE_SUGGESTIONS)
}

fn suggestions_for_buffer(buffer: &str, registry: &Registry) -> Vec<PaletteEntry> {
    if !buffer.starts_with('/') {
        return Vec::new();
    }
    let without_slash = buffer.trim_start_matches('/');
    let (command_prefix, argument_prefix) = without_slash
        .split_once(char::is_whitespace)
        .map(|(command, argument)| (command, Some(argument.trim())))
        .unwrap_or((without_slash, None));
    if let Some(argument_prefix) = argument_prefix {
        return argument_suggestions(command_prefix, argument_prefix, registry);
    }
    registry
        .suggest(command_prefix)
        .into_iter()
        .take(MAX_VISIBLE_SUGGESTIONS)
        .map(|(spec, _)| palette_entry_for_spec(spec))
        .collect()
}

fn argument_suggestions(
    command_prefix: &str,
    argument_prefix: &str,
    registry: &Registry,
) -> Vec<PaletteEntry> {
    let Some(spec) = registry.resolve(command_prefix) else {
        return Vec::new();
    };
    let candidates: &[(&str, &str)] = match spec.name {
        "sandbox" => &[
            ("host", "Run in the current workspace"),
            ("tmp-copy", "Copy workspace into an isolated tmp sandbox"),
        ],
        "permissions" => &[
            ("ask", "Ask before writes and shell commands"),
            ("auto-safe", "Allow known-safe local actions"),
            ("full-auto", "Run without routine prompts"),
            ("full-permissions", "Allow broad local actions"),
        ],
        "model" => &[
            ("qwen3-coder", "NVIDIA/OpenAI-compatible coding model"),
            ("default", "Use configured provider default"),
        ],
        "provider" => &[
            ("nvidia", "NVIDIA OpenAI-compatible endpoint"),
            ("openai-compatible", "Configured OpenAI-compatible endpoint"),
        ],
        _ => &[],
    };
    candidates
        .iter()
        .filter(|(value, _)| value.starts_with(argument_prefix))
        .map(|(value, description)| PaletteEntry {
            value: format!("/{} {}", spec.name, value),
            description: (*description).to_string(),
            detail: "argument".to_string(),
        })
        .collect()
}

fn palette_entry_for_spec(spec: &SlashCommandSpec) -> PaletteEntry {
    let detail = if spec.aliases.is_empty() {
        if spec.takes_args { "args" } else { "command" }.to_string()
    } else {
        format!("alias {}", spec.aliases.join(","))
    };
    PaletteEntry {
        value: format!("/{}", spec.name),
        description: spec.description.to_string(),
        detail,
    }
}

fn buffer_width_to_cursor(buffer: &str, cursor: usize) -> usize {
    buffer[..cursor]
        .chars()
        .map(|value| value.width().unwrap_or(0))
        .sum()
}

struct RawModeGuard;

impl RawModeGuard {
    fn new() -> anyhow::Result<Self> {
        terminal::enable_raw_mode().context("failed to enable terminal raw mode")?;
        Ok(Self)
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        if let Err(error) = terminal::disable_raw_mode() {
            eprintln!("quorp: failed to restore terminal mode: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
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
}
