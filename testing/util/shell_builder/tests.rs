use super::*;

#[test]
fn test_nu_shell_variable_substitution() {
    let shell = Shell::Program("nu".to_owned());
    let shell_builder = ShellBuilder::new(&shell, false);

    let (program, args) = shell_builder.build(
        Some("echo".into()),
        &[
            "${hello}".to_string(),
            "$world".to_string(),
            "nothing".to_string(),
            "--$something".to_string(),
            "$".to_string(),
            "${test".to_string(),
        ],
    );

    assert_eq!(program, "nu");
    assert_eq!(
        args,
        vec![
            "-i",
            "-c",
            "echo '$env.hello' '$env.world' nothing '--($env.something)' '$' '${test'"
        ]
    );
}

#[test]
fn redirect_stdin_to_dev_null_precedence() {
    let shell = Shell::Program("nu".to_owned());
    let shell_builder = ShellBuilder::new(&shell, false);

    let (program, args) = shell_builder
        .redirect_stdin_to_dev_null()
        .build(Some("echo".into()), &["nothing".to_string()]);

    assert_eq!(program, "nu");
    assert_eq!(args, vec!["-i", "-c", "(echo nothing) </dev/null"]);
}

#[test]
fn redirect_stdin_to_dev_null_fish() {
    let shell = Shell::Program("fish".to_owned());
    let shell_builder = ShellBuilder::new(&shell, false);

    let (program, args) = shell_builder
        .redirect_stdin_to_dev_null()
        .build(Some("echo".into()), &["test".to_string()]);

    assert_eq!(program, "fish");
    assert_eq!(args, vec!["-i", "-c", "begin; echo test; end </dev/null"]);
}

#[test]
fn does_not_quote_sole_command_only() {
    let shell = Shell::Program("fish".to_owned());
    let shell_builder = ShellBuilder::new(&shell, false);

    let (program, args) = shell_builder.build(Some("echo".into()), &[]);

    assert_eq!(program, "fish");
    assert_eq!(args, vec!["-i", "-c", "echo"]);

    let shell = Shell::Program("fish".to_owned());
    let shell_builder = ShellBuilder::new(&shell, false);

    let (program, args) = shell_builder.build(Some("echo oo".into()), &[]);

    assert_eq!(program, "fish");
    assert_eq!(args, vec!["-i", "-c", "echo oo"]);
}
