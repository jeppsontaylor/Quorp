# Success Criteria

- `cargo test --quiet -p cargo-dist --test integration-tests axolotlsay_edit_existing -- --exact` exits successfully.
- The fix addresses the bug described in [`START_HERE.md`](START_HERE.md) without reverting the dataset test patch.
- Changes stay focused on the owning implementation and any gold-patch documentation/config files that are needed.

## Dataset Fail-to-Pass Coverage
- `axolotlsay_edit_existing`
