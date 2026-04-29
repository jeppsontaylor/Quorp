use super::*;

// Examples
// WSL
// wsl.exe --distribution NixOS --cd /home/user -- /usr/bin/zsh -c "echo hello"
// wsl.exe --distribution NixOS --cd /home/user -- /usr/bin/zsh -c "\"echo hello\"" | grep hello"
// wsl.exe --distribution NixOS --cd ~ env RUST_LOG=info,remote=debug .quorp_wsl_server/quorp-remote-server-dev-build proxy --identifier dev-workspace-53
// PowerShell from Nushell
// nu -c overlay use "C:\Users\kubko\dev\python\39007\tests\.venv\Scripts\activate.nu"; ^"C:\Program Files\PowerShell\7\pwsh.exe" -C "C:\Users\kubko\dev\python\39007\tests\.venv\Scripts\python.exe -m pytest \"test_foo.py::test_foo\""
// PowerShell from CMD
// cmd /C \" \"C:\\\\Users\\\\kubko\\\\dev\\\\python\\\\39007\\\\tests\\\\.venv\\\\Scripts\\\\activate.bat\"& \"C:\\\\Program Files\\\\PowerShell\\\\7\\\\pwsh.exe\" -C \"C:\\\\Users\\\\kubko\\\\dev\\\\python\\\\39007\\\\tests\\\\.venv\\\\Scripts\\\\python.exe -m pytest \\\"test_foo.py::test_foo\\\"\"\"

#[test]
fn test_try_quote_powershell() {
    let shell_kind = ShellKind::PowerShell;
    assert_eq!(
        shell_kind
            .try_quote("C:\\Users\\johndoe\\dev\\python\\39007\\tests\\.venv\\Scripts\\python.exe -m pytest \"test_foo.py::test_foo\"")
            .unwrap()
            .into_owned(),
        "'C:\\Users\\johndoe\\dev\\python\\39007\\tests\\.venv\\Scripts\\python.exe -m pytest \\\"test_foo.py::test_foo\\\"'".to_string()
    );
}

#[test]
fn test_try_quote_cmd() {
    let shell_kind = ShellKind::Cmd;
    assert_eq!(
        shell_kind
            .try_quote("C:\\Users\\johndoe\\dev\\python\\39007\\tests\\.venv\\Scripts\\python.exe -m pytest \"test_foo.py::test_foo\"")
            .unwrap()
            .into_owned(),
        "^\"C:\\Users\\johndoe\\dev\\python\\39007\\tests\\.venv\\Scripts\\python.exe -m pytest \\^\"test_foo.py::test_foo\\^\"^\"".to_string()
    );
}

#[test]
fn test_try_quote_powershell_edge_cases() {
    let shell_kind = ShellKind::PowerShell;

    // Empty string
    assert_eq!(
        shell_kind.try_quote("").unwrap().into_owned(),
        "'\"\"'".to_string()
    );

    // String without special characters (no quoting needed)
    assert_eq!(shell_kind.try_quote("simple").unwrap(), "simple");

    // String with spaces
    assert_eq!(
        shell_kind.try_quote("hello world").unwrap().into_owned(),
        "'hello world'".to_string()
    );

    // String with dollar signs
    assert_eq!(
        shell_kind.try_quote("$variable").unwrap().into_owned(),
        "'$variable'".to_string()
    );

    // String with backticks
    assert_eq!(
        shell_kind.try_quote("test`command").unwrap().into_owned(),
        "'test`command'".to_string()
    );

    // String with multiple special characters
    assert_eq!(
        shell_kind
            .try_quote("test `\"$var`\" end")
            .unwrap()
            .into_owned(),
        "'test `\\\"$var`\\\" end'".to_string()
    );

    // String with backslashes and colon (path without spaces doesn't need quoting)
    assert_eq!(
        shell_kind.try_quote("C:\\path\\to\\file").unwrap(),
        "C:\\path\\to\\file"
    );
}

#[test]
fn test_try_quote_cmd_edge_cases() {
    let shell_kind = ShellKind::Cmd;

    // Empty string
    assert_eq!(
        shell_kind.try_quote("").unwrap().into_owned(),
        "^\"^\"".to_string()
    );

    // String without special characters (no quoting needed)
    assert_eq!(shell_kind.try_quote("simple").unwrap(), "simple");

    // String with spaces
    assert_eq!(
        shell_kind.try_quote("hello world").unwrap().into_owned(),
        "^\"hello world^\"".to_string()
    );

    // String with space and backslash (backslash not at end, so not doubled)
    assert_eq!(
        shell_kind.try_quote("path\\ test").unwrap().into_owned(),
        "^\"path\\ test^\"".to_string()
    );

    // String ending with backslash (must be doubled before closing quote)
    assert_eq!(
        shell_kind.try_quote("test path\\").unwrap().into_owned(),
        "^\"test path\\\\^\"".to_string()
    );

    // String ending with multiple backslashes (all doubled before closing quote)
    assert_eq!(
        shell_kind.try_quote("test path\\\\").unwrap().into_owned(),
        "^\"test path\\\\\\\\^\"".to_string()
    );

    // String with embedded quote (quote is escaped, backslash before it is doubled)
    assert_eq!(
        shell_kind.try_quote("test\\\"quote").unwrap().into_owned(),
        "^\"test\\\\\\^\"quote^\"".to_string()
    );

    // String with multiple backslashes before embedded quote (all doubled)
    assert_eq!(
        shell_kind
            .try_quote("test\\\\\"quote")
            .unwrap()
            .into_owned(),
        "^\"test\\\\\\\\\\^\"quote^\"".to_string()
    );

    // String with backslashes not before quotes (path without spaces doesn't need quoting)
    assert_eq!(
        shell_kind.try_quote("C:\\path\\to\\file").unwrap(),
        "C:\\path\\to\\file"
    );
}

#[test]
fn test_try_quote_nu_command() {
    let shell_kind = ShellKind::Nushell;
    assert_eq!(
        shell_kind.try_quote("'uname'").unwrap().into_owned(),
        "\"'uname'\"".to_string()
    );
    assert_eq!(
        shell_kind
            .try_quote_prefix_aware("'uname'")
            .unwrap()
            .into_owned(),
        "^\"'uname'\"".to_string()
    );
    assert_eq!(
        shell_kind.try_quote("^uname").unwrap().into_owned(),
        "'^uname'".to_string()
    );
    assert_eq!(
        shell_kind
            .try_quote_prefix_aware("^uname")
            .unwrap()
            .into_owned(),
        "^uname".to_string()
    );
    assert_eq!(
        shell_kind.try_quote("^'uname'").unwrap().into_owned(),
        "'^'\"'uname\'\"".to_string()
    );
    assert_eq!(
        shell_kind
            .try_quote_prefix_aware("^'uname'")
            .unwrap()
            .into_owned(),
        "^'uname'".to_string()
    );
    assert_eq!(
        shell_kind.try_quote("'uname a'").unwrap().into_owned(),
        "\"'uname a'\"".to_string()
    );
    assert_eq!(
        shell_kind
            .try_quote_prefix_aware("'uname a'")
            .unwrap()
            .into_owned(),
        "^\"'uname a'\"".to_string()
    );
    assert_eq!(
        shell_kind.try_quote("^'uname a'").unwrap().into_owned(),
        "'^'\"'uname a'\"".to_string()
    );
    assert_eq!(
        shell_kind
            .try_quote_prefix_aware("^'uname a'")
            .unwrap()
            .into_owned(),
        "^'uname a'".to_string()
    );
    assert_eq!(
        shell_kind.try_quote("uname").unwrap().into_owned(),
        "uname".to_string()
    );
    assert_eq!(
        shell_kind
            .try_quote_prefix_aware("uname")
            .unwrap()
            .into_owned(),
        "uname".to_string()
    );
}

#[test]
fn test_try_quote_single_quote_paths() {
    let path_with_quote = r"C:\Temp\O'Brien\repo";
    let shlex_shells = [
        ShellKind::Posix,
        ShellKind::Fish,
        ShellKind::Csh,
        ShellKind::Tcsh,
        ShellKind::Rc,
        ShellKind::Xonsh,
        ShellKind::Elvish,
        ShellKind::Nushell,
    ];

    for shell_kind in shlex_shells {
        let quoted = shell_kind.try_quote(path_with_quote).unwrap().into_owned();
        assert_ne!(quoted, path_with_quote);
        assert_eq!(
            shlex::split(&quoted),
            Some(vec![path_with_quote.to_string()])
        );

        if shell_kind == ShellKind::Nushell {
            let prefixed = shell_kind.prepend_command_prefix(&quoted);
            assert!(prefixed.starts_with('^'));
        }
    }

    for shell_kind in [ShellKind::PowerShell, ShellKind::Pwsh] {
        let quoted = shell_kind.try_quote(path_with_quote).unwrap().into_owned();
        assert!(quoted.starts_with('\''));
        assert!(quoted.ends_with('\''));
        assert!(quoted.contains("O''Brien"));
    }
}
