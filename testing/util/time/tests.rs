use super::*;

#[test]
fn test_duration_alt_display() {
    use duration_alt_display as f;
    assert_eq!("0s", f(Duration::from_secs(0)));
    assert_eq!("59s", f(Duration::from_secs(59)));
    assert_eq!("1m 0s", f(Duration::from_secs(60)));
    assert_eq!("10m 0s", f(Duration::from_secs(600)));
    assert_eq!("1h 0m 0s", f(Duration::from_secs(3600)));
    assert_eq!("3h 2m 1s", f(Duration::from_secs(3600 * 3 + 60 * 2 + 1)));
    assert_eq!("23h 59m 59s", f(Duration::from_secs(3600 * 24 - 1)));
    assert_eq!("100h 0m 0s", f(Duration::from_secs(3600 * 100)));
}
