#![allow(unused)]
use ratatui::style::Color;

pub const fn hex(v: u32) -> Color {
    Color::Rgb(
        ((v >> 16) & 0xff) as u8,
        ((v >> 8) & 0xff) as u8,
        (v & 0xff) as u8,
    )
}

#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub titlebar_bg: Color,
    pub activity_bg: Color,
    pub sidebar_bg: Color,
    pub editor_bg: Color,
    pub tab_active_bg: Color,
    pub tab_inactive_bg: Color,
    pub pill_bg: Color,
    pub inset_bg: Color,
    pub raised_bg: Color,
    pub divider_bg: Color,
    pub subtle_border: Color,
    pub border_hover: Color,
    pub input_border: Color,
    pub info_banner_bg: Color,
    pub banner_bg: Color,
    pub toolbar_button_bg: Color,
    pub status_blue: Color,
    pub status_gold: Color,

    pub text: Color,
    pub text_primary: Color,
    pub text_muted: Color,
    pub text_faint: Color,
    pub icon_active: Color,
    pub icon_inactive: Color,

    pub accent_blue: Color,
    pub link_blue: Color,
    pub success_green: Color,
    pub warning_yellow: Color,
    pub danger_orange: Color,

    pub folder_blue: Color,
    pub file_orange: Color,
    pub file_rust: Color,
    pub file_ts_js: Color,
    pub file_py: Color,
    pub file_doc: Color,
    pub file_cfg: Color,
    pub file_shell: Color,
    pub git_modified: Color,

    pub row_selected_bg: Color,
    pub row_hover_bg: Color,
    pub scrollbar_track: Color,
    pub scrollbar_thumb: Color,
    pub scrollbar_thumb_hi: Color,
    pub code_block_bg: Color,
    pub inline_code_bg: Color,

    pub explorer_accent: Color,
    pub editor_accent: Color,
    pub terminal_accent: Color,
    pub chat_accent: Color,
    pub drag_accent: Color,
    pub secondary_teal: Color,
}

#[derive(Clone, Copy, Debug)]
pub struct Metrics {
    pub activity_bar_width: u16,
    pub default_explorer_width: u16,
    pub default_assistant_width: u16,
    pub default_terminal_height: u16,
    pub title_height: u16,
    pub header_height: u16,
    pub composer_height: u16,
    pub banner_height: u16,
    pub scrollbar_width: u16,
    pub content_pad_x: u16,
}

#[derive(Clone, Copy, Debug)]
pub struct Glyphs {
    pub chevron_right: &'static str,
    pub chevron_down: &'static str,
    pub folder_icon: &'static str,
    pub file_icon: &'static str,
    pub close_icon: &'static str,
    pub activity_files: &'static str,
    pub activity_search: &'static str,
    pub activity_settings: &'static str,
    pub activity_agent: &'static str,
}

#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub palette: Palette,
    pub metrics: Metrics,
    pub glyphs: Glyphs,
}

impl Theme {
    pub fn antigravity() -> Self {
        Self {
            palette: Palette {
                titlebar_bg: hex(0x3C3C3C),
                activity_bg: hex(0x333333),
                sidebar_bg: hex(0x252526),
                editor_bg: hex(0x1E1E1E),
                tab_active_bg: hex(0x1E1E1E),
                tab_inactive_bg: hex(0x2D2D2D),
                pill_bg: hex(0x313232),
                inset_bg: hex(0x2C2D2F),
                raised_bg: hex(0x2F3033),
                divider_bg: hex(0x232324),
                subtle_border: hex(0x28282A),
                border_hover: hex(0x3A3A3A),
                input_border: hex(0x333438),
                info_banner_bg: hex(0x03395E),
                banner_bg: hex(0x03395E),
                toolbar_button_bg: hex(0x0F649C),
                status_blue: hex(0x007ACC),
                status_gold: hex(0x7B6400),

                text: hex(0xC6C7C6),
                text_primary: hex(0xCCCCCC),
                text_muted: hex(0x8D918F),
                text_faint: hex(0x767878),
                icon_active: hex(0xF7F9FA),
                icon_inactive: hex(0xB7B7B6),

                accent_blue: hex(0x3478C6),
                link_blue: hex(0x6EA1D2),
                success_green: hex(0x58B53E),
                warning_yellow: hex(0xCCA700),
                danger_orange: hex(0xCA6644),

                folder_blue: hex(0x569CD6),
                file_orange: hex(0xCA6644),
                file_rust: hex(0xDE784C),
                file_ts_js: hex(0x4FC1FF),
                file_py: hex(0x4EC9B0),
                file_doc: hex(0xCEBA84),
                file_cfg: hex(0x9CDCAA),
                file_shell: hex(0xB987D8),
                git_modified: hex(0xCFB58C),

                row_selected_bg: hex(0x313232),
                row_hover_bg: hex(0x2A2B2D),
                scrollbar_track: hex(0x252526),
                scrollbar_thumb: hex(0x474747),
                scrollbar_thumb_hi: hex(0x5A5A5A),
                code_block_bg: hex(0x1E1E1E),
                inline_code_bg: hex(0x313232),

                explorer_accent: hex(0x569CD6),
                editor_accent: hex(0x3478C6),
                terminal_accent: hex(0xCCA700),
                chat_accent: hex(0x4FC1FF),
                drag_accent: hex(0xBC3FBC),
                secondary_teal: hex(0x4EC9B0),
            },
            metrics: Metrics {
                activity_bar_width: 5,
                default_explorer_width: 23,
                default_assistant_width: 130,
                default_terminal_height: 47,
                title_height: 2,
                header_height: 2,
                composer_height: 4,
                banner_height: 2,
                scrollbar_width: 1,
                content_pad_x: 2,
            },
            glyphs: Glyphs {
                chevron_right: "›",
                chevron_down: "⌄",
                folder_icon: "󰉋",
                file_icon: "󰌛",
                close_icon: "×",
                activity_files: "☰",
                activity_search: "⌕",
                activity_settings: "⚙",
                activity_agent: "◈",
            },
        }
    }
    pub fn prism_forge() -> Self {
        Self {
            palette: Palette {
                titlebar_bg: hex(0x0B0F14),
                activity_bg: hex(0x111723),
                sidebar_bg: hex(0x111723),
                editor_bg: hex(0x1B2433),
                tab_active_bg: hex(0x171E2B),
                tab_inactive_bg: hex(0x171E2B),
                pill_bg: hex(0x171E2B),
                inset_bg: hex(0x111723),
                raised_bg: hex(0x171E2B),
                divider_bg: hex(0x273042),
                subtle_border: hex(0x273042),
                border_hover: hex(0x334059),
                input_border: hex(0x35D7FF),
                info_banner_bg: hex(0x131C29),
                banner_bg: hex(0x131C29),
                toolbar_button_bg: hex(0x273042),
                status_blue: hex(0x007ACC),
                status_gold: hex(0xFFB224),

                text: hex(0xEAF2FF),
                text_primary: hex(0xEAF2FF),
                text_muted: hex(0x9FB0C8),
                text_faint: hex(0x67778E),
                icon_active: hex(0xEAF2FF),
                icon_inactive: hex(0x67778E),

                accent_blue: hex(0x52A7FF),
                link_blue: hex(0x52A7FF),
                success_green: hex(0xA7F432),
                warning_yellow: hex(0xFFB224),
                danger_orange: hex(0xFF6B6B),

                folder_blue: hex(0x34E7A5),
                file_orange: hex(0xFFB224),
                file_rust: hex(0xDE784C),
                file_ts_js: hex(0x35D7FF),
                file_py: hex(0x00D7C9),
                file_doc: hex(0x00D7C9),
                file_cfg: hex(0x9FB0C8),
                file_shell: hex(0x8B5CF6),
                git_modified: hex(0xFFB224),

                row_selected_bg: hex(0x171E2B),
                row_hover_bg: hex(0x1B2433),
                scrollbar_track: hex(0x111723),
                scrollbar_thumb: hex(0x334059),
                scrollbar_thumb_hi: hex(0x273042),
                code_block_bg: hex(0x0B0F14),
                inline_code_bg: hex(0x171E2B),

                explorer_accent: hex(0x34E7A5),
                editor_accent: hex(0x8B5CF6),
                terminal_accent: hex(0xFFB224),
                chat_accent: hex(0x35D7FF),
                drag_accent: hex(0xFF4FD8),
                secondary_teal: hex(0x00D7C9),
            },
            metrics: Metrics {
                activity_bar_width: 4,
                default_explorer_width: 30,
                default_assistant_width: 48,
                default_terminal_height: 12,
                title_height: 1,
                header_height: 2,
                composer_height: 4,
                banner_height: 2,
                scrollbar_width: 1,
                content_pad_x: 1,
            },
            glyphs: Glyphs {
                chevron_right: "›",
                chevron_down: "⌄",
                folder_icon: "󰉋",
                file_icon: "󰌛",
                close_icon: "×",
                activity_files: "☰",
                activity_search: "⌕",
                activity_settings: "⚙",
                activity_agent: "◈",
            },
        }
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    fn color_u24(color: Color) -> u32 {
        match color {
            Color::Rgb(red, green, blue) => {
                ((red as u32) << 16) | ((green as u32) << 8) | (blue as u32)
            }
            _ => panic!("expected Rgb"),
        }
    }

    #[test]
    fn prism_forge_mock1_role_accents() {
        let palette = Theme::prism_forge().palette;
        assert_eq!(color_u24(palette.explorer_accent), 0x34E7A5);
        assert_eq!(color_u24(palette.editor_accent), 0x8B5CF6);
        assert_eq!(color_u24(palette.terminal_accent), 0xFFB224);
        assert_eq!(color_u24(palette.chat_accent), 0x35D7FF);
        assert_eq!(color_u24(palette.drag_accent), 0xFF4FD8);
        assert_eq!(color_u24(palette.success_green), 0xA7F432);
        assert_eq!(color_u24(palette.danger_orange), 0xFF6B6B);
        assert_eq!(color_u24(palette.accent_blue), 0x52A7FF);
        assert_eq!(color_u24(palette.secondary_teal), 0x00D7C9);
    }

    #[test]
    fn prism_forge_mock1_surfaces() {
        let palette = Theme::prism_forge().palette;
        assert_eq!(color_u24(palette.titlebar_bg), 0x0B0F14);
        assert_eq!(color_u24(palette.sidebar_bg), 0x111723);
        assert_eq!(color_u24(palette.divider_bg), 0x273042);
        assert_eq!(color_u24(palette.border_hover), 0x334059);
    }

    #[test]
    fn prism_forge_mock1_metrics() {
        let metrics = Theme::prism_forge().metrics;
        assert_eq!(metrics.activity_bar_width, 4);
        assert_eq!(metrics.default_explorer_width, 30);
        assert_eq!(metrics.default_assistant_width, 48);
        assert_eq!(metrics.default_terminal_height, 12);
        assert_eq!(metrics.title_height, 1);
        assert_eq!(metrics.header_height, 2);
        assert_eq!(metrics.composer_height, 4);
        assert_eq!(metrics.banner_height, 2);
        assert_eq!(metrics.scrollbar_width, 1);
        assert_eq!(metrics.content_pad_x, 1);
    }

    /// Regression guard for “TUI sidebar / explorer” color: tree uses emerald-aligned `folder_blue`.
    #[test]
    fn prism_forge_sidebar_explorer_tint_matches_mock1() {
        let palette = Theme::prism_forge().palette;
        assert_eq!(color_u24(palette.folder_blue), 0x34E7A5);
        assert_eq!(color_u24(palette.sidebar_bg), 0x111723);
    }
}

