use super::{generate_marked_text, marked_text_ranges};

#[allow(clippy::reversed_empty_ranges)]
#[test]
fn test_marked_text() {
    let (text, ranges) = marked_text_ranges("one «ˇtwo» «threeˇ» «ˇfour» fiveˇ six", true);

    assert_eq!(text, "one two three four five six");
    assert_eq!(ranges.len(), 4);
    assert_eq!(ranges[0], 7..4);
    assert_eq!(ranges[1], 8..13);
    assert_eq!(ranges[2], 18..14);
    assert_eq!(ranges[3], 23..23);

    assert_eq!(
        generate_marked_text(&text, &ranges, true),
        "one «ˇtwo» «threeˇ» «ˇfour» fiveˇ six"
    );
}
