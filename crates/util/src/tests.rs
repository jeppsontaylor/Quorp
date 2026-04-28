use crate::rel_path::rel_path;

use super::*;
use util_macros::perf;

#[perf]
fn compare_paths_with_dots() {
    let mut paths = vec![
        (Path::new("test_dirs"), false),
        (Path::new("test_dirs/1.46"), false),
        (Path::new("test_dirs/1.46/bar_1"), true),
        (Path::new("test_dirs/1.46/bar_2"), true),
        (Path::new("test_dirs/1.45"), false),
        (Path::new("test_dirs/1.45/foo_2"), true),
        (Path::new("test_dirs/1.45/foo_1"), true),
    ];
    paths.sort_by(|&a, &b| compare_paths(a, b));
    assert_eq!(
        paths,
        vec![
            (Path::new("test_dirs"), false),
            (Path::new("test_dirs/1.45"), false),
            (Path::new("test_dirs/1.45/foo_1"), true),
            (Path::new("test_dirs/1.45/foo_2"), true),
            (Path::new("test_dirs/1.46"), false),
            (Path::new("test_dirs/1.46/bar_1"), true),
            (Path::new("test_dirs/1.46/bar_2"), true),
        ]
    );
    let mut paths = vec![
        (Path::new("root1/one.txt"), true),
        (Path::new("root1/one.two.txt"), true),
    ];
    paths.sort_by(|&a, &b| compare_paths(a, b));
    assert_eq!(
        paths,
        vec![
            (Path::new("root1/one.txt"), true),
            (Path::new("root1/one.two.txt"), true),
        ]
    );
}

#[perf]
fn compare_paths_with_same_name_different_extensions() {
    let mut paths = vec![
        (Path::new("test_dirs/file.rs"), true),
        (Path::new("test_dirs/file.txt"), true),
        (Path::new("test_dirs/file.md"), true),
        (Path::new("test_dirs/file"), true),
        (Path::new("test_dirs/file.a"), true),
    ];
    paths.sort_by(|&a, &b| compare_paths(a, b));
    assert_eq!(
        paths,
        vec![
            (Path::new("test_dirs/file"), true),
            (Path::new("test_dirs/file.a"), true),
            (Path::new("test_dirs/file.md"), true),
            (Path::new("test_dirs/file.rs"), true),
            (Path::new("test_dirs/file.txt"), true),
        ]
    );
}

#[perf]
fn compare_paths_case_semi_sensitive() {
    let mut paths = vec![
        (Path::new("test_DIRS"), false),
        (Path::new("test_DIRS/foo_1"), true),
        (Path::new("test_DIRS/foo_2"), true),
        (Path::new("test_DIRS/bar"), true),
        (Path::new("test_DIRS/BAR"), true),
        (Path::new("test_dirs"), false),
        (Path::new("test_dirs/foo_1"), true),
        (Path::new("test_dirs/foo_2"), true),
        (Path::new("test_dirs/bar"), true),
        (Path::new("test_dirs/BAR"), true),
    ];
    paths.sort_by(|&a, &b| compare_paths(a, b));
    assert_eq!(
        paths,
        vec![
            (Path::new("test_dirs"), false),
            (Path::new("test_dirs/bar"), true),
            (Path::new("test_dirs/BAR"), true),
            (Path::new("test_dirs/foo_1"), true),
            (Path::new("test_dirs/foo_2"), true),
            (Path::new("test_DIRS"), false),
            (Path::new("test_DIRS/bar"), true),
            (Path::new("test_DIRS/BAR"), true),
            (Path::new("test_DIRS/foo_1"), true),
            (Path::new("test_DIRS/foo_2"), true),
        ]
    );
}

#[perf]
fn compare_paths_mixed_case_numeric_ordering() {
    let mut entries = [
        (Path::new(".config"), false),
        (Path::new("Dir1"), false),
        (Path::new("dir01"), false),
        (Path::new("dir2"), false),
        (Path::new("Dir02"), false),
        (Path::new("dir10"), false),
        (Path::new("Dir10"), false),
    ];

    entries.sort_by(|&a, &b| compare_paths(a, b));

    let ordered: Vec<&str> = entries
        .iter()
        .map(|(path, _)| path.to_str().unwrap())
        .collect();

    assert_eq!(
        ordered,
        vec![
            ".config", "Dir1", "dir01", "dir2", "Dir02", "dir10", "Dir10"
        ]
    );
}

#[perf]
fn compare_rel_paths_mixed_case_insensitive() {
    // Test that mixed mode is case-insensitive
    let mut paths = vec![
        (RelPath::unix("zebra.txt").unwrap(), true),
        (RelPath::unix("Apple").unwrap(), false),
        (RelPath::unix("banana.rs").unwrap(), true),
        (RelPath::unix("Carrot").unwrap(), false),
        (RelPath::unix("aardvark.txt").unwrap(), true),
    ];
    paths.sort_by(|&a, &b| compare_rel_paths_mixed(a, b));
    // Case-insensitive: aardvark < Apple < banana < Carrot < zebra
    assert_eq!(
        paths,
        vec![
            (RelPath::unix("aardvark.txt").unwrap(), true),
            (RelPath::unix("Apple").unwrap(), false),
            (RelPath::unix("banana.rs").unwrap(), true),
            (RelPath::unix("Carrot").unwrap(), false),
            (RelPath::unix("zebra.txt").unwrap(), true),
        ]
    );
}

#[perf]
fn compare_rel_paths_files_first_basic() {
    // Test that files come before directories
    let mut paths = vec![
        (RelPath::unix("zebra.txt").unwrap(), true),
        (RelPath::unix("Apple").unwrap(), false),
        (RelPath::unix("banana.rs").unwrap(), true),
        (RelPath::unix("Carrot").unwrap(), false),
        (RelPath::unix("aardvark.txt").unwrap(), true),
    ];
    paths.sort_by(|&a, &b| compare_rel_paths_files_first(a, b));
    // Files first (case-insensitive), then directories (case-insensitive)
    assert_eq!(
        paths,
        vec![
            (RelPath::unix("aardvark.txt").unwrap(), true),
            (RelPath::unix("banana.rs").unwrap(), true),
            (RelPath::unix("zebra.txt").unwrap(), true),
            (RelPath::unix("Apple").unwrap(), false),
            (RelPath::unix("Carrot").unwrap(), false),
        ]
    );
}

#[perf]
fn compare_rel_paths_files_first_case_insensitive() {
    // Test case-insensitive sorting within files and directories
    let mut paths = vec![
        (RelPath::unix("Zebra.txt").unwrap(), true),
        (RelPath::unix("apple").unwrap(), false),
        (RelPath::unix("Banana.rs").unwrap(), true),
        (RelPath::unix("carrot").unwrap(), false),
        (RelPath::unix("Aardvark.txt").unwrap(), true),
    ];
    paths.sort_by(|&a, &b| compare_rel_paths_files_first(a, b));
    assert_eq!(
        paths,
        vec![
            (RelPath::unix("Aardvark.txt").unwrap(), true),
            (RelPath::unix("Banana.rs").unwrap(), true),
            (RelPath::unix("Zebra.txt").unwrap(), true),
            (RelPath::unix("apple").unwrap(), false),
            (RelPath::unix("carrot").unwrap(), false),
        ]
    );
}

#[perf]
fn compare_rel_paths_files_first_numeric() {
    // Test natural number sorting with files first
    let mut paths = vec![
        (RelPath::unix("file10.txt").unwrap(), true),
        (RelPath::unix("dir2").unwrap(), false),
        (RelPath::unix("file2.txt").unwrap(), true),
        (RelPath::unix("dir10").unwrap(), false),
        (RelPath::unix("file1.txt").unwrap(), true),
    ];
    paths.sort_by(|&a, &b| compare_rel_paths_files_first(a, b));
    assert_eq!(
        paths,
        vec![
            (RelPath::unix("file1.txt").unwrap(), true),
            (RelPath::unix("file2.txt").unwrap(), true),
            (RelPath::unix("file10.txt").unwrap(), true),
            (RelPath::unix("dir2").unwrap(), false),
            (RelPath::unix("dir10").unwrap(), false),
        ]
    );
}

#[perf]
fn compare_rel_paths_mixed_case() {
    // Test case-insensitive sorting with varied capitalization
    let mut paths = vec![
        (RelPath::unix("README.md").unwrap(), true),
        (RelPath::unix("readme.txt").unwrap(), true),
        (RelPath::unix("ReadMe.rs").unwrap(), true),
    ];
    paths.sort_by(|&a, &b| compare_rel_paths_mixed(a, b));
    // All "readme" variants should group together, sorted by extension
    assert_eq!(
        paths,
        vec![
            (RelPath::unix("readme.txt").unwrap(), true),
            (RelPath::unix("ReadMe.rs").unwrap(), true),
            (RelPath::unix("README.md").unwrap(), true),
        ]
    );
}

#[perf]
fn compare_rel_paths_mixed_files_and_dirs() {
    // Verify directories and files are still mixed
    let mut paths = vec![
        (RelPath::unix("file2.txt").unwrap(), true),
        (RelPath::unix("Dir1").unwrap(), false),
        (RelPath::unix("file1.txt").unwrap(), true),
        (RelPath::unix("dir2").unwrap(), false),
    ];
    paths.sort_by(|&a, &b| compare_rel_paths_mixed(a, b));
    // Case-insensitive: dir1, dir2, file1, file2 (all mixed)
    assert_eq!(
        paths,
        vec![
            (RelPath::unix("Dir1").unwrap(), false),
            (RelPath::unix("dir2").unwrap(), false),
            (RelPath::unix("file1.txt").unwrap(), true),
            (RelPath::unix("file2.txt").unwrap(), true),
        ]
    );
}

#[perf]
fn compare_rel_paths_mixed_same_name_different_case_file_and_dir() {
    let mut paths = vec![
        (RelPath::unix("Hello.txt").unwrap(), true),
        (RelPath::unix("hello").unwrap(), false),
    ];
    paths.sort_by(|&a, &b| compare_rel_paths_mixed(a, b));
    assert_eq!(
        paths,
        vec![
            (RelPath::unix("hello").unwrap(), false),
            (RelPath::unix("Hello.txt").unwrap(), true),
        ]
    );

    let mut paths = vec![
        (RelPath::unix("hello").unwrap(), false),
        (RelPath::unix("Hello.txt").unwrap(), true),
    ];
    paths.sort_by(|&a, &b| compare_rel_paths_mixed(a, b));
    assert_eq!(
        paths,
        vec![
            (RelPath::unix("hello").unwrap(), false),
            (RelPath::unix("Hello.txt").unwrap(), true),
        ]
    );
}

#[perf]
fn compare_rel_paths_mixed_with_nested_paths() {
    // Test that nested paths still work correctly
    let mut paths = vec![
        (RelPath::unix("src/main.rs").unwrap(), true),
        (RelPath::unix("Cargo.toml").unwrap(), true),
        (RelPath::unix("src").unwrap(), false),
        (RelPath::unix("target").unwrap(), false),
    ];
    paths.sort_by(|&a, &b| compare_rel_paths_mixed(a, b));
    assert_eq!(
        paths,
        vec![
            (RelPath::unix("Cargo.toml").unwrap(), true),
            (RelPath::unix("src").unwrap(), false),
            (RelPath::unix("src/main.rs").unwrap(), true),
            (RelPath::unix("target").unwrap(), false),
        ]
    );
}

#[perf]
fn compare_rel_paths_files_first_with_nested() {
    // Files come before directories, even with nested paths
    let mut paths = vec![
        (RelPath::unix("src/lib.rs").unwrap(), true),
        (RelPath::unix("README.md").unwrap(), true),
        (RelPath::unix("src").unwrap(), false),
        (RelPath::unix("tests").unwrap(), false),
    ];
    paths.sort_by(|&a, &b| compare_rel_paths_files_first(a, b));
    assert_eq!(
        paths,
        vec![
            (RelPath::unix("README.md").unwrap(), true),
            (RelPath::unix("src").unwrap(), false),
            (RelPath::unix("src/lib.rs").unwrap(), true),
            (RelPath::unix("tests").unwrap(), false),
        ]
    );
}

#[perf]
fn compare_rel_paths_mixed_dotfiles() {
    // Test that dotfiles are handled correctly in mixed mode
    let mut paths = vec![
        (RelPath::unix(".gitignore").unwrap(), true),
        (RelPath::unix("README.md").unwrap(), true),
        (RelPath::unix(".github").unwrap(), false),
        (RelPath::unix("src").unwrap(), false),
    ];
    paths.sort_by(|&a, &b| compare_rel_paths_mixed(a, b));
    assert_eq!(
        paths,
        vec![
            (RelPath::unix(".github").unwrap(), false),
            (RelPath::unix(".gitignore").unwrap(), true),
            (RelPath::unix("README.md").unwrap(), true),
            (RelPath::unix("src").unwrap(), false),
        ]
    );
}

#[perf]
fn compare_rel_paths_files_first_dotfiles() {
    // Test that dotfiles come first when they're files
    let mut paths = vec![
        (RelPath::unix(".gitignore").unwrap(), true),
        (RelPath::unix("README.md").unwrap(), true),
        (RelPath::unix(".github").unwrap(), false),
        (RelPath::unix("src").unwrap(), false),
    ];
    paths.sort_by(|&a, &b| compare_rel_paths_files_first(a, b));
    assert_eq!(
        paths,
        vec![
            (RelPath::unix(".gitignore").unwrap(), true),
            (RelPath::unix("README.md").unwrap(), true),
            (RelPath::unix(".github").unwrap(), false),
            (RelPath::unix("src").unwrap(), false),
        ]
    );
}

#[perf]
fn compare_rel_paths_mixed_same_stem_different_extension() {
    // Files with same stem but different extensions should sort by extension
    let mut paths = vec![
        (RelPath::unix("file.rs").unwrap(), true),
        (RelPath::unix("file.md").unwrap(), true),
        (RelPath::unix("file.txt").unwrap(), true),
    ];
    paths.sort_by(|&a, &b| compare_rel_paths_mixed(a, b));
    assert_eq!(
        paths,
        vec![
            (RelPath::unix("file.txt").unwrap(), true),
            (RelPath::unix("file.rs").unwrap(), true),
            (RelPath::unix("file.md").unwrap(), true),
        ]
    );
}

#[perf]
fn compare_rel_paths_files_first_same_stem() {
    // Same stem files should still sort by extension with files_first
    let mut paths = vec![
        (RelPath::unix("main.rs").unwrap(), true),
        (RelPath::unix("main.c").unwrap(), true),
        (RelPath::unix("main").unwrap(), false),
    ];
    paths.sort_by(|&a, &b| compare_rel_paths_files_first(a, b));
    assert_eq!(
        paths,
        vec![
            (RelPath::unix("main.c").unwrap(), true),
            (RelPath::unix("main.rs").unwrap(), true),
            (RelPath::unix("main").unwrap(), false),
        ]
    );
}

#[perf]
fn compare_rel_paths_mixed_deep_nesting() {
    // Test sorting with deeply nested paths
    let mut paths = vec![
        (RelPath::unix("a/b/c.txt").unwrap(), true),
        (RelPath::unix("A/B.txt").unwrap(), true),
        (RelPath::unix("a.txt").unwrap(), true),
        (RelPath::unix("A.txt").unwrap(), true),
    ];
    paths.sort_by(|&a, &b| compare_rel_paths_mixed(a, b));
    assert_eq!(
        paths,
        vec![
            (RelPath::unix("a/b/c.txt").unwrap(), true),
            (RelPath::unix("A/B.txt").unwrap(), true),
            (RelPath::unix("a.txt").unwrap(), true),
            (RelPath::unix("A.txt").unwrap(), true),
        ]
    );
}

#[perf]
fn path_with_position_parse_posix_path() {
    // Test POSIX filename edge cases
    // Read more at https://en.wikipedia.org/wiki/Filename
    assert_eq!(
        PathWithPosition::parse_str("test_file"),
        PathWithPosition {
            path: PathBuf::from("test_file"),
            row: None,
            column: None
        }
    );

    assert_eq!(
        PathWithPosition::parse_str("a:bc:.zip:1"),
        PathWithPosition {
            path: PathBuf::from("a:bc:.zip"),
            row: Some(1),
            column: None
        }
    );

    assert_eq!(
        PathWithPosition::parse_str("one.second.zip:1"),
        PathWithPosition {
            path: PathBuf::from("one.second.zip"),
            row: Some(1),
            column: None
        }
    );

    // Trim off trailing `:`s for otherwise valid input.
    assert_eq!(
        PathWithPosition::parse_str("test_file:10:1:"),
        PathWithPosition {
            path: PathBuf::from("test_file"),
            row: Some(10),
            column: Some(1)
        }
    );

    assert_eq!(
        PathWithPosition::parse_str("test_file.rs:"),
        PathWithPosition {
            path: PathBuf::from("test_file.rs"),
            row: None,
            column: None
        }
    );

    assert_eq!(
        PathWithPosition::parse_str("test_file.rs:1:"),
        PathWithPosition {
            path: PathBuf::from("test_file.rs"),
            row: Some(1),
            column: None
        }
    );

    assert_eq!(
        PathWithPosition::parse_str("ab\ncd"),
        PathWithPosition {
            path: PathBuf::from("ab\ncd"),
            row: None,
            column: None
        }
    );

    assert_eq!(
        PathWithPosition::parse_str("👋\nab"),
        PathWithPosition {
            path: PathBuf::from("👋\nab"),
            row: None,
            column: None
        }
    );

    assert_eq!(
        PathWithPosition::parse_str("Types.hs:(617,9)-(670,28):"),
        PathWithPosition {
            path: PathBuf::from("Types.hs"),
            row: Some(617),
            column: Some(9),
        }
    );

    assert_eq!(
        PathWithPosition::parse_str("main (1).log"),
        PathWithPosition {
            path: PathBuf::from("main (1).log"),
            row: None,
            column: None
        }
    );
}

#[perf]
#[cfg(not(target_os = "windows"))]
fn path_with_position_parse_posix_path_with_suffix() {
    assert_eq!(
        PathWithPosition::parse_str("foo/bar:34:in"),
        PathWithPosition {
            path: PathBuf::from("foo/bar"),
            row: Some(34),
            column: None,
        }
    );
    assert_eq!(
        PathWithPosition::parse_str("foo/bar.rs:1902:::15:"),
        PathWithPosition {
            path: PathBuf::from("foo/bar.rs:1902"),
            row: Some(15),
            column: None
        }
    );

    assert_eq!(
        PathWithPosition::parse_str("app-editors:quorp-0.143.6:20240710-201212.log:34:"),
        PathWithPosition {
            path: PathBuf::from("app-editors:quorp-0.143.6:20240710-201212.log"),
            row: Some(34),
            column: None,
        }
    );

    assert_eq!(
        PathWithPosition::parse_str("crates/file_finder/src/file_finder.rs:1902:13:"),
        PathWithPosition {
            path: PathBuf::from("crates/file_finder/src/file_finder.rs"),
            row: Some(1902),
            column: Some(13),
        }
    );

    assert_eq!(
        PathWithPosition::parse_str("crate/utils/src/test:today.log:34"),
        PathWithPosition {
            path: PathBuf::from("crate/utils/src/test:today.log"),
            row: Some(34),
            column: None,
        }
    );
    assert_eq!(
        PathWithPosition::parse_str("/testing/out/src/file_finder.odin(7:15)"),
        PathWithPosition {
            path: PathBuf::from("/testing/out/src/file_finder.odin"),
            row: Some(7),
            column: Some(15),
        }
    );
}

#[perf]
#[cfg(target_os = "windows")]
fn path_with_position_parse_windows_path() {
    assert_eq!(
        PathWithPosition::parse_str("crates\\utils\\paths.rs"),
        PathWithPosition {
            path: PathBuf::from("crates\\utils\\paths.rs"),
            row: None,
            column: None
        }
    );

    assert_eq!(
        PathWithPosition::parse_str("C:\\Users\\someone\\test_file.rs"),
        PathWithPosition {
            path: PathBuf::from("C:\\Users\\someone\\test_file.rs"),
            row: None,
            column: None
        }
    );

    assert_eq!(
        PathWithPosition::parse_str("C:\\Users\\someone\\main (1).log"),
        PathWithPosition {
            path: PathBuf::from("C:\\Users\\someone\\main (1).log"),
            row: None,
            column: None
        }
    );
}

#[perf]
#[cfg(target_os = "windows")]
fn path_with_position_parse_windows_path_with_suffix() {
    assert_eq!(
        PathWithPosition::parse_str("crates\\utils\\paths.rs:101"),
        PathWithPosition {
            path: PathBuf::from("crates\\utils\\paths.rs"),
            row: Some(101),
            column: None
        }
    );

    assert_eq!(
        PathWithPosition::parse_str("\\\\?\\C:\\Users\\someone\\test_file.rs:1:20"),
        PathWithPosition {
            path: PathBuf::from("\\\\?\\C:\\Users\\someone\\test_file.rs"),
            row: Some(1),
            column: Some(20)
        }
    );

    assert_eq!(
        PathWithPosition::parse_str("C:\\Users\\someone\\test_file.rs(1902,13)"),
        PathWithPosition {
            path: PathBuf::from("C:\\Users\\someone\\test_file.rs"),
            row: Some(1902),
            column: Some(13)
        }
    );

    // Trim off trailing `:`s for otherwise valid input.
    assert_eq!(
        PathWithPosition::parse_str("\\\\?\\C:\\Users\\someone\\test_file.rs:1902:13:"),
        PathWithPosition {
            path: PathBuf::from("\\\\?\\C:\\Users\\someone\\test_file.rs"),
            row: Some(1902),
            column: Some(13)
        }
    );

    assert_eq!(
        PathWithPosition::parse_str("\\\\?\\C:\\Users\\someone\\test_file.rs:1902:13:15:"),
        PathWithPosition {
            path: PathBuf::from("\\\\?\\C:\\Users\\someone\\test_file.rs:1902"),
            row: Some(13),
            column: Some(15)
        }
    );

    assert_eq!(
        PathWithPosition::parse_str("\\\\?\\C:\\Users\\someone\\test_file.rs:1902:::15:"),
        PathWithPosition {
            path: PathBuf::from("\\\\?\\C:\\Users\\someone\\test_file.rs:1902"),
            row: Some(15),
            column: None
        }
    );

    assert_eq!(
        PathWithPosition::parse_str("\\\\?\\C:\\Users\\someone\\test_file.rs(1902,13):"),
        PathWithPosition {
            path: PathBuf::from("\\\\?\\C:\\Users\\someone\\test_file.rs"),
            row: Some(1902),
            column: Some(13),
        }
    );

    assert_eq!(
        PathWithPosition::parse_str("\\\\?\\C:\\Users\\someone\\test_file.rs(1902):"),
        PathWithPosition {
            path: PathBuf::from("\\\\?\\C:\\Users\\someone\\test_file.rs"),
            row: Some(1902),
            column: None,
        }
    );

    assert_eq!(
        PathWithPosition::parse_str("C:\\Users\\someone\\test_file.rs:1902:13:"),
        PathWithPosition {
            path: PathBuf::from("C:\\Users\\someone\\test_file.rs"),
            row: Some(1902),
            column: Some(13),
        }
    );

    assert_eq!(
        PathWithPosition::parse_str("C:\\Users\\someone\\test_file.rs(1902,13):"),
        PathWithPosition {
            path: PathBuf::from("C:\\Users\\someone\\test_file.rs"),
            row: Some(1902),
            column: Some(13),
        }
    );

    assert_eq!(
        PathWithPosition::parse_str("C:\\Users\\someone\\test_file.rs(1902):"),
        PathWithPosition {
            path: PathBuf::from("C:\\Users\\someone\\test_file.rs"),
            row: Some(1902),
            column: None,
        }
    );

    assert_eq!(
        PathWithPosition::parse_str("crates/utils/paths.rs:101"),
        PathWithPosition {
            path: PathBuf::from("crates\\utils\\paths.rs"),
            row: Some(101),
            column: None,
        }
    );
}

#[perf]
fn test_path_compact() {
    let path: PathBuf = [
        home_dir().to_string_lossy().into_owned(),
        "some_file.txt".to_string(),
    ]
    .iter()
    .collect();
    if cfg!(any(target_os = "linux", target_os = "freebsd")) || cfg!(target_os = "macos") {
        assert_eq!(path.compact().to_str(), Some("~/some_file.txt"));
    } else {
        assert_eq!(path.compact().to_str(), path.to_str());
    }
}

#[perf]
fn test_extension_or_hidden_file_name() {
    // No dots in name
    let path = Path::new("/a/b/c/file_name.rs");
    assert_eq!(path.extension_or_hidden_file_name(), Some("rs"));

    // Single dot in name
    let path = Path::new("/a/b/c/file.name.rs");
    assert_eq!(path.extension_or_hidden_file_name(), Some("rs"));

    // Multiple dots in name
    let path = Path::new("/a/b/c/long.file.name.rs");
    assert_eq!(path.extension_or_hidden_file_name(), Some("rs"));

    // Hidden file, no extension
    let path = Path::new("/a/b/c/.gitignore");
    assert_eq!(path.extension_or_hidden_file_name(), Some("gitignore"));

    // Hidden file, with extension
    let path = Path::new("/a/b/c/.eslintrc.js");
    assert_eq!(path.extension_or_hidden_file_name(), Some("eslintrc.js"));
}

#[perf]
// fn edge_of_glob() {
//     let path = Path::new("/work/node_modules");
//     let path_matcher =
//         PathMatcher::new(&["**/node_modules/**".to_owned()], PathStyle::Posix).unwrap();
//     assert!(
//         path_matcher.is_match(path),
//         "Path matcher should match {path:?}"
//     );
// }

// #[perf]
// fn file_in_dirs() {
//     let path = Path::new("/work/.env");
//     let path_matcher = PathMatcher::new(&["**/.env".to_owned()], PathStyle::Posix).unwrap();
//     assert!(
//         path_matcher.is_match(path),
//         "Path matcher should match {path:?}"
//     );
//     let path = Path::new("/work/package.json");
//     assert!(
//         !path_matcher.is_match(path),
//         "Path matcher should not match {path:?}"
//     );
// }

// #[perf]
// fn project_search() {
//     let path = Path::new("/Users/someonetoignore/work/quorp/quorp.dev/node_modules");
//     let path_matcher =
//         PathMatcher::new(&["**/node_modules/**".to_owned()], PathStyle::Posix).unwrap();
//     assert!(
//         path_matcher.is_match(path),
//         "Path matcher should match {path:?}"
//     );
// }
#[perf]
#[cfg(target_os = "windows")]
fn test_sanitiquorp_path() {
    let path = Path::new("C:\\Users\\someone\\test_file.rs");
    let sanitiquorp_path = SanitiquorpPath::new(path);
    assert_eq!(
        sanitiquorp_path.to_string(),
        "C:\\Users\\someone\\test_file.rs"
    );

    let path = Path::new("\\\\?\\C:\\Users\\someone\\test_file.rs");
    let sanitiquorp_path = SanitiquorpPath::new(path);
    assert_eq!(
        sanitiquorp_path.to_string(),
        "C:\\Users\\someone\\test_file.rs"
    );
}

#[perf]
fn test_compare_numeric_segments() {
    // Helper function to create peekable iterators and test
    fn compare(a: &str, b: &str) -> Ordering {
        let mut a_iter = a.chars().peekable();
        let mut b_iter = b.chars().peekable();

        let result = compare_numeric_segments(&mut a_iter, &mut b_iter);

        // Verify iterators advanced correctly
        assert!(
            !a_iter.next().is_some_and(|c| c.is_ascii_digit()),
            "Iterator a should have consumed all digits"
        );
        assert!(
            !b_iter.next().is_some_and(|c| c.is_ascii_digit()),
            "Iterator b should have consumed all digits"
        );

        result
    }

    // Basic numeric comparisons
    assert_eq!(compare("0", "0"), Ordering::Equal);
    assert_eq!(compare("1", "2"), Ordering::Less);
    assert_eq!(compare("9", "10"), Ordering::Less);
    assert_eq!(compare("10", "9"), Ordering::Greater);
    assert_eq!(compare("99", "100"), Ordering::Less);

    // Leading zeros
    assert_eq!(compare("0", "00"), Ordering::Less);
    assert_eq!(compare("00", "0"), Ordering::Greater);
    assert_eq!(compare("01", "1"), Ordering::Greater);
    assert_eq!(compare("001", "1"), Ordering::Greater);
    assert_eq!(compare("001", "01"), Ordering::Greater);

    // Same value different representation
    assert_eq!(compare("000100", "100"), Ordering::Greater);
    assert_eq!(compare("100", "0100"), Ordering::Less);
    assert_eq!(compare("0100", "00100"), Ordering::Less);

    // Large numbers
    assert_eq!(compare("9999999999", "10000000000"), Ordering::Less);
    assert_eq!(
        compare(
            "340282366920938463463374607431768211455", // u128::MAX
            "340282366920938463463374607431768211456"
        ),
        Ordering::Less
    );
    assert_eq!(
        compare(
            "340282366920938463463374607431768211456", // > u128::MAX
            "340282366920938463463374607431768211455"
        ),
        Ordering::Greater
    );

    // Iterator advancement verification
    let mut a_iter = "123abc".chars().peekable();
    let mut b_iter = "456def".chars().peekable();

    compare_numeric_segments(&mut a_iter, &mut b_iter);

    assert_eq!(a_iter.collect::<String>(), "abc");
    assert_eq!(b_iter.collect::<String>(), "def");
}

#[perf]
fn test_natural_sort() {
    // Basic alphanumeric
    assert_eq!(natural_sort("a", "b"), Ordering::Less);
    assert_eq!(natural_sort("b", "a"), Ordering::Greater);
    assert_eq!(natural_sort("a", "a"), Ordering::Equal);

    // Case sensitivity
    assert_eq!(natural_sort("a", "A"), Ordering::Less);
    assert_eq!(natural_sort("A", "a"), Ordering::Greater);
    assert_eq!(natural_sort("aA", "aa"), Ordering::Greater);
    assert_eq!(natural_sort("aa", "aA"), Ordering::Less);

    // Numbers
    assert_eq!(natural_sort("1", "2"), Ordering::Less);
    assert_eq!(natural_sort("2", "10"), Ordering::Less);
    assert_eq!(natural_sort("02", "10"), Ordering::Less);
    assert_eq!(natural_sort("02", "2"), Ordering::Greater);

    // Mixed alphanumeric
    assert_eq!(natural_sort("a1", "a2"), Ordering::Less);
    assert_eq!(natural_sort("a2", "a10"), Ordering::Less);
    assert_eq!(natural_sort("a02", "a2"), Ordering::Greater);
    assert_eq!(natural_sort("a1b", "a1c"), Ordering::Less);

    // Multiple numeric segments
    assert_eq!(natural_sort("1a2", "1a10"), Ordering::Less);
    assert_eq!(natural_sort("1a10", "1a2"), Ordering::Greater);
    assert_eq!(natural_sort("2a1", "10a1"), Ordering::Less);

    // Special characters
    assert_eq!(natural_sort("a-1", "a-2"), Ordering::Less);
    assert_eq!(natural_sort("a_1", "a_2"), Ordering::Less);
    assert_eq!(natural_sort("a.1", "a.2"), Ordering::Less);

    // Unicode
    assert_eq!(natural_sort("文1", "文2"), Ordering::Less);
    assert_eq!(natural_sort("文2", "文10"), Ordering::Less);
    assert_eq!(natural_sort("🔤1", "🔤2"), Ordering::Less);

    // Empty and special cases
    assert_eq!(natural_sort("", ""), Ordering::Equal);
    assert_eq!(natural_sort("", "a"), Ordering::Less);
    assert_eq!(natural_sort("a", ""), Ordering::Greater);
    assert_eq!(natural_sort(" ", "  "), Ordering::Less);

    // Mixed everything
    assert_eq!(natural_sort("File-1.txt", "File-2.txt"), Ordering::Less);
    assert_eq!(natural_sort("File-02.txt", "File-2.txt"), Ordering::Greater);
    assert_eq!(natural_sort("File-2.txt", "File-10.txt"), Ordering::Less);
    assert_eq!(natural_sort("File_A1", "File_A2"), Ordering::Less);
    assert_eq!(natural_sort("File_a1", "File_A1"), Ordering::Less);
}

#[perf]
fn test_compare_paths() {
    // Helper function for cleaner tests
    fn compare(a: &str, is_a_file: bool, b: &str, is_b_file: bool) -> Ordering {
        compare_paths((Path::new(a), is_a_file), (Path::new(b), is_b_file))
    }

    // Basic path comparison
    assert_eq!(compare("a", true, "b", true), Ordering::Less);
    assert_eq!(compare("b", true, "a", true), Ordering::Greater);
    assert_eq!(compare("a", true, "a", true), Ordering::Equal);

    // Files vs Directories
    assert_eq!(compare("a", true, "a", false), Ordering::Greater);
    assert_eq!(compare("a", false, "a", true), Ordering::Less);
    assert_eq!(compare("b", false, "a", true), Ordering::Less);

    // Extensions
    assert_eq!(compare("a.txt", true, "a.md", true), Ordering::Greater);
    assert_eq!(compare("a.md", true, "a.txt", true), Ordering::Less);
    assert_eq!(compare("a", true, "a.txt", true), Ordering::Less);

    // Nested paths
    assert_eq!(compare("dir/a", true, "dir/b", true), Ordering::Less);
    assert_eq!(compare("dir1/a", true, "dir2/a", true), Ordering::Less);
    assert_eq!(compare("dir/sub/a", true, "dir/a", true), Ordering::Less);

    // Case sensitivity in paths
    assert_eq!(
        compare("Dir/file", true, "dir/file", true),
        Ordering::Greater
    );
    assert_eq!(
        compare("dir/File", true, "dir/file", true),
        Ordering::Greater
    );
    assert_eq!(compare("dir/file", true, "Dir/File", true), Ordering::Less);

    // Hidden files and special names
    assert_eq!(compare(".hidden", true, "visible", true), Ordering::Less);
    assert_eq!(compare("_special", true, "normal", true), Ordering::Less);
    assert_eq!(compare(".config", false, ".data", false), Ordering::Less);

    // Mixed numeric paths
    assert_eq!(
        compare("dir1/file", true, "dir2/file", true),
        Ordering::Less
    );
    assert_eq!(
        compare("dir2/file", true, "dir10/file", true),
        Ordering::Less
    );
    assert_eq!(
        compare("dir02/file", true, "dir2/file", true),
        Ordering::Greater
    );

    // Root paths
    assert_eq!(compare("/a", true, "/b", true), Ordering::Less);
    assert_eq!(compare("/", false, "/a", true), Ordering::Less);

    // Complex real-world examples
    assert_eq!(
        compare("project/src/main.rs", true, "project/src/lib.rs", true),
        Ordering::Greater
    );
    assert_eq!(
        compare(
            "project/tests/test_1.rs",
            true,
            "project/tests/test_2.rs",
            true
        ),
        Ordering::Less
    );
    assert_eq!(
        compare(
            "project/v1.0.0/README.md",
            true,
            "project/v1.10.0/README.md",
            true
        ),
        Ordering::Less
    );
}

#[perf]
fn test_natural_sort_case_sensitivity() {
    std::thread::sleep(std::time::Duration::from_millis(100));
    // Same letter different case - lowercase should come first
    assert_eq!(natural_sort("a", "A"), Ordering::Less);
    assert_eq!(natural_sort("A", "a"), Ordering::Greater);
    assert_eq!(natural_sort("a", "a"), Ordering::Equal);
    assert_eq!(natural_sort("A", "A"), Ordering::Equal);

    // Mixed case strings
    assert_eq!(natural_sort("aaa", "AAA"), Ordering::Less);
    assert_eq!(natural_sort("AAA", "aaa"), Ordering::Greater);
    assert_eq!(natural_sort("aAa", "AaA"), Ordering::Less);

    // Different letters
    assert_eq!(natural_sort("a", "b"), Ordering::Less);
    assert_eq!(natural_sort("A", "b"), Ordering::Less);
    assert_eq!(natural_sort("a", "B"), Ordering::Less);
}

#[perf]
fn test_natural_sort_with_numbers() {
    // Basic number ordering
    assert_eq!(natural_sort("file1", "file2"), Ordering::Less);
    assert_eq!(natural_sort("file2", "file10"), Ordering::Less);
    assert_eq!(natural_sort("file10", "file2"), Ordering::Greater);

    // Numbers in different positions
    assert_eq!(natural_sort("1file", "2file"), Ordering::Less);
    assert_eq!(natural_sort("file1text", "file2text"), Ordering::Less);
    assert_eq!(natural_sort("text1file", "text2file"), Ordering::Less);

    // Multiple numbers in string
    assert_eq!(natural_sort("file1-2", "file1-10"), Ordering::Less);
    assert_eq!(natural_sort("2-1file", "10-1file"), Ordering::Less);

    // Leading zeros
    assert_eq!(natural_sort("file002", "file2"), Ordering::Greater);
    assert_eq!(natural_sort("file002", "file10"), Ordering::Less);

    // Very large numbers
    assert_eq!(
        natural_sort("file999999999999999999999", "file999999999999999999998"),
        Ordering::Greater
    );

    // u128 edge cases

    // Numbers near u128::MAX (340,282,366,920,938,463,463,374,607,431,768,211,455)
    assert_eq!(
        natural_sort(
            "file340282366920938463463374607431768211454",
            "file340282366920938463463374607431768211455"
        ),
        Ordering::Less
    );

    // Equal length numbers that overflow u128
    assert_eq!(
        natural_sort(
            "file340282366920938463463374607431768211456",
            "file340282366920938463463374607431768211455"
        ),
        Ordering::Greater
    );

    // Different length numbers that overflow u128
    assert_eq!(
        natural_sort(
            "file3402823669209384634633746074317682114560",
            "file340282366920938463463374607431768211455"
        ),
        Ordering::Greater
    );

    // Leading zeros with numbers near u128::MAX
    assert_eq!(
        natural_sort(
            "file0340282366920938463463374607431768211455",
            "file340282366920938463463374607431768211455"
        ),
        Ordering::Greater
    );

    // Very large numbers with different lengths (both overflow u128)
    assert_eq!(
        natural_sort(
            "file999999999999999999999999999999999999999999999999",
            "file9999999999999999999999999999999999999999999999999"
        ),
        Ordering::Less
    );
}

#[perf]
fn test_natural_sort_case_sensitive() {
    // Numerically smaller values come first.
    assert_eq!(natural_sort("File1", "file2"), Ordering::Less);
    assert_eq!(natural_sort("file1", "File2"), Ordering::Less);

    // Numerically equal values: the case-insensitive comparison decides first.
    // Case-sensitive comparison only occurs when both are equal case-insensitively.
    assert_eq!(natural_sort("Dir1", "dir01"), Ordering::Less);
    assert_eq!(natural_sort("dir2", "Dir02"), Ordering::Less);
    assert_eq!(natural_sort("dir2", "dir02"), Ordering::Less);

    // Numerically equal and case-insensitively equal:
    // the lexicographically smaller (case-sensitive) one wins.
    assert_eq!(natural_sort("dir1", "Dir1"), Ordering::Less);
    assert_eq!(natural_sort("dir02", "Dir02"), Ordering::Less);
    assert_eq!(natural_sort("dir10", "Dir10"), Ordering::Less);
}

#[perf]
fn test_natural_sort_edge_cases() {
    // Empty strings
    assert_eq!(natural_sort("", ""), Ordering::Equal);
    assert_eq!(natural_sort("", "a"), Ordering::Less);
    assert_eq!(natural_sort("a", ""), Ordering::Greater);

    // Special characters
    assert_eq!(natural_sort("file-1", "file_1"), Ordering::Less);
    assert_eq!(natural_sort("file.1", "file_1"), Ordering::Less);
    assert_eq!(natural_sort("file 1", "file_1"), Ordering::Less);

    // Unicode characters
    // 9312 vs 9313
    assert_eq!(natural_sort("file①", "file②"), Ordering::Less);
    // 9321 vs 9313
    assert_eq!(natural_sort("file⑩", "file②"), Ordering::Greater);
    // 28450 vs 23383
    assert_eq!(natural_sort("file漢", "file字"), Ordering::Greater);

    // Mixed alphanumeric with special chars
    assert_eq!(natural_sort("file-1a", "file-1b"), Ordering::Less);
    assert_eq!(natural_sort("file-1.2", "file-1.10"), Ordering::Less);
    assert_eq!(natural_sort("file-1.10", "file-1.2"), Ordering::Greater);
}

#[test]
fn test_multiple_extensions() {
    // No extensions
    let path = Path::new("/a/b/c/file_name");
    assert_eq!(path.multiple_extensions(), None);

    // Only one extension
    let path = Path::new("/a/b/c/file_name.tsx");
    assert_eq!(path.multiple_extensions(), None);

    // Stories sample extension
    let path = Path::new("/a/b/c/file_name.stories.tsx");
    assert_eq!(path.multiple_extensions(), Some("stories.tsx".to_string()));

    // Longer sample extension
    let path = Path::new("/a/b/c/long.app.tar.gz");
    assert_eq!(path.multiple_extensions(), Some("app.tar.gz".to_string()));
}

#[test]
fn test_strip_path_suffix() {
    let base = Path::new("/a/b/c/file_name");
    let suffix = Path::new("file_name");
    assert_eq!(strip_path_suffix(base, suffix), Some(Path::new("/a/b/c")));

    let base = Path::new("/a/b/c/file_name.tsx");
    let suffix = Path::new("file_name.tsx");
    assert_eq!(strip_path_suffix(base, suffix), Some(Path::new("/a/b/c")));

    let base = Path::new("/a/b/c/file_name.stories.tsx");
    let suffix = Path::new("c/file_name.stories.tsx");
    assert_eq!(strip_path_suffix(base, suffix), Some(Path::new("/a/b")));

    let base = Path::new("/a/b/c/long.app.tar.gz");
    let suffix = Path::new("b/c/long.app.tar.gz");
    assert_eq!(strip_path_suffix(base, suffix), Some(Path::new("/a")));

    let base = Path::new("/a/b/c/long.app.tar.gz");
    let suffix = Path::new("/a/b/c/long.app.tar.gz");
    assert_eq!(strip_path_suffix(base, suffix), Some(Path::new("")));

    let base = Path::new("/a/b/c/long.app.tar.gz");
    let suffix = Path::new("/a/b/c/no_match.app.tar.gz");
    assert_eq!(strip_path_suffix(base, suffix), None);

    let base = Path::new("/a/b/c/long.app.tar.gz");
    let suffix = Path::new("app.tar.gz");
    assert_eq!(strip_path_suffix(base, suffix), None);
}

#[test]
fn test_strip_prefix() {
    let expected = [
        (
            PathStyle::Posix,
            "/a/b/c",
            "/a/b",
            Some(rel_path("c").into_arc()),
        ),
        (
            PathStyle::Posix,
            "/a/b/c",
            "/a/b/",
            Some(rel_path("c").into_arc()),
        ),
        (
            PathStyle::Posix,
            "/a/b/c",
            "/",
            Some(rel_path("a/b/c").into_arc()),
        ),
        (PathStyle::Posix, "/a/b/c", "", None),
        (PathStyle::Posix, "/a/b//c", "/a/b/", None),
        (PathStyle::Posix, "/a/bc", "/a/b", None),
        (
            PathStyle::Posix,
            "/a/b/c",
            "/a/b/c",
            Some(rel_path("").into_arc()),
        ),
        (
            PathStyle::Windows,
            "C:\\a\\b\\c",
            "C:\\a\\b",
            Some(rel_path("c").into_arc()),
        ),
        (
            PathStyle::Windows,
            "C:\\a\\b\\c",
            "C:\\a\\b\\",
            Some(rel_path("c").into_arc()),
        ),
        (
            PathStyle::Windows,
            "C:\\a\\b\\c",
            "C:\\",
            Some(rel_path("a/b/c").into_arc()),
        ),
        (PathStyle::Windows, "C:\\a\\b\\c", "", None),
        (PathStyle::Windows, "C:\\a\\b\\\\c", "C:\\a\\b\\", None),
        (PathStyle::Windows, "C:\\a\\bc", "C:\\a\\b", None),
        (
            PathStyle::Windows,
            "C:\\a\\b/c",
            "C:\\a\\b",
            Some(rel_path("c").into_arc()),
        ),
        (
            PathStyle::Windows,
            "C:\\a\\b/c",
            "C:\\a\\b\\",
            Some(rel_path("c").into_arc()),
        ),
        (
            PathStyle::Windows,
            "C:\\a\\b/c",
            "C:\\a\\b/",
            Some(rel_path("c").into_arc()),
        ),
    ];
    let actual = expected.clone().map(|(style, child, parent, _)| {
        (
            style,
            child,
            parent,
            style
                .strip_prefix(child.as_ref(), parent.as_ref())
                .map(|rel_path| rel_path.into_arc()),
        )
    });
    pretty_assertions::assert_eq!(actual, expected);
}

#[cfg(target_os = "windows")]
#[test]
fn test_wsl_path() {
    use super::WslPath;
    let path = "/a/b/c";
    assert_eq!(WslPath::from_path(&path), None);

    let path = r"\\wsl.localhost";
    assert_eq!(WslPath::from_path(&path), None);

    let path = r"\\wsl.localhost\Distro";
    assert_eq!(
        WslPath::from_path(&path),
        Some(WslPath {
            distro: "Distro".to_owned(),
            path: "/".into(),
        })
    );

    let path = r"\\wsl.localhost\Distro\blue";
    assert_eq!(
        WslPath::from_path(&path),
        Some(WslPath {
            distro: "Distro".to_owned(),
            path: "/blue".into()
        })
    );

    let path = r"\\wsl$\archlinux\tomato\.\paprika\..\aubergine.txt";
    assert_eq!(
        WslPath::from_path(&path),
        Some(WslPath {
            distro: "archlinux".to_owned(),
            path: "/tomato/paprika/../aubergine.txt".into()
        })
    );

    let path = r"\\windows.localhost\Distro\foo";
    assert_eq!(WslPath::from_path(&path), None);
}

#[test]
fn test_url_to_file_path_ext_posix_basic() {
    use super::UrlExt;

    let url = url::Url::parse("file:///home/user/file.txt").unwrap();
    assert_eq!(
        url.to_file_path_ext(PathStyle::Posix),
        Ok(PathBuf::from("/home/user/file.txt"))
    );

    let url = url::Url::parse("file:///").unwrap();
    assert_eq!(
        url.to_file_path_ext(PathStyle::Posix),
        Ok(PathBuf::from("/"))
    );

    let url = url::Url::parse("file:///a/b/c/d/e").unwrap();
    assert_eq!(
        url.to_file_path_ext(PathStyle::Posix),
        Ok(PathBuf::from("/a/b/c/d/e"))
    );
}

#[test]
fn test_url_to_file_path_ext_posix_percent_encoding() {
    use super::UrlExt;

    let url = url::Url::parse("file:///home/user/file%20with%20spaces.txt").unwrap();
    assert_eq!(
        url.to_file_path_ext(PathStyle::Posix),
        Ok(PathBuf::from("/home/user/file with spaces.txt"))
    );

    let url = url::Url::parse("file:///path%2Fwith%2Fencoded%2Fslashes").unwrap();
    assert_eq!(
        url.to_file_path_ext(PathStyle::Posix),
        Ok(PathBuf::from("/path/with/encoded/slashes"))
    );

    let url = url::Url::parse("file:///special%23chars%3F.txt").unwrap();
    assert_eq!(
        url.to_file_path_ext(PathStyle::Posix),
        Ok(PathBuf::from("/special#chars?.txt"))
    );
}

#[test]
fn test_url_to_file_path_ext_posix_localhost() {
    use super::UrlExt;

    let url = url::Url::parse("file://localhost/home/user/file.txt").unwrap();
    assert_eq!(
        url.to_file_path_ext(PathStyle::Posix),
        Ok(PathBuf::from("/home/user/file.txt"))
    );
}

#[test]
fn test_url_to_file_path_ext_posix_rejects_host() {
    use super::UrlExt;

    let url = url::Url::parse("file://somehost/home/user/file.txt").unwrap();
    assert_eq!(url.to_file_path_ext(PathStyle::Posix), Err(()));
}

#[test]
fn test_url_to_file_path_ext_posix_windows_drive_letter() {
    use super::UrlExt;

    let url = url::Url::parse("file:///C:").unwrap();
    assert_eq!(
        url.to_file_path_ext(PathStyle::Posix),
        Ok(PathBuf::from("/C:/"))
    );

    let url = url::Url::parse("file:///D|").unwrap();
    assert_eq!(
        url.to_file_path_ext(PathStyle::Posix),
        Ok(PathBuf::from("/D|/"))
    );
}

#[test]
fn test_url_to_file_path_ext_windows_basic() {
    use super::UrlExt;

    let url = url::Url::parse("file:///C:/Users/user/file.txt").unwrap();
    assert_eq!(
        url.to_file_path_ext(PathStyle::Windows),
        Ok(PathBuf::from("C:\\Users\\user\\file.txt"))
    );

    let url = url::Url::parse("file:///D:/folder/subfolder/file.rs").unwrap();
    assert_eq!(
        url.to_file_path_ext(PathStyle::Windows),
        Ok(PathBuf::from("D:\\folder\\subfolder\\file.rs"))
    );

    let url = url::Url::parse("file:///C:/").unwrap();
    assert_eq!(
        url.to_file_path_ext(PathStyle::Windows),
        Ok(PathBuf::from("C:\\"))
    );
}

#[test]
fn test_url_to_file_path_ext_windows_encoded_drive_letter() {
    use super::UrlExt;

    let url = url::Url::parse("file:///C%3A/Users/file.txt").unwrap();
    assert_eq!(
        url.to_file_path_ext(PathStyle::Windows),
        Ok(PathBuf::from("C:\\Users\\file.txt"))
    );

    let url = url::Url::parse("file:///c%3a/Users/file.txt").unwrap();
    assert_eq!(
        url.to_file_path_ext(PathStyle::Windows),
        Ok(PathBuf::from("c:\\Users\\file.txt"))
    );

    let url = url::Url::parse("file:///D%3A/folder/file.txt").unwrap();
    assert_eq!(
        url.to_file_path_ext(PathStyle::Windows),
        Ok(PathBuf::from("D:\\folder\\file.txt"))
    );

    let url = url::Url::parse("file:///d%3A/folder/file.txt").unwrap();
    assert_eq!(
        url.to_file_path_ext(PathStyle::Windows),
        Ok(PathBuf::from("d:\\folder\\file.txt"))
    );
}

#[test]
fn test_url_to_file_path_ext_windows_unc_path() {
    use super::UrlExt;

    let url = url::Url::parse("file://server/share/path/file.txt").unwrap();
    assert_eq!(
        url.to_file_path_ext(PathStyle::Windows),
        Ok(PathBuf::from("\\\\server\\share\\path\\file.txt"))
    );

    let url = url::Url::parse("file://server/share").unwrap();
    assert_eq!(
        url.to_file_path_ext(PathStyle::Windows),
        Ok(PathBuf::from("\\\\server\\share"))
    );
}

#[test]
fn test_url_to_file_path_ext_windows_percent_encoding() {
    use super::UrlExt;

    let url = url::Url::parse("file:///C:/Users/user/file%20with%20spaces.txt").unwrap();
    assert_eq!(
        url.to_file_path_ext(PathStyle::Windows),
        Ok(PathBuf::from("C:\\Users\\user\\file with spaces.txt"))
    );

    let url = url::Url::parse("file:///C:/special%23chars%3F.txt").unwrap();
    assert_eq!(
        url.to_file_path_ext(PathStyle::Windows),
        Ok(PathBuf::from("C:\\special#chars?.txt"))
    );
}

#[test]
fn test_url_to_file_path_ext_windows_invalid_drive() {
    use super::UrlExt;

    let url = url::Url::parse("file:///1:/path/file.txt").unwrap();
    assert_eq!(url.to_file_path_ext(PathStyle::Windows), Err(()));

    let url = url::Url::parse("file:///CC:/path/file.txt").unwrap();
    assert_eq!(url.to_file_path_ext(PathStyle::Windows), Err(()));

    let url = url::Url::parse("file:///C/path/file.txt").unwrap();
    assert_eq!(url.to_file_path_ext(PathStyle::Windows), Err(()));

    let url = url::Url::parse("file:///invalid").unwrap();
    assert_eq!(url.to_file_path_ext(PathStyle::Windows), Err(()));
}

#[test]
fn test_url_to_file_path_ext_non_file_scheme() {
    use super::UrlExt;

    let url = url::Url::parse("http://example.com/path").unwrap();
    assert_eq!(url.to_file_path_ext(PathStyle::Posix), Err(()));
    assert_eq!(url.to_file_path_ext(PathStyle::Windows), Err(()));

    let url = url::Url::parse("https://example.com/path").unwrap();
    assert_eq!(url.to_file_path_ext(PathStyle::Posix), Err(()));
    assert_eq!(url.to_file_path_ext(PathStyle::Windows), Err(()));
}

#[test]
fn test_url_to_file_path_ext_windows_localhost() {
    use super::UrlExt;

    let url = url::Url::parse("file://localhost/C:/Users/file.txt").unwrap();
    assert_eq!(
        url.to_file_path_ext(PathStyle::Windows),
        Ok(PathBuf::from("C:\\Users\\file.txt"))
    );
}
