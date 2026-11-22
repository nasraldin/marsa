//! The six accent swatches from Settings · General in Figma.

use marsa_ui::{
    ColorToken,
    direction::LogicalStyle,
    gpui::{Context, Div, ParentElement, Styled, div, linear_color_stop, linear_gradient, px},
    gpui_component::{Disableable, Selectable, button::Button},
    gui_keys as gui,
    preferences::AccentColor,
    tokens::{
        accent_swatch,
        scale::{Radius, Space},
        typography::Role,
    },
};

use super::{super::SystemView, settings_heading, settings_icon, settings_text};
use crate::screens::common::{ScreenContext, color, panel, row};

impl SystemView {
    pub(super) fn accent_picker(&self, screen: &ScreenContext<'_>, cx: &mut Context<Self>) -> Div {
        let locale = screen.locale;
        panel(screen)
            .gap(px(Space::S16.pixels() + Space::S4.pixels()))
            .child(settings_heading(
                screen,
                gui::GUI_ACCENT_TITLE.text(locale),
                gui::GUI_ACCENT_DESCRIPTION.text(locale),
            ))
            .child(
                row(locale)
                    .gap(px(Space::S12.pixels() + 2.))
                    .flex_wrap()
                    .children(
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
                                ColorToken::GlassControl
                            };
                            let border = if selected {
                                ColorToken::AccentFocus
                            } else {
                                ColorToken::BorderSubtle
                            };
                            let check = selected.then(|| {
                                div()
                                    .absolute()
                                    .bottom(px(-2.))
                                    .inset_end(px(-8.), cx)
                                    .size(px(Space::S16.pixels() + 2.))
                                    .rounded_full()
                                    .border_1()
                                    .border_color(color(screen.palette, ColorToken::BgElevated))
                                    .bg(color(screen.palette, ColorToken::AccentDefault))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(settings_icon(
                                        screen,
                                        "icons/check.svg",
                                        10.,
                                        ColorToken::TextOnAccent,
                                    ))
                            });
                            let button = Button::new(("accent", accent as usize))
                                .outline()
                                .selected(selected)
                                .h(px(Space::S64.pixels()))
                                .flex_1()
                                .min_w(px(136.))
                                .max_w(px(152.))
                                .rounded(px(Radius::R12.pixels()))
                                .bg(color(screen.palette, panel))
                                .border_color(color(screen.palette, border))
                                .disabled(self.busy() || self.settings.is_none())
                                .accessibility_label(label)
                                .child(
                                    row(locale)
                                        .w_full()
                                        .gap_2()
                                        .child(
                                            div()
                                                .size(px(34.))
                                                .flex_shrink_0()
                                                .relative()
                                                .rounded_full()
                                                .bg(linear_gradient(
                                                    125.,
                                                    linear_color_stop(deep, 0.),
                                                    linear_color_stop(tint, 1.),
                                                ))
                                                .children(check),
                                        )
                                        .child(settings_text(
                                            screen,
                                            Role::Label,
                                            label,
                                            ColorToken::TextPrimary,
                                        )),
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.save_settings(
                                        |settings| settings.ui.accent_color = accent,
                                        cx,
                                    );
                                }));
                            if selected { button.border_2() } else { button }
                        }),
                    ),
            )
    }
}
