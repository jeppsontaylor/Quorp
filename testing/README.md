# Testing Layout

Actual Rust tests live under `testing/`, grouped by crate and module path.

Source files under `crates/` should only keep a declaration that points at the out-of-tree test module:

```rust
#[cfg(test)]
#[path = "../../../testing/example_crate/example_module/tests.rs"]
mod tests;
```

The expected layout for tests extracted from `crates/<crate>/src/foo/bar.rs` is:

```text
testing/<crate>/foo/bar/tests.rs
```

Large suites may keep chunk files beside their root test module, such as `tests_chunk_00.rs`, as long as those files stay under the same `testing/<crate>/<area>/` directory.

Production files may still contain `#[cfg(test)]` support hooks when alternate test behavior must compile with the crate, but they should not contain test functions or inline test modules.

Run `./script/quorp-test-layout-audit` to check the layout.
