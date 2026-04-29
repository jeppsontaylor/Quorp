use super::*;
use std::fs;
use tempfile::tempdir;

#[test]
fn extension_maps_correctly() {
    assert_eq!(Language::from_extension("rs"), Language::Rust);
    assert_eq!(Language::from_extension("ts"), Language::TypeScript);
    assert_eq!(Language::from_extension("xyz"), Language::Other);
}

#[test]
fn scan_skips_target_directory() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("target")).unwrap();
    fs::write(root.join("target/big.rs"), "fn main(){}").unwrap();
    fs::write(root.join("good.rs"), "fn good(){}").unwrap();
    let scanned = scan(root);
    assert!(scanned.iter().any(|f| f.path.ends_with("good.rs")));
    assert!(
        !scanned
            .iter()
            .any(|f| f.path.to_string_lossy().contains("target"))
    );
}

#[test]
fn harvest_picks_up_top_level_symbols() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("a.rs");
    let src = "pub fn hello() {}\nstruct Foo;\nenum Bar { A }\n";
    fs::write(&path, src).unwrap();
    let file = ScannedFile {
        path,
        language: Language::Rust,
        bytes: src.len() as u64,
    };
    let symbols = harvest_rust_symbols(&file, src);
    let kinds: Vec<SymbolKind> = symbols.iter().map(|s| s.kind).collect();
    assert!(kinds.contains(&SymbolKind::Function));
    assert!(kinds.contains(&SymbolKind::Struct));
    assert!(kinds.contains(&SymbolKind::Enum));
}
