# Quorp proof lanes. Run `just <lane>`.

# Fast: under 30s; pre-commit-grade.
fast:
    cargo fmt --all --check
    cargo check --workspace

# Medium: under ~3 min; PR-ready.
medium: fast
    cargo clippy --workspace --no-deps -- -D warnings
    cargo test --workspace --lib

# Deep: under ~15 min; nightly / pre-release.
deep: medium
    cargo test --workspace --all-features
    cargo test --workspace --doc

# Security: triggered on auth/secrets/unsafe/FFI/CI/shell/deser/path changes.
security:
    cargo audit
    gitleaks detect --source . --redact || true
    ./script/check-loc-cap 2000

# Release: full evidence pack.
release: deep security
    cargo doc --workspace --no-deps

# CI helper: enforce file-size policy (2000 hard, 800 soft).
loc-check:
    ./script/check-loc-cap 800 --warn
    ./script/check-loc-cap 2000 --error

# CI helper: deterministic scoring tests plus optional live smoke/regression score.
benchmark-gate:
    ./script/quorp-benchmark-regression-gate
