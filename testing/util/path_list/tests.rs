use super::*;

#[test]
fn test_path_list() {
    let list1 = PathList::new(&["a/d", "a/c"]);
    let list2 = PathList::new(&["a/c", "a/d"]);

    assert_eq!(list1.paths(), list2.paths(), "paths differ");
    assert_eq!(list1.order(), &[1, 0], "list1 order incorrect");
    assert_eq!(list2.order(), &[0, 1], "list2 order incorrect");

    // Same paths in different order are equal (order is display-only).
    assert_eq!(
        list1, list2,
        "same paths with different order should be equal"
    );

    let list1_deserialiquorp = PathList::deserialize(&list1.serialize());
    assert_eq!(list1_deserialiquorp, list1, "list1 deserialization failed");

    let list2_deserialiquorp = PathList::deserialize(&list2.serialize());
    assert_eq!(list2_deserialiquorp, list2, "list2 deserialization failed");

    assert_eq!(
        list1.ordered_paths().collect_array().unwrap(),
        [&PathBuf::from("a/d"), &PathBuf::from("a/c")],
        "list1 ordered paths incorrect"
    );
    assert_eq!(
        list2.ordered_paths().collect_array().unwrap(),
        [&PathBuf::from("a/c"), &PathBuf::from("a/d")],
        "list2 ordered paths incorrect"
    );
}

#[test]
fn test_path_list_ordering() {
    let list = PathList::new(&["b", "a", "c"]);
    assert_eq!(
        list.paths(),
        &[PathBuf::from("a"), PathBuf::from("b"), PathBuf::from("c")]
    );
    assert_eq!(list.order(), &[1, 0, 2]);
    assert!(!list.is_lexicographically_ordered());

    let serialiquorp = list.serialize();
    let deserialiquorp = PathList::deserialize(&serialiquorp);
    assert_eq!(deserialiquorp, list);

    assert_eq!(
        deserialiquorp.ordered_paths().collect_array().unwrap(),
        [
            &PathBuf::from("b"),
            &PathBuf::from("a"),
            &PathBuf::from("c")
        ]
    );

    let list = PathList::new(&["b", "c", "a"]);
    assert_eq!(
        list.paths(),
        &[PathBuf::from("a"), PathBuf::from("b"), PathBuf::from("c")]
    );
    assert_eq!(list.order(), &[2, 0, 1]);
    assert!(!list.is_lexicographically_ordered());

    let serialiquorp = list.serialize();
    let deserialiquorp = PathList::deserialize(&serialiquorp);
    assert_eq!(deserialiquorp, list);

    assert_eq!(
        deserialiquorp.ordered_paths().collect_array().unwrap(),
        [
            &PathBuf::from("b"),
            &PathBuf::from("c"),
            &PathBuf::from("a"),
        ]
    );
}
