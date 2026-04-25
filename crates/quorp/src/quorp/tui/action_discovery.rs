use crate::quorp::tui::engage_target::EngageTargetKind;
use crate::quorp::tui::models_pane::ModelsPaneEntry;
use crate::quorp::tui::proof_rail::RailMode;
use crate::quorp::tui::shell::ShellOverlay;
use crate::quorp::tui::slash_commands::CommandDeckEntry;
use crate::quorp::tui::workbench::LeafId;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::borrow::Cow;
use std::sync::OnceLock;

#[derive(Clone, Copy)]
struct ActionDiscoveryEntry {
    key: &'static str,
    context: &'static str,
    action: &'static str,
    shell_overlay_line: Option<&'static str>,
    show_in_popup: bool,
}

#[derive(Clone, Copy)]
struct FocusContextHint {
    pane: LeafId,
    compact_ui: Option<bool>,
    text: &'static str,
}

const ACTION_CONTEXT_HINTS: &[FocusContextHint] = &[
    FocusContextHint {
        pane: LeafId(0),
        compact_ui: None,
        text: "Enter open  Tab next focus  Alt+2 preview  Ctrl+b hide files  ? help",
    },
    FocusContextHint {
        pane: LeafId(1),
        compact_ui: None,
        text: "Tab next focus  Alt+1 files  Alt+3 assistant  Ctrl+` terminal  Space attach",
    },
    FocusContextHint {
        pane: LeafId(3),
        compact_ui: Some(false),
        text: "Enter send  PgUp/PgDn results  Alt+a/p/x mode  Ctrl+t new chat  Ctrl+m models",
    },
    FocusContextHint {
        pane: LeafId(3),
        compact_ui: Some(true),
        text: "Enter send  PgUp/PgDn results  Alt+a/p/x mode  Ctrl+t new chat  Esc close overlay",
    },
    FocusContextHint {
        pane: LeafId(4),
        compact_ui: None,
        text: "Assistant actions are shown inline in the assistant rail.",
    },
    FocusContextHint {
        pane: LeafId(2),
        compact_ui: None,
        text: "Terminal",
    },
];

const ACTION_DISCOVERY: &[ActionDiscoveryEntry] = &[
    ActionDiscoveryEntry {
        key: "Click",
        context: "Global",
        action: "Focus pane",
        shell_overlay_line: None,
        show_in_popup: true,
    },
    ActionDiscoveryEntry {
        key: "Tab / Shift+Tab",
        context: "Global",
        action: "Cycle pane focus",
        shell_overlay_line: Some("Tab / Shift+Tab      cycle focus islands"),
        show_in_popup: true,
    },
    ActionDiscoveryEntry {
        key: "Alt+1..4",
        context: "Global",
        action: "Jump workspace panes",
        shell_overlay_line: Some("Alt+1..4             jump Explorer / Main / Assistant / Dock"),
        show_in_popup: false,
    },
    ActionDiscoveryEntry {
        key: "Ctrl+h / j / k / l",
        context: "Global",
        action: "Vim-style pane navigation",
        shell_overlay_line: Some("Ctrl+h / j / k / l   Vim-style focus moves"),
        show_in_popup: true,
    },
    ActionDiscoveryEntry {
        key: "Ctrl+b",
        context: "Global",
        action: "Toggle files",
        shell_overlay_line: Some("Ctrl+b               toggle explorer"),
        show_in_popup: false,
    },
    ActionDiscoveryEntry {
        key: "Ctrl+`",
        context: "Global",
        action: "Toggle terminal dock",
        shell_overlay_line: Some("Ctrl+`               toggle terminal dock"),
        show_in_popup: false,
    },
    ActionDiscoveryEntry {
        key: "Ctrl+p",
        context: "Global",
        action: "Quick open",
        shell_overlay_line: Some("Ctrl+p               quick open"),
        show_in_popup: false,
    },
    ActionDiscoveryEntry {
        key: "Ctrl+k",
        context: "Global",
        action: "Open control deck",
        shell_overlay_line: Some("Ctrl+k               control deck"),
        show_in_popup: true,
    },
    ActionDiscoveryEntry {
        key: "Ctrl+n",
        context: "Global",
        action: "New thread",
        shell_overlay_line: Some("Ctrl+n               choose root for a new thread"),
        show_in_popup: true,
    },
    ActionDiscoveryEntry {
        key: "Ctrl+t",
        context: "Chat",
        action: "New chat session",
        shell_overlay_line: Some("Ctrl+t               open a new assistant session"),
        show_in_popup: true,
    },
    ActionDiscoveryEntry {
        key: "Alt+a / Alt+p / Alt+x",
        context: "Chat",
        action: "Switch Ask / Plan / Act mode",
        shell_overlay_line: Some("Alt+a / Alt+p / Alt+x switch Ask / Plan / Act"),
        show_in_popup: true,
    },
    ActionDiscoveryEntry {
        key: "Space",
        context: "Global",
        action: "Attach current file",
        shell_overlay_line: Some("Space                attach current file to chat"),
        show_in_popup: false,
    },
    ActionDiscoveryEntry {
        key: "Ctrl+g",
        context: "Terminal",
        action: "Enter terminal navigation mode",
        shell_overlay_line: Some("Ctrl+g               switch terminal to navigation mode"),
        show_in_popup: false,
    },
    ActionDiscoveryEntry {
        key: "Enter",
        context: "Terminal",
        action: "Re-enter terminal capture",
        shell_overlay_line: Some("Enter                re-enter terminal capture"),
        show_in_popup: false,
    },
    ActionDiscoveryEntry {
        key: "Ctrl+m",
        context: "Global",
        action: "Toggle Models pane",
        shell_overlay_line: None,
        show_in_popup: true,
    },
    ActionDiscoveryEntry {
        key: "Esc",
        context: "Global",
        action: "Quit (or dismiss help)",
        shell_overlay_line: None,
        show_in_popup: true,
    },
    ActionDiscoveryEntry {
        key: "Up / Down",
        context: "File Tree",
        action: "Navigate entries",
        shell_overlay_line: None,
        show_in_popup: true,
    },
    ActionDiscoveryEntry {
        key: "Home / End",
        context: "File Tree",
        action: "Jump to first / last",
        shell_overlay_line: None,
        show_in_popup: true,
    },
    ActionDiscoveryEntry {
        key: "Enter",
        context: "File Tree",
        action: "Expand dir / Select file",
        shell_overlay_line: None,
        show_in_popup: true,
    },
    ActionDiscoveryEntry {
        key: "Left / Right",
        context: "File Tree",
        action: "Collapse (or parent) / Expand",
        shell_overlay_line: None,
        show_in_popup: true,
    },
    ActionDiscoveryEntry {
        key: "Up/Down/PgUp/PgDn/Home/End",
        context: "Code Preview",
        action: "Scroll",
        shell_overlay_line: None,
        show_in_popup: true,
    },
    ActionDiscoveryEntry {
        key: "Alt+Up",
        context: "Code / Chat",
        action: "Focus tab strip (arrows switch tabs)",
        shell_overlay_line: None,
        show_in_popup: true,
    },
    ActionDiscoveryEntry {
        key: "Left/Right",
        context: "Tab strip focused",
        action: "Previous / next tab",
        shell_overlay_line: None,
        show_in_popup: true,
    },
    ActionDiscoveryEntry {
        key: "Delete / Ctrl+w",
        context: "Tab strip focused",
        action: "Close active tab",
        shell_overlay_line: None,
        show_in_popup: true,
    },
    ActionDiscoveryEntry {
        key: "Ctrl+Shift+w",
        context: "Tab strip focused",
        action: "Close all tabs",
        shell_overlay_line: None,
        show_in_popup: true,
    },
    ActionDiscoveryEntry {
        key: "PgUp / PgDn",
        context: "Assistant",
        action: "Scroll results",
        shell_overlay_line: None,
        show_in_popup: true,
    },
    ActionDiscoveryEntry {
        key: "Shift+PgUp/PgDn",
        context: "Terminal",
        action: "Scrollback offset",
        shell_overlay_line: None,
        show_in_popup: true,
    },
    ActionDiscoveryEntry {
        key: "[ / ]",
        context: "Chat",
        action: "Cycle model",
        shell_overlay_line: None,
        show_in_popup: true,
    },
    ActionDiscoveryEntry {
        key: "Enter",
        context: "Chat",
        action: "Send message",
        shell_overlay_line: None,
        show_in_popup: true,
    },
    ActionDiscoveryEntry {
        key: "/",
        context: "Chat",
        action: "Open workflow deck when composer is empty",
        shell_overlay_line: Some("/                    workflow deck (empty composer)"),
        show_in_popup: true,
    },
    ActionDiscoveryEntry {
        key: "Enter",
        context: "Models",
        action: "Switch model / Download",
        shell_overlay_line: None,
        show_in_popup: true,
    },
    ActionDiscoveryEntry {
        key: "Ctrl+s",
        context: "Global",
        action: "Stop running model",
        shell_overlay_line: None,
        show_in_popup: true,
    },
    ActionDiscoveryEntry {
        key: "?",
        context: "Global",
        action: "Toggle this help",
        shell_overlay_line: None,
        show_in_popup: true,
    },
];

const ACTION_DISCOVERY_OVERLAY_TITLE: &str = "Keybindings";
const QUICK_OPEN_OVERLAY_TITLE: &str = "Quick Open";
const NEW_THREAD_OVERLAY_TITLE: &str = "New Thread";
const MODEL_PICKER_OVERLAY_TITLE: &str = "Model Selector";
const SLASH_COMMAND_OVERLAY_TITLE: &str = "Slash Commands";
const ACTION_DECK_OVERLAY_TITLE: &str = "Control Deck";

#[derive(Clone, Copy, Debug)]
pub enum ActionDeckCommand {
    SetRailMode(Option<RailMode>),
    AddWatchpoint(&'static str),
    InsertSlash(&'static str),
    OpenFirstTarget(EngageTargetKind),
    OpenDiffTarget,
}

#[derive(Clone, Copy, Debug)]
pub struct ActionDeckEntry {
    pub label: &'static str,
    pub detail: &'static str,
    pub command: ActionDeckCommand,
}

const ACTION_DECK_ENTRIES: &[ActionDeckEntry] = &[
    ActionDeckEntry {
        label: "Command Center",
        detail: "Return to the default control tower rail.",
        command: ActionDeckCommand::SetRailMode(None),
    },
    ActionDeckEntry {
        label: "Diff Lens",
        detail: "Focus blast radius, semantic patch surface, and rollback.",
        command: ActionDeckCommand::SetRailMode(Some(RailMode::DiffReactor)),
    },
    ActionDeckEntry {
        label: "Verify Radar",
        detail: "Foreground proof progress, tests, and time-to-proof.",
        command: ActionDeckCommand::SetRailMode(Some(RailMode::VerifyRadar)),
    },
    ActionDeckEntry {
        label: "Trace Lens",
        detail: "Open the reasoning artifact view with evidence and rollback.",
        command: ActionDeckCommand::SetRailMode(Some(RailMode::TraceLens)),
    },
    ActionDeckEntry {
        label: "Timeline",
        detail: "Scrub checkpoints, stop reason, and exported artifacts.",
        command: ActionDeckCommand::SetRailMode(Some(RailMode::TimelineScrubber)),
    },
    ActionDeckEntry {
        label: "Memory Viewport",
        detail: "Show context pressure and compaction state.",
        command: ActionDeckCommand::SetRailMode(Some(RailMode::MemoryViewport)),
    },
    ActionDeckEntry {
        label: "Tool Orchestra",
        detail: "Follow the active tools instead of the broader command center.",
        command: ActionDeckCommand::SetRailMode(Some(RailMode::ToolOrchestra)),
    },
    ActionDeckEntry {
        label: "Watchpoint: no migrations",
        detail: "Trip if schema or migration files appear in the blast radius.",
        command: ActionDeckCommand::AddWatchpoint("no migrations"),
    },
    ActionDeckEntry {
        label: "Watchpoint: no public API widening",
        detail: "Trip if public surface grows while the agent is editing.",
        command: ActionDeckCommand::AddWatchpoint("no public API widening"),
    },
    ActionDeckEntry {
        label: "Watchpoint: auth untouched",
        detail: "Trip if auth paths or auth-adjacent files are touched.",
        command: ActionDeckCommand::AddWatchpoint("auth untouched"),
    },
    ActionDeckEntry {
        label: "Watchpoint: one-hop write radius",
        detail: "Trip if the change escapes the first intended file cluster.",
        command: ActionDeckCommand::AddWatchpoint("one-hop write radius"),
    },
    ActionDeckEntry {
        label: "Open Run Artifacts",
        detail: "Execute /open-run-artifacts in the active assistant session.",
        command: ActionDeckCommand::InsertSlash("/open-run-artifacts"),
    },
    ActionDeckEntry {
        label: "Resume Last Run",
        detail: "Execute /resume-last in the active assistant session.",
        command: ActionDeckCommand::InsertSlash("/resume-last"),
    },
    ActionDeckEntry {
        label: "Open Touched File",
        detail: "Preview the first changed file in the center surface.",
        command: ActionDeckCommand::OpenFirstTarget(EngageTargetKind::ChangedFile),
    },
    ActionDeckEntry {
        label: "Open Diff Target",
        detail: "Promote the current preview target into Diff Lens when edits exist.",
        command: ActionDeckCommand::OpenDiffTarget,
    },
    ActionDeckEntry {
        label: "Open Artifact",
        detail: "Preview the newest artifact path inside Quorp first.",
        command: ActionDeckCommand::OpenFirstTarget(EngageTargetKind::Artifact),
    },
    ActionDeckEntry {
        label: "Follow Tool",
        detail: "Jump to the most relevant active tool target in the center surface.",
        command: ActionDeckCommand::OpenFirstTarget(EngageTargetKind::ToolTarget),
    },
];

#[derive(Clone, Copy)]
struct ActionDiscoveryCatalog {
    shell_overlay_lines: &'static [&'static str],
}

static ACTION_DISCOVERY_CATALOG: OnceLock<ActionDiscoveryCatalog> = OnceLock::new();
static ACTION_DISCOVERY_OVERLAY: OnceLock<ShellOverlay> = OnceLock::new();

pub struct ActionDiscoveryModel;

#[derive(Clone, Copy)]
pub enum OverlayTextInput {
    Close,
    MoveUp,
    MoveDown,
    Confirm,
    Backspace,
    InsertChar(char),
    Ignore,
}

impl ActionDiscoveryModel {
    #[inline]
    pub const fn overlay_title() -> &'static str {
        ACTION_DISCOVERY_OVERLAY_TITLE
    }

    #[inline]
    pub const fn quick_open_title() -> &'static str {
        QUICK_OPEN_OVERLAY_TITLE
    }

    #[inline]
    pub const fn new_thread_title() -> &'static str {
        NEW_THREAD_OVERLAY_TITLE
    }

    #[inline]
    pub const fn model_picker_title() -> &'static str {
        MODEL_PICKER_OVERLAY_TITLE
    }

    #[inline]
    pub const fn slash_command_title() -> &'static str {
        SLASH_COMMAND_OVERLAY_TITLE
    }

    #[inline]
    pub const fn action_deck_title() -> &'static str {
        ACTION_DECK_OVERLAY_TITLE
    }

    #[inline]
    pub fn parse_text_overlay_input(key: &KeyEvent) -> OverlayTextInput {
        if key.modifiers.contains(KeyModifiers::ALT) || key.modifiers.contains(KeyModifiers::SHIFT)
        {
            return OverlayTextInput::Ignore;
        }
        match key.code {
            KeyCode::Esc => OverlayTextInput::Close,
            KeyCode::Up => OverlayTextInput::MoveUp,
            KeyCode::Down => OverlayTextInput::MoveDown,
            KeyCode::Enter => OverlayTextInput::Confirm,
            KeyCode::Backspace => OverlayTextInput::Backspace,
            KeyCode::Char(character) if key.modifiers.is_empty() => {
                OverlayTextInput::InsertChar(character)
            }
            _ => OverlayTextInput::Ignore,
        }
    }

    #[inline]
    pub fn context_hint(pane: LeafId, compact_ui: bool) -> &'static str {
        ACTION_CONTEXT_HINTS
            .iter()
            .find(|hint| {
                hint.pane == pane
                    && hint
                        .compact_ui
                        .is_none_or(|required_compact| required_compact == compact_ui)
            })
            .map(|hint| hint.text)
            .unwrap_or("Tab next focus  Alt+1..4 jump  Ctrl+b files  Ctrl+` terminal  ? help")
    }

    #[inline]
    pub fn shell_overlay_lines() -> &'static [&'static str] {
        Self::catalog().shell_overlay_lines
    }

    #[inline]
    pub fn help_overlay_snapshot() -> &'static ShellOverlay {
        ACTION_DISCOVERY_OVERLAY.get_or_init(|| {
            let lines = Self::shell_overlay_lines()
                .iter()
                .copied()
                .map(Cow::Borrowed)
                .collect();
            ShellOverlay {
                title: Self::overlay_title().to_string(),
                lines,
            }
        })
    }

    #[inline]
    pub fn model_picker_overlay_snapshot(
        entries: &[ModelsPaneEntry],
        selected_index: usize,
    ) -> ShellOverlay {
        let mut lines = Vec::with_capacity(entries.len());
        for (index, entry) in entries.iter().enumerate() {
            let marker = if index == selected_index { "> " } else { "  " };
            lines.push(Cow::Owned(format!(
                "{marker}{}  {}",
                entry.title, entry.subtitle
            )));
        }
        ShellOverlay {
            title: Self::model_picker_title().to_string(),
            lines,
        }
    }

    #[inline]
    pub fn help_hint(overlay_active: bool) -> &'static str {
        if overlay_active {
            "Press ? or Esc to close help"
        } else {
            "Press ? for help"
        }
    }

    fn catalog() -> &'static ActionDiscoveryCatalog {
        ACTION_DISCOVERY_CATALOG.get_or_init(|| {
            fn into_static_slice<T>(items: Vec<T>) -> &'static [T] {
                Box::leak(items.into_boxed_slice())
            }

            let shell_overlay_lines = ACTION_DISCOVERY
                .iter()
                .filter_map(|entry| {
                    let _ = entry.key.len();
                    let _ = entry.context.len();
                    let _ = entry.action.len();
                    entry
                        .show_in_popup
                        .then_some(())
                        .and(entry.shell_overlay_line)
                })
                .collect::<Vec<_>>();

            ActionDiscoveryCatalog {
                shell_overlay_lines: into_static_slice(shell_overlay_lines),
            }
        })
    }

    pub fn quick_open_overlay_lines(
        query: &str,
        matches: &[(String, std::path::PathBuf)],
        selected_index: usize,
    ) -> Vec<Cow<'static, str>> {
        let mut lines = vec![Cow::Owned(format!("Search: {query}"))];
        if matches.is_empty() {
            lines.push(Cow::Borrowed("No files match the current query."));
        } else {
            lines.extend(matches.iter().enumerate().map(|(index, (label, _))| {
                Cow::Owned(format!(
                    "{} {}",
                    if index == selected_index { ">" } else { " " },
                    label
                ))
            }));
        }
        lines
    }

    #[inline]
    pub fn quick_open_overlay_snapshot(
        query: &str,
        matches: &[(String, std::path::PathBuf)],
        selected_index: usize,
    ) -> ShellOverlay {
        ShellOverlay {
            title: Self::quick_open_title().to_string(),
            lines: Self::quick_open_overlay_lines(query, matches, selected_index),
        }
    }

    pub fn slash_command_overlay_lines(
        query: &str,
        matches: &[CommandDeckEntry],
        selected_index: usize,
    ) -> Vec<Cow<'static, str>> {
        let mut lines = vec![Cow::Borrowed(
            "Type to filter. Enter inserts the command. Esc closes the deck.",
        )];
        lines.push(Cow::Owned(format!("Query: /{query}")));
        if matches.is_empty() {
            lines.push(Cow::Borrowed("No slash commands match the current query."));
            return lines;
        }
        lines.extend(matches.iter().enumerate().map(|(index, entry)| {
            let marker = if index == selected_index { "▸" } else { " " };
            Cow::Owned(format!(
                "{marker} {}  {}  ·  {}  ·  {}",
                entry.label, entry.safety_mode, entry.target_scope, entry.expected_outcome
            ))
        }));
        lines
    }

    #[inline]
    pub fn slash_command_overlay_snapshot(
        query: &str,
        matches: &[CommandDeckEntry],
        selected_index: usize,
    ) -> ShellOverlay {
        ShellOverlay {
            title: Self::slash_command_title().to_string(),
            lines: Self::slash_command_overlay_lines(query, matches, selected_index),
        }
    }

    pub fn action_deck_overlay_lines(
        query: &str,
        matches: &[ActionDeckEntry],
        selected_index: usize,
    ) -> Vec<Cow<'static, str>> {
        let mut lines = vec![Cow::Borrowed(
            "Type to filter. Enter executes the action. Esc closes the deck.",
        )];
        lines.push(Cow::Owned(format!("Query: {query}")));
        if matches.is_empty() {
            lines.push(Cow::Borrowed(
                "No control deck actions match the current query.",
            ));
            return lines;
        }
        lines.extend(matches.iter().enumerate().map(|(index, entry)| {
            let marker = if index == selected_index { "▸" } else { " " };
            Cow::Owned(format!("{marker} {}  {}", entry.label, entry.detail))
        }));
        lines
    }

    #[inline]
    pub fn action_deck_overlay_snapshot(
        query: &str,
        matches: &[ActionDeckEntry],
        selected_index: usize,
    ) -> ShellOverlay {
        ShellOverlay {
            title: Self::action_deck_title().to_string(),
            lines: Self::action_deck_overlay_lines(query, matches, selected_index),
        }
    }

    pub fn new_thread_overlay_lines(
        query: &str,
        matches: &[(String, std::path::PathBuf)],
        selected_index: usize,
    ) -> Vec<Cow<'static, str>> {
        let mut lines = vec![Cow::Borrowed(
            "Type folder name and use ↑/↓ to select. Enter to open. Esc to cancel.",
        )];

        lines.push(Cow::Owned(format!("Query: {query}")));

        lines.extend(matches.iter().enumerate().map(|(index, (label, _))| {
            let marker = if index == selected_index {
                "▸ "
            } else {
                "  "
            };
            Cow::Owned(format!("{marker}{}", label))
        }));
        lines
    }

    #[inline]
    pub fn new_thread_overlay_snapshot(
        query: &str,
        matches: &[(String, std::path::PathBuf)],
        selected_index: usize,
    ) -> ShellOverlay {
        ShellOverlay {
            title: Self::new_thread_title().to_string(),
            lines: Self::new_thread_overlay_lines(query, matches, selected_index),
        }
    }
}

pub fn filter_action_deck_entries(query: &str) -> Vec<ActionDeckEntry> {
    let normalized = query.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return ACTION_DECK_ENTRIES.to_vec();
    }
    ACTION_DECK_ENTRIES
        .iter()
        .copied()
        .filter(|entry| {
            entry.label.to_ascii_lowercase().contains(&normalized)
                || entry.detail.to_ascii_lowercase().contains(&normalized)
        })
        .collect()
}
