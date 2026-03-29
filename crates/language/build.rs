fn main() {
    if let Ok(bundled) = std::env::var("QUORP_BUNDLE") {
        println!("cargo:rustc-env=QUORP_BUNDLE={}", bundled);
    }
}
