use super::*;

#[test]
fn test_extend_sorted() {
    let mut vec = vec![];

    extend_sorted(&mut vec, vec![21, 17, 13, 8, 1, 0], 5, |a, b| b.cmp(a));
    assert_eq!(vec, &[21, 17, 13, 8, 1]);

    extend_sorted(&mut vec, vec![101, 19, 17, 8, 2], 8, |a, b| b.cmp(a));
    assert_eq!(vec, &[101, 21, 19, 17, 13, 8, 2, 1]);

    extend_sorted(&mut vec, vec![1000, 19, 17, 9, 5], 8, |a, b| b.cmp(a));
    assert_eq!(vec, &[1000, 101, 21, 19, 17, 13, 9, 8]);
}

#[test]
fn test_truncate_to_bottom_n_sorted_by() {
    let mut vec: Vec<u32> = vec![5, 2, 3, 4, 1];
    truncate_to_bottom_n_sorted_by(&mut vec, 10, &u32::cmp);
    assert_eq!(vec, &[1, 2, 3, 4, 5]);

    vec = vec![5, 2, 3, 4, 1];
    truncate_to_bottom_n_sorted_by(&mut vec, 5, &u32::cmp);
    assert_eq!(vec, &[1, 2, 3, 4, 5]);

    vec = vec![5, 2, 3, 4, 1];
    truncate_to_bottom_n_sorted_by(&mut vec, 4, &u32::cmp);
    assert_eq!(vec, &[1, 2, 3, 4]);

    vec = vec![5, 2, 3, 4, 1];
    truncate_to_bottom_n_sorted_by(&mut vec, 1, &u32::cmp);
    assert_eq!(vec, &[1]);

    vec = vec![5, 2, 3, 4, 1];
    truncate_to_bottom_n_sorted_by(&mut vec, 0, &u32::cmp);
    assert!(vec.is_empty());
}

#[test]
fn test_iife() {
    fn option_returning_function() -> Option<()> {
        None
    }

    let foo = maybe!({
        option_returning_function()?;
        Some(())
    });

    assert_eq!(foo, None);
}

#[test]
fn test_truncate_and_trailoff() {
    assert_eq!(truncate_and_trailoff("", 5), "");
    assert_eq!(truncate_and_trailoff("aaaaaa", 7), "aaaaaa");
    assert_eq!(truncate_and_trailoff("aaaaaa", 6), "aaaaaa");
    assert_eq!(truncate_and_trailoff("aaaaaa", 5), "aaaaa…");
    assert_eq!(truncate_and_trailoff("èèèèèè", 7), "èèèèèè");
    assert_eq!(truncate_and_trailoff("èèèèèè", 6), "èèèèèè");
    assert_eq!(truncate_and_trailoff("èèèèèè", 5), "èèèèè…");
}

#[test]
fn test_truncate_and_remove_front() {
    assert_eq!(truncate_and_remove_front("", 5), "");
    assert_eq!(truncate_and_remove_front("aaaaaa", 7), "aaaaaa");
    assert_eq!(truncate_and_remove_front("aaaaaa", 6), "aaaaaa");
    assert_eq!(truncate_and_remove_front("aaaaaa", 5), "…aaaaa");
    assert_eq!(truncate_and_remove_front("èèèèèè", 7), "èèèèèè");
    assert_eq!(truncate_and_remove_front("èèèèèè", 6), "èèèèèè");
    assert_eq!(truncate_and_remove_front("èèèèèè", 5), "…èèèèè");
}

#[test]
fn test_numeric_prefix_str_method() {
    let target = "1a";
    assert_eq!(
        NumericPrefixWithSuffix::from_numeric_prefixed_str(target),
        NumericPrefixWithSuffix(Some(1), "a")
    );

    let target = "12ab";
    assert_eq!(
        NumericPrefixWithSuffix::from_numeric_prefixed_str(target),
        NumericPrefixWithSuffix(Some(12), "ab")
    );

    let target = "12_ab";
    assert_eq!(
        NumericPrefixWithSuffix::from_numeric_prefixed_str(target),
        NumericPrefixWithSuffix(Some(12), "_ab")
    );

    let target = "1_2ab";
    assert_eq!(
        NumericPrefixWithSuffix::from_numeric_prefixed_str(target),
        NumericPrefixWithSuffix(Some(1), "_2ab")
    );

    let target = "1.2";
    assert_eq!(
        NumericPrefixWithSuffix::from_numeric_prefixed_str(target),
        NumericPrefixWithSuffix(Some(1), ".2")
    );

    let target = "1.2_a";
    assert_eq!(
        NumericPrefixWithSuffix::from_numeric_prefixed_str(target),
        NumericPrefixWithSuffix(Some(1), ".2_a")
    );

    let target = "12.2_a";
    assert_eq!(
        NumericPrefixWithSuffix::from_numeric_prefixed_str(target),
        NumericPrefixWithSuffix(Some(12), ".2_a")
    );

    let target = "12a.2_a";
    assert_eq!(
        NumericPrefixWithSuffix::from_numeric_prefixed_str(target),
        NumericPrefixWithSuffix(Some(12), "a.2_a")
    );
}

#[test]
fn test_numeric_prefix_with_suffix() {
    let mut sorted = vec!["1-abc", "10", "11def", "2", "21-abc"];
    sorted.sort_by_key(|s| NumericPrefixWithSuffix::from_numeric_prefixed_str(s));
    assert_eq!(sorted, ["1-abc", "2", "10", "11def", "21-abc"]);

    for numeric_prefix_less in ["numeric_prefix_less", "aaa", "~™£"] {
        assert_eq!(
            NumericPrefixWithSuffix::from_numeric_prefixed_str(numeric_prefix_less),
            NumericPrefixWithSuffix(None, numeric_prefix_less),
            "String without numeric prefix `{numeric_prefix_less}` should not be converted into NumericPrefixWithSuffix"
        )
    }
}

#[test]
fn test_word_consists_of_emojis() {
    let words_to_test = vec![
        ("👨‍👩‍👧‍👧👋🥒", true),
        ("👋", true),
        ("!👋", false),
        ("👋!", false),
        ("👋 ", false),
        (" 👋", false),
        ("Test", false),
    ];

    for (text, expected_result) in words_to_test {
        assert_eq!(word_consists_of_emojis(text), expected_result);
    }
}

#[test]
fn test_truncate_lines_and_trailoff() {
    let text = r#"Line 1
Line 2
Line 3"#;

    assert_eq!(
        truncate_lines_and_trailoff(text, 2),
        r#"Line 1
…"#
    );

    assert_eq!(
        truncate_lines_and_trailoff(text, 3),
        r#"Line 1
Line 2
…"#
    );

    assert_eq!(
        truncate_lines_and_trailoff(text, 4),
        r#"Line 1
Line 2
Line 3"#
    );
}

#[test]
fn test_expanded_and_wrapped_usize_range() {
    // Neither wrap
    assert_eq!(
        expanded_and_wrapped_usize_range(2..4, 1, 1, 8).collect::<Vec<usize>>(),
        (1..5).collect::<Vec<usize>>()
    );
    // Start wraps
    assert_eq!(
        expanded_and_wrapped_usize_range(2..4, 3, 1, 8).collect::<Vec<usize>>(),
        ((0..5).chain(7..8)).collect::<Vec<usize>>()
    );
    // Start wraps all the way around
    assert_eq!(
        expanded_and_wrapped_usize_range(2..4, 5, 1, 8).collect::<Vec<usize>>(),
        (0..8).collect::<Vec<usize>>()
    );
    // Start wraps all the way around and past 0
    assert_eq!(
        expanded_and_wrapped_usize_range(2..4, 10, 1, 8).collect::<Vec<usize>>(),
        (0..8).collect::<Vec<usize>>()
    );
    // End wraps
    assert_eq!(
        expanded_and_wrapped_usize_range(3..5, 1, 4, 8).collect::<Vec<usize>>(),
        (0..1).chain(2..8).collect::<Vec<usize>>()
    );
    // End wraps all the way around
    assert_eq!(
        expanded_and_wrapped_usize_range(3..5, 1, 5, 8).collect::<Vec<usize>>(),
        (0..8).collect::<Vec<usize>>()
    );
    // End wraps all the way around and past the end
    assert_eq!(
        expanded_and_wrapped_usize_range(3..5, 1, 10, 8).collect::<Vec<usize>>(),
        (0..8).collect::<Vec<usize>>()
    );
    // Both start and end wrap
    assert_eq!(
        expanded_and_wrapped_usize_range(3..5, 4, 4, 8).collect::<Vec<usize>>(),
        (0..8).collect::<Vec<usize>>()
    );
}

#[test]
fn test_wrapped_usize_outward_from() {
    // No wrapping
    assert_eq!(
        wrapped_usize_outward_from(4, 2, 2, 10).collect::<Vec<usize>>(),
        vec![4, 5, 3, 6, 2]
    );
    // Wrapping at end
    assert_eq!(
        wrapped_usize_outward_from(8, 2, 3, 10).collect::<Vec<usize>>(),
        vec![8, 9, 7, 0, 6, 1]
    );
    // Wrapping at start
    assert_eq!(
        wrapped_usize_outward_from(1, 3, 2, 10).collect::<Vec<usize>>(),
        vec![1, 2, 0, 3, 9, 8]
    );
    // All values wrap around
    assert_eq!(
        wrapped_usize_outward_from(5, 10, 10, 8).collect::<Vec<usize>>(),
        vec![5, 6, 4, 7, 3, 0, 2, 1]
    );
    // None before / after
    assert_eq!(
        wrapped_usize_outward_from(3, 0, 0, 8).collect::<Vec<usize>>(),
        vec![3]
    );
    // Starting point already wrapped
    assert_eq!(
        wrapped_usize_outward_from(15, 2, 2, 10).collect::<Vec<usize>>(),
        vec![5, 6, 4, 7, 3]
    );
    // wrap_length of 0
    assert_eq!(
        wrapped_usize_outward_from(4, 2, 2, 0).collect::<Vec<usize>>(),
        Vec::<usize>::new()
    );
}

#[test]
fn test_split_with_ranges() {
    let input = "hi";
    let result = split_str_with_ranges(input, &|c| c == ' ');

    assert_eq!(result.len(), 1);
    assert_eq!(result[0], (0..2, "hi"));

    let input = "héllo🦀world";
    let result = split_str_with_ranges(input, &|c| c == '🦀');

    assert_eq!(result.len(), 2);
    assert_eq!(result[0], (0..6, "héllo")); // 'é' is 2 bytes
    assert_eq!(result[1], (10..15, "world")); // '🦀' is 4 bytes
}

#[test]
fn test_truncate_lines_to_byte_limit() {
    let text = "Line 1\nLine 2\nLine 3\nLine 4";

    // Limit that includes all lines
    assert_eq!(truncate_lines_to_byte_limit(text, 100), text);

    // Exactly the first line
    assert_eq!(truncate_lines_to_byte_limit(text, 7), "Line 1\n");

    // Limit between lines
    assert_eq!(truncate_lines_to_byte_limit(text, 13), "Line 1\n");
    assert_eq!(truncate_lines_to_byte_limit(text, 20), "Line 1\nLine 2\n");

    // Limit before first newline
    assert_eq!(truncate_lines_to_byte_limit(text, 6), "Line ");

    // Test with non-ASCII characters
    let text_utf8 = "Line 1\nLíne 2\nLine 3";
    assert_eq!(
        truncate_lines_to_byte_limit(text_utf8, 15),
        "Line 1\nLíne 2\n"
    );
}
