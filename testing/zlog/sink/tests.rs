use super::*;

#[test]
fn test_open_or_create_log_file_rotate() {
    let temp_dir = tempfile::tempdir().unwrap();
    let log_file_path = temp_dir.path().join("log.txt");
    let rotation_log_file_path = temp_dir.path().join("log_rotated.txt");

    let contents = String::from("Hello, world!");
    std::fs::write(&log_file_path, &contents).unwrap();

    open_or_create_log_file(&log_file_path, Some(&rotation_log_file_path), 4).unwrap();

    assert!(log_file_path.exists());
    assert_eq!(log_file_path.metadata().unwrap().len(), 0);
    assert!(rotation_log_file_path.exists());
    assert_eq!(std::fs::read_to_string(&log_file_path).unwrap(), "");
}

#[test]
fn test_open_or_create_log_file() {
    let temp_dir = tempfile::tempdir().unwrap();
    let log_file_path = temp_dir.path().join("log.txt");
    let rotation_log_file_path = temp_dir.path().join("log_rotated.txt");

    let contents = String::from("Hello, world!");
    std::fs::write(&log_file_path, &contents).unwrap();

    open_or_create_log_file(&log_file_path, Some(&rotation_log_file_path), !0).unwrap();

    assert!(log_file_path.exists());
    assert_eq!(log_file_path.metadata().unwrap().len(), 13);
    assert!(!rotation_log_file_path.exists());
    assert_eq!(std::fs::read_to_string(&log_file_path).unwrap(), contents);
}

/// Regression test, ensuring that if log level values change we are made aware
#[test]
fn test_log_level_names() {
    assert_eq!(LEVEL_OUTPUT_STRINGS[log::Level::Error as usize], "ERROR");
    assert_eq!(LEVEL_OUTPUT_STRINGS[log::Level::Warn as usize], "WARN ");
    assert_eq!(LEVEL_OUTPUT_STRINGS[log::Level::Info as usize], "INFO ");
    assert_eq!(LEVEL_OUTPUT_STRINGS[log::Level::Debug as usize], "DEBUG");
    assert_eq!(LEVEL_OUTPUT_STRINGS[log::Level::Trace as usize], "TRACE");
}
