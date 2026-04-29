use super::*;

#[test]
fn copy_dir_all_skips_generated_build_directories() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let source = temp_dir.path().join("source");
    let destination = temp_dir.path().join("destination");

    fs::create_dir_all(source.join("nested")).expect("nested");
    fs::create_dir_all(source.join("target").join("debug")).expect("target");
    fs::create_dir_all(source.join(".quorp-cargo-target").join("debug")).expect("cache");
    fs::create_dir_all(source.join(".quorp-cargo-target-eval").join("debug")).expect("eval cache");
    fs::create_dir_all(source.join(".git").join("objects")).expect("git");
    fs::create_dir_all(source.join("node_modules").join("pkg")).expect("node_modules");
    fs::write(source.join("nested").join("keep.txt"), "keep").expect("keep");
    fs::write(source.join("target").join("debug").join("drop.txt"), "drop").expect("drop");
    fs::write(
        source
            .join(".quorp-cargo-target")
            .join("debug")
            .join("drop.txt"),
        "drop",
    )
    .expect("cache drop");
    fs::write(
        source
            .join(".quorp-cargo-target-eval")
            .join("debug")
            .join("drop.txt"),
        "drop",
    )
    .expect("eval drop");
    fs::write(source.join(".git").join("objects").join("drop.txt"), "drop").expect("git drop");
    fs::write(
        source.join("node_modules").join("pkg").join("drop.txt"),
        "drop",
    )
    .expect("node_modules drop");

    copy_dir_all(&source, &destination).expect("copy");

    assert!(destination.join("nested").join("keep.txt").exists());
    assert!(!destination.join("target").exists());
    assert!(!destination.join(".quorp-cargo-target").exists());
    assert!(!destination.join(".quorp-cargo-target-eval").exists());
    assert!(!destination.join(".git").exists());
    assert!(!destination.join("node_modules").exists());
}
