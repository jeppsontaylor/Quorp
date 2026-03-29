//! Default TCP port for the flash-moe `infer --serve` OpenAI-compatible HTTP API (`/v1`).
//! Shared by `quorp-tui` and the SSD-MOE language-model provider so both can attach to the same
//! server without conflicting defaults.

pub const DEFAULT_INFER_SERVE_PORT: u16 = 8080;
