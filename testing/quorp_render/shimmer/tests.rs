use super::*;

#[test]
fn truecolor_frame_contains_escapes_and_verb_chars() {
    let s = render_shimmer(
        "Cogitating",
        0.5,
        ShimmerStyle::default(),
        ColorCapability::TrueColor,
    );
    assert!(s.contains("\x1b[38;2"));
    assert!(s.contains('C'));
    assert!(s.ends_with("\x1b[0m"));
}

#[test]
fn no_color_falls_back_to_braille() {
    let s = render_shimmer(
        "Reading",
        0.0,
        ShimmerStyle::default(),
        ColorCapability::NoColor,
    );
    assert!(s.contains("Reading"));
    assert!(s.starts_with('⠋') || s.starts_with('⠙') || s.starts_with('⠹') || s.starts_with('⠸'));
}

#[test]
fn frame_at_zero_time_has_stable_first_color() {
    let a = render_shimmer(
        "X",
        0.0,
        ShimmerStyle::default(),
        ColorCapability::TrueColor,
    );
    let b = render_shimmer(
        "X",
        0.0,
        ShimmerStyle::default(),
        ColorCapability::TrueColor,
    );
    assert_eq!(a, b);
}

#[test]
fn width_is_grapheme_aware() {
    assert_eq!(shimmer_visible_width("hello"), 5);
    // Wide kana counts as 2 columns each.
    assert_eq!(shimmer_visible_width("こんにちは"), 10);
}
