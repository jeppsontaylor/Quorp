# Offline Rust builds and tests

This repository includes an offline path for restricted environments that cannot fetch from crates.io or git.

## 1) Bootstrap vendor directory (networked environment)

Run once in an environment that can access crates.io:

```bash
./script/bootstrap-vendor
```

This creates a `vendor/` directory with all locked dependencies.

## 2) Run cargo offline (restricted environment)

Use the offline wrapper:

```bash
./script/cargo-offline test -p quorp tui_flow_tests -- --nocapture
```

The wrapper enables:

- `--offline`
- source replacement via `.cargo/config.offline-vendor.toml`

## 3) Notes

- If `vendor/` is missing, `script/cargo-offline` exits with a clear error.
- Re-run `./script/bootstrap-vendor` whenever `Cargo.lock` changes.
