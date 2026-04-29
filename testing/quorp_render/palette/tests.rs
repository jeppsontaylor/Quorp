use super::*;

#[test]
fn fg_escape_is_correct() {
    let red = Rgb::new(255, 0, 0);
    assert_eq!(red.fg(), "\x1b[38;2;255;0;0m");
}

#[test]
fn lerp_endpoints_unchanged() {
    let a = Rgb::new(0, 0, 0);
    let b = Rgb::new(255, 255, 255);
    assert_eq!(lerp_rgb(a, b, 0.0), a);
    assert_eq!(lerp_rgb(a, b, 1.0), b);
    let mid = lerp_rgb(a, b, 0.5);
    assert!(mid.r >= 126 && mid.r <= 129);
}
