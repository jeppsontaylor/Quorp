use super::*;

#[test]
fn test_markdown_escaped() {
    let input = r#"
    # Heading

    Another heading
    ===

    Another heading variant
    ---

    Paragraph with [link](https://example.com) and `code`, *emphasis*, and ~strikethrough~.

    ```
    code block
    ```

    List with varying leaders:
      - Item 1
      * Item 2
      + Item 3

    Some math:  $`\sqrt{3x-1}+(1+x)^2`$

    HTML entity: &nbsp;
    "#;

    let expected = r#"
    \# Heading

    Another heading
    \=\=\=

    Another heading variant
    \-\-\-

    Paragraph with \[link](https://example.com) and \`code\`, \*emphasis\*, and \~strikethrough\~.

    \`\`\`
    code block
    \`\`\`

    List with varying leaders:
      \- Item 1
      \* Item 2
      \+ Item 3

    Some math:  \$\`\\sqrt{3x\-1}\+(1\+x)\^2\`\$

    HTML entity: \&nbsp;
    "#;

    assert_eq!(MarkdownEscaped(input).to_string(), expected);
}

#[test]
fn test_markdown_inline_code() {
    assert_eq!(MarkdownInlineCode(" ").to_string(), "` `");
    assert_eq!(MarkdownInlineCode("text").to_string(), "`text`");
    assert_eq!(MarkdownInlineCode("text ").to_string(), "`text `");
    assert_eq!(MarkdownInlineCode(" text ").to_string(), "`  text  `");
    assert_eq!(MarkdownInlineCode("`").to_string(), "`` ` ``");
    assert_eq!(MarkdownInlineCode("``").to_string(), "``` `` ```");
    assert_eq!(MarkdownInlineCode("`text`").to_string(), "`` `text` ``");
    assert_eq!(
        MarkdownInlineCode("some `text` no leading or trailing backticks").to_string(),
        "``some `text` no leading or trailing backticks``"
    );
}

#[test]
fn test_count_max_consecutive_chars() {
    assert_eq!(
        count_max_consecutive_chars("``a```b``", '`'),
        3,
        "the highest seen consecutive segment of backticks counts"
    );
    assert_eq!(
        count_max_consecutive_chars("```a``b`", '`'),
        3,
        "it can't be downgraded later"
    );
}
