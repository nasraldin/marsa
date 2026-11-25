//! Resolve GPUI Component control states from the Figma semantic palette.

use gpui::{App, Hsla, px};
use gpui_component::{Theme, ThemeMode};
use marsa_core::settings::DisplaySettings;
use marsa_l10n::Locale;

use super::{
    ColorToken, Palette,
    typography::{Family, Role, rem_size},
};

impl Palette {
    /// Synchronize component surfaces, focus indicators and fonts with the workspace.
    pub fn apply_to_components(self, locale: Locale, display: DisplaySettings, cx: &mut App) {
        use ColorToken::*;

        if Theme::global(cx).is_dark() != self.is_dark() {
            let mode = if self.is_dark() {
                ThemeMode::Dark
            } else {
                ThemeMode::Light
            };
            Theme::change(mode, None, cx);
        }
        Theme::set_scrollbar_mode(gpui_component::scroll::ScrollbarMode::Always, cx);

        let c = |token| -> Hsla { self.color(token).into() };
        Theme::update(cx, |theme| {
            let colors = &mut theme.colors;
            colors.background = c(BgCanvas);
            colors.foreground = c(TextPrimary);
            colors.border = c(BorderSubtle);
            colors.input = c(BorderControl);
            colors.caret = c(AccentDefault);
            colors.muted = c(BgSubtle);
            colors.muted_foreground = c(TextMuted);
            colors.popover = c(BgElevated);
            colors.popover_foreground = c(TextPrimary);
            colors.accent = c(BgHover);
            colors.accent_foreground = c(TextPrimary);
            colors.primary = c(AccentDefault);
            colors.primary_active = c(AccentPressed);
            colors.primary_foreground = c(TextOnAccent);
            colors.primary_hover = c(AccentHover);
            colors.button_primary = colors.primary;
            colors.button_primary_active = colors.primary_active;
            colors.button_primary_foreground = colors.primary_foreground;
            colors.button_primary_hover = colors.primary_hover;
            colors.button = c(GlassControl);
            colors.button_active = c(BgPressed);
            colors.button_foreground = c(TextPrimary);
            colors.button_hover = c(BgHover);
            colors.button_danger = c(StatusErrorBg);
            colors.button_danger_foreground = c(StatusError);
            colors.button_danger_hover = c(StatusErrorBg);
            colors.button_danger_active = c(BgPressed);
            colors.secondary = c(GlassControl);
            colors.secondary_active = c(BgPressed);
            colors.secondary_foreground = c(TextPrimary);
            colors.secondary_hover = c(BgHover);
            colors.button_secondary = colors.secondary;
            colors.button_secondary_active = colors.secondary_active;
            colors.button_secondary_foreground = colors.secondary_foreground;
            colors.button_secondary_hover = colors.secondary_hover;
            colors.link = colors.primary;
            colors.link_hover = colors.primary_hover;
            colors.link_active = colors.primary_active;
            colors.ring = c(AccentFocus);
            colors.selection = c(AccentSubtle);
            colors.list = c(BgElevated);
            colors.list_active = c(AccentSubtle);
            colors.list_active_border = c(AccentFocus);
            colors.list_even = c(BgSurface);
            colors.list_head = c(BgSubtle);
            colors.list_hover = c(BgHover);
            colors.list_highlight_foreground = c(AccentDefault);
            colors.table = c(BgElevated);
            colors.table_active = c(AccentSubtle);
            colors.table_active_border = c(AccentFocus);
            colors.table_even = c(BgSurface);
            colors.table_head = c(BgSubtle);
            colors.table_head_foreground = c(TextMuted);
            colors.table_foot = c(BgSubtle);
            colors.table_foot_foreground = c(TextMuted);
            colors.table_hover = c(BgHover);
            colors.table_row_border = c(BorderSubtle);
            colors.progress_bar = c(AccentDefault);
            colors.tab_active = c(BgElevated);
            colors.tab_active_foreground = c(TextPrimary);
            colors.tab_bar = c(BgSubtle);
            colors.tab_bar_segmented = c(BgSubtle);
            colors.tab_foreground = c(TextMuted);
            colors.sidebar = c(GlassRail);
            colors.sidebar_border = c(GlassEdge);
            colors.sidebar_foreground = c(TextMuted);
            colors.sidebar_accent = c(AccentSubtle);
            colors.sidebar_accent_foreground = c(AccentDefault);
            colors.chart_1 = c(ChartCpu);
            colors.chart_2 = c(ChartMemory);
            colors.chart_3 = c(ChartNetwork);
            colors.chart_4 = c(ChartDisk);
            colors.chart_grid = c(BorderSubtle);
            theme.font_family = Role::Body
                .with_ui_font(locale == Locale::Arabic, display.ui_font)
                .family
                .name()
                .into();
            theme.font_size = px(rem_size(display.ui_font_size));
            theme.mono_font_family = Family::Mono.name().into();
        });
    }
}
