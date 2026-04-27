//! Compatibility re-export. The MCP client lives in `quorp_mcp` since
//! Phase 4-A; existing callsites that reach `crate::quorp::tui::mcp_client::*`
//! continue to resolve through this shim until the binary's session/CLI
//! crates land in their own packages and can depend on `quorp_mcp` directly.
pub use quorp_mcp::*;
