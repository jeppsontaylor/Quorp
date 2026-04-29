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
