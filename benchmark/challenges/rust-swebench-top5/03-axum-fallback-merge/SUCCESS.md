# Success Criteria

- `cargo test --quiet -p axum --lib routing::tests::` exits successfully.
- The fix addresses the bug described in [`START_HERE.md`](START_HERE.md) without reverting the dataset test patch.
- Changes stay focused on the owning implementation and any gold-patch documentation/config files that are needed.

## Dataset Fail-to-Pass Coverage
- `routing::tests::merging_routers_with_fallbacks_panics - should panic`
- `routing::tests::nesting_router_with_fallbacks_panics - should panic`
