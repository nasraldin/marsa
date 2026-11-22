//! The six accent swatches from Settings · General in Figma.

use marsa_ui::{
    ColorToken,
    gpui::{Context, Div, ParentElement, Styled, div, linear_color_stop, linear_gradient, px},
    gpui_component::{Disableable, button::Button},
    gui_keys as gui,
    preferences::AccentColor,
    tokens::accent_swatch,
};

use super::super::{SystemView, machine::title_text};
use crate::screens::common::{ScreenContext, color, panel, row};

impl SystemView {
    pub(super) fn accent_picker(&self, screen: &ScreenContext<'_>, cx: &mut Context<Self>) -> Div {
        let locale = screen.locale;
        panel(screen)
            .child(title_text(gui::GUI_ACCENT_TITLE.text(locale)))
            .child(
                div()
                    .text_xs()
                    .text_color(color(screen.palette, ColorToken::TextMuted))
                    .child(gui::GUI_ACCENT_DESCRIPTION.text(locale)),
            )
            .child(
                row(locale).gap_3().flex_wrap().children(
                    [
                        (AccentColor::MarsaBlue, gui::GUI_ACCENT_MARSA_BLUE),
                        (AccentColor::Ocean, gui::GUI_ACCENT_OCEAN),
                        (AccentColor::Fern, gui::GUI_ACCENT_FERN),
                        (AccentColor::Coral, gui::GUI_ACCENT_CORAL),
                        (AccentColor::Amber, gui::GUI_ACCENT_AMBER),
                        (AccentColor::Violet, gui::GUI_ACCENT_VIOLET),
                    ]
                    .into_iter()
                    .map(|(accent, key)| {
                        let [deep, _, tint] = accent_swatch(accent);
                        let label = key.text(locale);
                        let selected = self.accent() == accent;
                        let panel = if selected {
                            ColorToken::AccentSubtle
                        } else {
                            ColorToken::BgSurface
                        };
                        let border = if selected {
                            ColorToken::AccentFocus
                        } else {
                            ColorToken::BorderSubtle
                        };
                        Button::new(("accent", accent as usize))
                            .outline()
                            .h(px(64.))
                            .w(px(152.))
                            .rounded(px(12.))
                            .bg(color(screen.palette, panel))
                            .border_color(color(screen.palette, border))
                            .disabled(self.busy() || self.settings.is_none())
                            .accessibility_label(label)
                            .child(
                                row(locale)
                                    .w_full()
                                    .gap_2()
                                    .child(div().size(px(34.)).flex_shrink_0().rounded_full().bg(
                                        linear_gradient(
                                            125.,
                                            linear_color_stop(deep, 0.),
                                            linear_color_stop(tint, 1.),
                                        ),
                                    ))
                                    .child(div().text_sm().child(label)),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.save_settings(
                                    |settings| settings.ui.accent_color = accent,
                                    cx,
                                );
                            }))
                    }),
                ),
            )
    }
}
