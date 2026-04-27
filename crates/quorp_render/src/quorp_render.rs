//! Brilliant CLI renderer — custom ANSI on top of crossterm, scrollback-
//! first, with an oscillating shimmer for active turns.
//!
//! Phase 9 ships the pure rendering primitives — palette, shimmer
//! gradient, capability detection, status footer formatting, splash
//! checklist — as deterministic functions that produce styled byte
//! sequences. The interactive event loop binds them to `crossterm` and
//! the runtime in the wire-up phase.
//!
//! All rendering is unit-testable: nothing here directly touches stdout.

pub mod caps;
pub mod logo;
pub mod palette;
pub mod permission_modal;
pub mod session;
pub mod shell;
pub mod shimmer;
pub mod splash;
pub mod status_footer;
pub mod transcript;

pub use caps::{ColorCapability, GraphicsCapability, RenderProfile};
pub use logo::{LogoMode, LogoRenderOptions, render_logo};
pub use palette::{Rgb, lerp_rgb};
pub use permission_modal::{PermissionPrompt, render_permission_modal};
pub use session::{
    CommandCard, CommandState, SessionFrame, TaskRow, TaskState, render_command_card,
    render_session_frame,
};
pub use shell::{
    ComposerView, LiveTurn, PaletteRow, ShellFrame, ShellOverlay, StatusLine, ToolStatus,
    TranscriptItem, render_shell_frame,
};
pub use shimmer::{ShimmerStyle, render_shimmer};
pub use splash::{SplashStep, render_splash};
pub use status_footer::{StatusFooter, render_status_footer};
