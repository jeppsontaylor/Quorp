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
    pub canvas_bg: Color,
    pub panel_bg: Color,
    pub terminal_bg: Color,
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
    pub runtime_online: Color,
    pub runtime_online_hi: Color,
    pub runtime_transition: Color,
    pub runtime_offline: Color,
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
    pub grid_line: Color,
    pub grid_line_focus: Color,
    pub code_block_bg: Color,
    pub inline_code_bg: Color,
    pub terminal_title_fg: Color,
    pub terminal_path_fg: Color,
    pub terminal_prompt_fg: Color,

    pub explorer_accent: Color,
    pub editor_accent: Color,
    pub terminal_accent: Color,
    pub chat_accent: Color,
    pub drag_accent: Color,
    pub secondary_teal: Color,

    pub diff_add_bg: Color,
    pub diff_add_fg: Color,
    pub diff_remove_bg: Color,
    pub diff_remove_fg: Color,

    pub tool_search: Color,
    pub tool_plan: Color,
    pub tool_edit: Color,
    pub tool_verify: Color,
    pub tool_git: Color,
    pub tool_risk: Color,
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
    pub fn session_default() -> Self {
        Self::void_neon()
    }

    pub fn core_tui() -> Self {
        Self {
            palette: Palette {
                canvas_bg: hex(0x0E110F),
                panel_bg: hex(0x121714),
                terminal_bg: hex(0x000000),
                titlebar_bg: hex(0x08100E),
                activity_bg: hex(0x141712),
                sidebar_bg: hex(0x2A2A22),
                editor_bg: hex(0x121714),
                tab_active_bg: hex(0x1D221D),
                tab_inactive_bg: hex(0x171B17),
                pill_bg: hex(0x1B201C),
                inset_bg: hex(0x0A0F0C),
                raised_bg: hex(0x1A1E1A),
                divider_bg: hex(0x31372F),
                subtle_border: hex(0x394139),
                border_hover: hex(0x61DFFF),
                input_border: hex(0x28DFFF),
                info_banner_bg: hex(0x18201A),
                banner_bg: hex(0x18201A),
                toolbar_button_bg: hex(0x232823),
                status_blue: hex(0x22D3EE),
                status_gold: hex(0xFFB938),

                text: hex(0xE7ECE1),
                text_primary: hex(0xF6F7F1),
                text_muted: hex(0xC0C7B9),
                text_faint: hex(0x929A8A),
                icon_active: hex(0xF0F2EA),
                icon_inactive: hex(0x9DA495),

                accent_blue: hex(0x33C8FF),
                link_blue: hex(0x7FE7FF),
                success_green: hex(0x69FF61),
                runtime_online: hex(0x58FF4A),
                runtime_online_hi: hex(0xC5FF38),
                runtime_transition: hex(0xFFC857),
                runtime_offline: hex(0xFF7A67),
                warning_yellow: hex(0xFFC857),
                danger_orange: hex(0xFF7A67),

                folder_blue: hex(0x7EC4FF),
                file_orange: hex(0xF6C177),
                file_rust: hex(0xFF9E64),
                file_ts_js: hex(0x4DE3FF),
                file_py: hex(0x7EFF7A),
                file_doc: hex(0xC6A0F6),
                file_cfg: hex(0x9CCFD8),
                file_shell: hex(0xE0AF68),
                git_modified: hex(0xF6C177),

                row_selected_bg: hex(0x34362A),
                row_hover_bg: hex(0x262A24),
                scrollbar_track: hex(0x1C201B),
                scrollbar_thumb: hex(0x73D9FF),
                scrollbar_thumb_hi: hex(0xD3F7FF),
                grid_line: hex(0x353B35),
                grid_line_focus: hex(0x5EE6FF),
                code_block_bg: hex(0x070907),
                inline_code_bg: hex(0x1A1F1B),
                terminal_title_fg: hex(0xF6F7F1),
                terminal_path_fg: hex(0x51C7FF),
                terminal_prompt_fg: hex(0x66FF52),

                explorer_accent: hex(0x7FE7FF),
                editor_accent: hex(0x28DFFF),
                terminal_accent: hex(0xFFC857),
                chat_accent: hex(0x20E4FF),
                drag_accent: hex(0xF7768E),
                secondary_teal: hex(0x64FFD4),

                diff_add_bg: hex(0x0A1F0C),
                diff_add_fg: hex(0x69FF61),
                diff_remove_bg: hex(0x1F0A0C),
                diff_remove_fg: hex(0xFF7A67),

                tool_search: hex(0x7FE7FF),
                tool_plan: hex(0xC6A0F6),
                tool_edit: hex(0xF7768E),
                tool_verify: hex(0x69FF61),
                tool_git: hex(0x7EC4FF),
                tool_risk: hex(0xFFC857),
            },
            metrics: Metrics {
                activity_bar_width: 5,
                default_explorer_width: 23,
                default_assistant_width: 40,
                default_terminal_height: 12,
                title_height: 1,
                header_height: 2,
                composer_height: 3,
                banner_height: 1,
                scrollbar_width: 1,
                content_pad_x: 1,
            },
            glyphs: Glyphs {
                chevron_right: ">",
                chevron_down: "v",
                folder_icon: "+",
                file_icon: "-",
                close_icon: "x",
                activity_files: "F",
                activity_search: "S",
                activity_settings: "C",
                activity_agent: "A",
            },
        }
    }
    pub fn prism_forge() -> Self {
        Self {
            palette: Palette {
                canvas_bg: hex(0x101310),
                panel_bg: hex(0x151A15),
                terminal_bg: hex(0x000000),
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
                runtime_online: hex(0x5FFF72),
                runtime_online_hi: hex(0xD5FF3B),
                runtime_transition: hex(0xFFC542),
                runtime_offline: hex(0xFF5C67),
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
                scrollbar_track: hex(0x182334),
                scrollbar_thumb: hex(0x3AE0FF),
                scrollbar_thumb_hi: hex(0xA6F5FF),
                grid_line: hex(0x273042),
                grid_line_focus: hex(0x8AF0FF),
                code_block_bg: hex(0x0B0F14),
                inline_code_bg: hex(0x171E2B),
                terminal_title_fg: hex(0xEAF2FF),
                terminal_path_fg: hex(0x52A7FF),
                terminal_prompt_fg: hex(0xA7F432),

                explorer_accent: hex(0x34E7A5),
                editor_accent: hex(0x8B5CF6),
                terminal_accent: hex(0xFFB224),
                chat_accent: hex(0x35D7FF),
                drag_accent: hex(0xFF4FD8),
                secondary_teal: hex(0x00D7C9),

                diff_add_bg: hex(0x0A1A0F),
                diff_add_fg: hex(0xA7F432),
                diff_remove_bg: hex(0x1A0A0F),
                diff_remove_fg: hex(0xFF6B6B),

                tool_search: hex(0x35D7FF),
                tool_plan: hex(0x8B5CF6),
                tool_edit: hex(0xFF4FD8),
                tool_verify: hex(0xA7F432),
                tool_git: hex(0x52A7FF),
                tool_risk: hex(0xFFB224),
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
                chevron_right: ">",
                chevron_down: "v",
                folder_icon: "+",
                file_icon: "-",
                close_icon: "x",
                activity_files: "F",
                activity_search: "S",
                activity_settings: "C",
                activity_agent: "A",
            },
        }
    }

    pub fn terminal_pulse() -> Self {
        let mut theme = Self::core_tui();
        theme.palette.canvas_bg = hex(0x12070D);
        theme.palette.panel_bg = hex(0x190B12);
        theme.palette.titlebar_bg = hex(0x0F0509);
        theme.palette.activity_bg = hex(0x1A0B12);
        theme.palette.sidebar_bg = hex(0x21111A);
        theme.palette.editor_bg = hex(0x150A11);
        theme.palette.tab_active_bg = hex(0x26111B);
        theme.palette.tab_inactive_bg = hex(0x190B12);
        theme.palette.pill_bg = hex(0x2A1220);
        theme.palette.inset_bg = hex(0x0D0408);
        theme.palette.raised_bg = hex(0x26111B);
        theme.palette.divider_bg = hex(0x40202C);
        theme.palette.subtle_border = hex(0x5B2F40);
        theme.palette.border_hover = hex(0x8FF7D2);
        theme.palette.input_border = hex(0x7DF9FF);
        theme.palette.toolbar_button_bg = hex(0x301523);
        theme.palette.text = hex(0xF5E9EF);
        theme.palette.text_primary = hex(0xFFF7FA);
        theme.palette.text_muted = hex(0xD8B7C5);
        theme.palette.text_faint = hex(0xA77E90);
        theme.palette.accent_blue = hex(0x53E3FF);
        theme.palette.link_blue = hex(0x8DF3FF);
        theme.palette.success_green = hex(0x7DFF70);
        theme.palette.runtime_online = hex(0x58FF8A);
        theme.palette.runtime_online_hi = hex(0xC9FF64);
        theme.palette.runtime_transition = hex(0xFFC857);
        theme.palette.runtime_offline = hex(0xFF6E7C);
        theme.palette.warning_yellow = hex(0xFFD166);
        theme.palette.danger_orange = hex(0xFF7B72);
        theme.palette.row_selected_bg = hex(0x371726);
        theme.palette.row_hover_bg = hex(0x28111D);
        theme.palette.scrollbar_track = hex(0x18080F);
        theme.palette.scrollbar_thumb = hex(0x55E9FF);
        theme.palette.scrollbar_thumb_hi = hex(0xD7FBFF);
        theme.palette.grid_line = hex(0x4C2635);
        theme.palette.grid_line_focus = hex(0x8FF7D2);
        theme.palette.code_block_bg = hex(0x070205);
        theme.palette.inline_code_bg = hex(0x301523);
        theme.palette.terminal_title_fg = hex(0xFFF7FA);
        theme.palette.terminal_path_fg = hex(0x79F3FF);
        theme.palette.terminal_prompt_fg = hex(0x8BFF5C);
        theme.palette.explorer_accent = hex(0x8FF7D2);
        theme.palette.editor_accent = hex(0x53E3FF);
        theme.palette.terminal_accent = hex(0xFFD166);
        theme.palette.chat_accent = hex(0xFF6AD5);
        theme.palette.drag_accent = hex(0xFF7B72);
        theme.palette.secondary_teal = hex(0x8FF7D2);
        theme
    }
    pub fn void_neon() -> Self {
        let mut theme = Self::core_tui();
        // Foundations
        theme.palette.canvas_bg = hex(0x060913);
        theme.palette.panel_bg = hex(0x0D1324);
        theme.palette.titlebar_bg = hex(0x08111F);
        theme.palette.activity_bg = hex(0x060913);
        theme.palette.sidebar_bg = hex(0x0D1324);
        theme.palette.editor_bg = hex(0x060913);
        theme.palette.tab_active_bg = hex(0x121931);
        theme.palette.tab_inactive_bg = hex(0x0D1324);
        theme.palette.pill_bg = hex(0x121931);
        theme.palette.inset_bg = hex(0x050816);
        theme.palette.raised_bg = hex(0x121931);
        theme.palette.divider_bg = hex(0x1B2740);
        theme.palette.subtle_border = hex(0x26385E);
        theme.palette.border_hover = hex(0x44E6FF);
        theme.palette.input_border = hex(0x5A84FF);
        theme.palette.toolbar_button_bg = hex(0x121931);

        // Text
        theme.palette.text = hex(0xF3F7FF);
        theme.palette.text_primary = hex(0xF3F7FF);
        theme.palette.text_muted = hex(0xA3B0CF);
        theme.palette.text_faint = hex(0x6E7D9B);

        // Accents
        theme.palette.accent_blue = hex(0x5A84FF);
        theme.palette.link_blue = hex(0x44E6FF);
        theme.palette.success_green = hex(0x20F391);
        theme.palette.runtime_online = hex(0x20F391);
        theme.palette.runtime_online_hi = hex(0xC4FF4B);
        theme.palette.runtime_transition = hex(0xFFB94E);
        theme.palette.runtime_offline = hex(0xFF5075);
        theme.palette.warning_yellow = hex(0xFFDB7D);
        theme.palette.danger_orange = hex(0xFF5075);

        // Roles
        theme.palette.row_selected_bg = hex(0x121931);
        theme.palette.row_hover_bg = hex(0x121931);
        theme.palette.scrollbar_track = hex(0x08111F);
        theme.palette.scrollbar_thumb = hex(0x26385E);
        theme.palette.scrollbar_thumb_hi = hex(0x5A84FF);
        theme.palette.grid_line = hex(0x1B2740);
        theme.palette.grid_line_focus = hex(0x44E6FF);

        // Code
        theme.palette.code_block_bg = hex(0x050816);
        theme.palette.inline_code_bg = hex(0x121931);
        theme.palette.terminal_title_fg = hex(0xF3F7FF);
        theme.palette.terminal_path_fg = hex(0x5A84FF);
        theme.palette.terminal_prompt_fg = hex(0xC4FF4B);

        // Accents semantic
        theme.palette.explorer_accent = hex(0x44E6FF);
        theme.palette.editor_accent = hex(0x5A84FF);
        theme.palette.terminal_accent = hex(0xFFB94E);
        theme.palette.chat_accent = hex(0xA574FF);
        theme.palette.drag_accent = hex(0xFF4ED0);
        theme.palette.secondary_teal = hex(0x27E8F2);

        // Diff
        theme.palette.diff_add_bg = hex(0x061A12);
        theme.palette.diff_add_fg = hex(0x20F391);
        theme.palette.diff_remove_bg = hex(0x1A0614);
        theme.palette.diff_remove_fg = hex(0xFF5075);

        // Tool-type semantic colors
        theme.palette.tool_search = hex(0x44E6FF);
        theme.palette.tool_plan = hex(0xA574FF);
        theme.palette.tool_edit = hex(0xFF4ED0);
        theme.palette.tool_verify = hex(0x20F391);
        theme.palette.tool_git = hex(0x5A84FF);
        theme.palette.tool_risk = hex(0xFFDB7D);

        theme
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

    #[test]
    fn terminal_pulse_has_distinct_cli_palette() {
        let palette = Theme::terminal_pulse().palette;
        assert_eq!(color_u24(palette.canvas_bg), 0x12070D);
        assert_eq!(color_u24(palette.chat_accent), 0xFF6AD5);
        assert_eq!(color_u24(palette.runtime_online), 0x58FF8A);
    }
}
