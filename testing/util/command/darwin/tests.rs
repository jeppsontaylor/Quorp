use super::*;
use futures_lite::AsyncWriteExt;

#[test]
fn test_spawn_echo() {
    smol::block_on(async {
        let output = Command::new("/bin/echo")
            .args(["-n", "hello world"])
            .output()
            .await
            .expect("failed to run command");

        assert!(output.status.success());
        assert_eq!(output.stdout, b"hello world");
    });
}

#[test]
fn test_spawn_cat_stdin() {
    smol::block_on(async {
        let mut child = Command::new("/bin/cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("failed to spawn");

        if let Some(ref mut stdin) = child.stdin {
            stdin
                .write_all(b"hello from stdin")
                .await
                .expect("failed to write");
            stdin.close().await.expect("failed to close");
        }
        drop(child.stdin.take());

        let output = child.output().await.expect("failed to get output");
        assert!(output.status.success());
        assert_eq!(output.stdout, b"hello from stdin");
    });
}

#[test]
fn test_spawn_stderr() {
    smol::block_on(async {
        let output = Command::new("/bin/sh")
            .args(["-c", "echo error >&2"])
            .output()
            .await
            .expect("failed to run command");

        assert!(output.status.success());
        assert_eq!(output.stderr, b"error\n");
    });
}

#[test]
fn test_spawn_exit_code() {
    smol::block_on(async {
        let output = Command::new("/bin/sh")
            .args(["-c", "exit 42"])
            .output()
            .await
            .expect("failed to run command");

        assert!(!output.status.success());
        assert_eq!(output.status.code(), Some(42));
    });
}

#[test]
fn test_spawn_current_dir() {
    smol::block_on(async {
        let output = Command::new("/bin/pwd")
            .current_dir("/tmp")
            .output()
            .await
            .expect("failed to run command");

        assert!(output.status.success());
        let pwd = String::from_utf8_lossy(&output.stdout);
        assert!(pwd.trim() == "/tmp" || pwd.trim() == "/private/tmp");
    });
}

#[test]
fn test_spawn_env() {
    smol::block_on(async {
        let output = Command::new("/bin/sh")
            .args(["-c", "echo $MY_TEST_VAR"])
            .env("MY_TEST_VAR", "test_value")
            .output()
            .await
            .expect("failed to run command");

        assert!(output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "test_value");
    });
}

#[test]
fn test_spawn_status() {
    smol::block_on(async {
        let status = Command::new("/usr/bin/true")
            .status()
            .await
            .expect("failed to run command");

        assert!(status.success());

        let status = Command::new("/usr/bin/false")
            .status()
            .await
            .expect("failed to run command");

        assert!(!status.success());
    });
}

#[test]
fn test_env_remove_removes_set_env() {
    smol::block_on(async {
        let output = Command::new("/bin/sh")
            .args(["-c", "echo ${MY_VAR:-unset}"])
            .env("MY_VAR", "set_value")
            .env_remove("MY_VAR")
            .output()
            .await
            .expect("failed to run command");

        assert!(output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "unset");
    });
}

#[test]
fn test_env_remove_removes_inherited_env() {
    smol::block_on(async {
        // SAFETY: This test is single-threaded and we clean up the var at the end
        unsafe { std::env::set_var("TEST_INHERITED_VAR", "inherited_value") };

        let output = Command::new("/bin/sh")
            .args(["-c", "echo ${TEST_INHERITED_VAR:-unset}"])
            .env_remove("TEST_INHERITED_VAR")
            .output()
            .await
            .expect("failed to run command");

        assert!(output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "unset");

        // SAFETY: Cleaning up test env var
        unsafe { std::env::remove_var("TEST_INHERITED_VAR") };
    });
}

#[test]
fn test_env_after_env_remove() {
    smol::block_on(async {
        let output = Command::new("/bin/sh")
            .args(["-c", "echo ${MY_VAR:-unset}"])
            .env_remove("MY_VAR")
            .env("MY_VAR", "new_value")
            .output()
            .await
            .expect("failed to run command");

        assert!(output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "new_value");
    });
}

#[test]
fn test_env_remove_after_env_clear() {
    smol::block_on(async {
        let output = Command::new("/bin/sh")
            .args(["-c", "echo ${MY_VAR:-unset}"])
            .env_clear()
            .env("MY_VAR", "set_value")
            .env_remove("MY_VAR")
            .output()
            .await
            .expect("failed to run command");

        assert!(output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "unset");
    });
}

#[test]
fn test_stdio_null_stdin() {
    smol::block_on(async {
        let child = Command::new("/bin/cat")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .spawn()
            .expect("failed to spawn");

        let output = child.output().await.expect("failed to get output");
        assert!(output.status.success());
        assert!(
            output.stdout.is_empty(),
            "stdin from /dev/null should produce no output from cat"
        );
    });
}

#[test]
fn test_stdio_null_stdout() {
    smol::block_on(async {
        let mut child = Command::new("/bin/echo")
            .args(["hello"])
            .stdout(Stdio::null())
            .spawn()
            .expect("failed to spawn");

        assert!(
            child.stdout.is_none(),
            "stdout should be None when Stdio::null() is used"
        );

        let status = child.status().await.expect("failed to get status");
        assert!(status.success());
    });
}

#[test]
fn test_stdio_null_stderr() {
    smol::block_on(async {
        let mut child = Command::new("/bin/sh")
            .args(["-c", "echo error >&2"])
            .stderr(Stdio::null())
            .spawn()
            .expect("failed to spawn");

        assert!(
            child.stderr.is_none(),
            "stderr should be None when Stdio::null() is used"
        );

        let status = child.status().await.expect("failed to get status");
        assert!(status.success());
    });
}

#[test]
fn test_stdio_piped_stdin() {
    smol::block_on(async {
        let mut child = Command::new("/bin/cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("failed to spawn");

        assert!(
            child.stdin.is_some(),
            "stdin should be Some when Stdio::piped() is used"
        );

        if let Some(ref mut stdin) = child.stdin {
            stdin
                .write_all(b"piped input")
                .await
                .expect("failed to write");
            stdin.close().await.expect("failed to close");
        }
        drop(child.stdin.take());

        let output = child.output().await.expect("failed to get output");
        assert!(output.status.success());
        assert_eq!(output.stdout, b"piped input");
    });
}
