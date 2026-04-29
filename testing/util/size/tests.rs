use super::*;

#[test]
fn test_format_file_size_decimal() {
    assert_eq!(format_file_size(0, true), "0B");
    assert_eq!(format_file_size(999, true), "999B");
    assert_eq!(format_file_size(1000, true), "1.0KB");
    assert_eq!(format_file_size(1500, true), "1.5KB");
    assert_eq!(format_file_size(999999, true), "1000.0KB");
    assert_eq!(format_file_size(1000000, true), "1.0MB");
    assert_eq!(format_file_size(1500000, true), "1.5MB");
    assert_eq!(format_file_size(10000000, true), "10.0MB");
}

#[test]
fn test_format_file_size_binary() {
    assert_eq!(format_file_size(0, false), "0B");
    assert_eq!(format_file_size(1023, false), "1023B");
    assert_eq!(format_file_size(1024, false), "1.0KiB");
    assert_eq!(format_file_size(1536, false), "1.5KiB");
    assert_eq!(format_file_size(1048575, false), "1024.0KiB");
    assert_eq!(format_file_size(1048576, false), "1.0MiB");
    assert_eq!(format_file_size(1572864, false), "1.5MiB");
    assert_eq!(format_file_size(10485760, false), "10.0MiB");
}
