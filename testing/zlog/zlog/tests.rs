use super::*;

#[test]
fn test_crate_name() {
    assert_eq!(crate_name!(), "quorp_log");
    assert_eq!(
        private::extract_crate_name_from_module_path("my_speedy_⚡️_crate::some_module"),
        "my_speedy_⚡️_crate"
    );
    assert_eq!(
        private::extract_crate_name_from_module_path("my_speedy_crate_⚡️::some_module"),
        "my_speedy_crate_⚡️"
    );
    assert_eq!(
        private::extract_crate_name_from_module_path("my_speedy_crate_:⚡️:some_module"),
        "my_speedy_crate_:⚡️:some_module"
    );
    assert_eq!(
        private::extract_crate_name_from_module_path("my_speedy_crate_::⚡️some_module"),
        "my_speedy_crate_"
    );
}
