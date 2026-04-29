use super::*;

#[test]
fn global_level() {
    let input = "info";
    let filter = parse(input).unwrap();

    assert_eq!(filter.level_global.unwrap(), log::LevelFilter::Info);
    assert!(filter.directive_names.is_empty());
    assert!(filter.directive_levels.is_empty());
}

#[test]
fn directive_level() {
    let input = "my_module=debug";
    let filter = parse(input).unwrap();

    assert_eq!(filter.level_global, None);
    assert_eq!(filter.directive_names, vec!["my_module".to_string()]);
    assert_eq!(filter.directive_levels, vec![log::LevelFilter::Debug]);
}

#[test]
fn global_level_and_directive_level() {
    let input = "info,my_module=debug";
    let filter = parse(input).unwrap();

    assert_eq!(filter.level_global.unwrap(), log::LevelFilter::Info);
    assert_eq!(filter.directive_names, vec!["my_module".to_string()]);
    assert_eq!(filter.directive_levels, vec![log::LevelFilter::Debug]);
}

#[test]
fn global_level_and_bare_module() {
    let input = "info,my_module";
    let filter = parse(input).unwrap();

    assert_eq!(filter.level_global.unwrap(), log::LevelFilter::Info);
    assert_eq!(filter.directive_names, vec!["my_module".to_string()]);
    assert_eq!(filter.directive_levels, vec![log::LevelFilter::max()]);
}

#[test]
fn err_when_multiple_max_levels() {
    let input = "info,warn";
    let result = parse(input);

    assert!(result.is_err());
}

#[test]
fn err_when_invalid_level() {
    let input = "my_module=foobar";
    let result = parse(input);

    assert!(result.is_err());
}
