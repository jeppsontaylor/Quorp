//! Slash-command registry and fuzzy palette dispatcher.
//!
//! Expanded from `quorp_term::SlashCommand` with the full Claude-CLI /
//! Codex-CLI command set. Consumers register a `Registry`, parse user
//! input with `parse`, fuzzy-rank candidates with `suggest`.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlashCommandSpec {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub description: &'static str,
    pub takes_args: bool,
}

/// Built-in command set. Consumers may extend via `Registry::extend`.
pub const BUILTIN: &[SlashCommandSpec] = &[
    SlashCommandSpec { name: "help", aliases: &["h", "?"], description: "Show help and command list", takes_args: false },
    SlashCommandSpec { name: "clear", aliases: &[], description: "Clear scrollback and reset transcript", takes_args: false },
    SlashCommandSpec { name: "model", aliases: &[], description: "Switch active model", takes_args: true },
    SlashCommandSpec { name: "provider", aliases: &[], description: "Switch active provider", takes_args: true },
    SlashCommandSpec { name: "plan", aliases: &[], description: "Enter plan mode (read-only proposing)", takes_args: false },
    SlashCommandSpec { name: "act", aliases: &[], description: "Exit plan mode and execute", takes_args: false },
    SlashCommandSpec { name: "permissions", aliases: &["perms"], description: "Configure permission mode", takes_args: true },
    SlashCommandSpec { name: "memory", aliases: &["mem"], description: "Inspect / edit memory store", takes_args: false },
    SlashCommandSpec { name: "rules", aliases: &[], description: "Show active rule forge rules", takes_args: false },
    SlashCommandSpec { name: "session", aliases: &[], description: "Save/load/new session", takes_args: true },
    SlashCommandSpec { name: "status", aliases: &[], description: "Print full session status", takes_args: false },
    SlashCommandSpec { name: "exit", aliases: &["quit"], description: "Quit Quorp", takes_args: false },
    SlashCommandSpec { name: "init", aliases: &[], description: "Scaffold .quorp/ for the workspace", takes_args: false },
    SlashCommandSpec { name: "compact", aliases: &[], description: "Compact transcript now", takes_args: false },
    SlashCommandSpec { name: "edit", aliases: &[], description: "Open path in $EDITOR", takes_args: true },
    SlashCommandSpec { name: "undo", aliases: &[], description: "Revert last assistant write batch", takes_args: false },
    SlashCommandSpec { name: "redo", aliases: &[], description: "Redo last reverted write batch", takes_args: false },
    SlashCommandSpec { name: "files", aliases: &["f"], description: "Fuzzy file picker", takes_args: false },
    SlashCommandSpec { name: "diff", aliases: &[], description: "Show working tree or stored diff", takes_args: true },
    SlashCommandSpec { name: "test", aliases: &[], description: "Run validation tests", takes_args: false },
    SlashCommandSpec { name: "verify", aliases: &[], description: "Run configured proof lane", takes_args: false },
    SlashCommandSpec { name: "save", aliases: &[], description: "Persist current session", takes_args: false },
    SlashCommandSpec { name: "load", aliases: &[], description: "Load named session", takes_args: true },
    SlashCommandSpec { name: "auto", aliases: &[], description: "Toggle to auto-safe permissions", takes_args: false },
    SlashCommandSpec { name: "manual", aliases: &[], description: "Toggle to ask permissions", takes_args: false },
    SlashCommandSpec { name: "think", aliases: &[], description: "Single-turn plan-only override", takes_args: false },
    SlashCommandSpec { name: "doctor", aliases: &[], description: "Re-run quorp doctor inline", takes_args: false },
    SlashCommandSpec { name: "sandbox", aliases: &[], description: "Set sandbox mode (host|tmp-copy)", takes_args: true },
    SlashCommandSpec { name: "mcp", aliases: &[], description: "List MCP servers", takes_args: false },
    SlashCommandSpec { name: "hooks", aliases: &[], description: "List lifecycle hooks", takes_args: false },
    SlashCommandSpec { name: "tasks", aliases: &[], description: "Show task checklist", takes_args: false },
    SlashCommandSpec { name: "checkpoint", aliases: &[], description: "Create git checkpoint", takes_args: false },
    SlashCommandSpec { name: "rollback", aliases: &[], description: "Revert to last checkpoint", takes_args: false },
    SlashCommandSpec { name: "theme", aliases: &[], description: "Cycle CLI theme", takes_args: false },
];

#[derive(Debug, Clone)]
pub struct Registry {
    commands: Vec<SlashCommandSpec>,
}

impl Default for Registry {
    fn default() -> Self {
        Self { commands: BUILTIN.to_vec() }
    }
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn extend(&mut self, extra: impl IntoIterator<Item = SlashCommandSpec>) {
        self.commands.extend(extra);
    }

    pub fn all(&self) -> &[SlashCommandSpec] {
        &self.commands
    }

    /// Resolve a command name (or alias) to its spec.
    pub fn resolve(&self, name: &str) -> Option<&SlashCommandSpec> {
        let name = name.trim_start_matches('/');
        self.commands.iter().find(|c| {
            c.name.eq_ignore_ascii_case(name) || c.aliases.iter().any(|a| a.eq_ignore_ascii_case(name))
        })
    }

    /// Rank candidates for `prefix` using a cheap subsequence score.
    pub fn suggest(&self, prefix: &str) -> Vec<(&SlashCommandSpec, i32)> {
        let needle = prefix.trim_start_matches('/').to_ascii_lowercase();
        let mut scored: Vec<(&SlashCommandSpec, i32)> = self
            .commands
            .iter()
            .filter_map(|c| subsequence_score(&needle, c.name).map(|score| (c, score)))
            .collect();
        scored.sort_by_key(|(_, score)| -*score);
        scored
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedCommand {
    pub name: String,
    pub args: String,
}

/// Parse a slash invocation line. Returns `None` if the line does not
/// start with `/` after trimming whitespace.
pub fn parse(line: &str) -> Option<ParsedCommand> {
    let trimmed = line.trim();
    if !trimmed.starts_with('/') {
        return None;
    }
    let body = &trimmed[1..];
    let mut parts = body.splitn(2, char::is_whitespace);
    let name = parts.next()?.to_string();
    let args = parts.next().unwrap_or("").trim().to_string();
    if name.is_empty() {
        None
    } else {
        Some(ParsedCommand { name, args })
    }
}

fn subsequence_score(needle: &str, hay: &str) -> Option<i32> {
    if needle.is_empty() {
        return Some(0);
    }
    let mut hay_iter = hay.chars();
    let mut score: i32 = 0;
    let mut last_index: i32 = -1;
    let mut current_index: i32 = -1;
    for needle_char in needle.chars() {
        let mut matched = false;
        for hay_char in hay_iter.by_ref() {
            current_index += 1;
            if hay_char.eq_ignore_ascii_case(&needle_char) {
                let bonus = if last_index >= 0 && current_index == last_index + 1 { 4 } else { 1 };
                score += bonus;
                last_index = current_index;
                matched = true;
                break;
            }
        }
        if !matched {
            return None;
        }
    }
    Some(score)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_single_command() {
        let parsed = parse("/help").unwrap();
        assert_eq!(parsed.name, "help");
        assert!(parsed.args.is_empty());
    }

    #[test]
    fn parse_with_args() {
        let parsed = parse("  /load main-session  ").unwrap();
        assert_eq!(parsed.name, "load");
        assert_eq!(parsed.args, "main-session");
    }

    #[test]
    fn parse_returns_none_without_slash() {
        assert!(parse("help").is_none());
        assert!(parse("/").is_none());
    }

    #[test]
    fn registry_resolves_aliases() {
        let r = Registry::new();
        let by_alias = r.resolve("/quit").unwrap();
        assert_eq!(by_alias.name, "exit");
    }

    #[test]
    fn suggest_ranks_prefix_matches() {
        let r = Registry::new();
        let suggestions = r.suggest("/per");
        assert!(suggestions.iter().any(|(c, _)| c.name == "permissions"));
    }
}
