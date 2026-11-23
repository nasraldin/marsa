//! Display accent choices beside the appearance preferences.

use marsa_ui::{
    ColorToken,
    gpui::{Context, Div, ParentElement, Styled, div, px},
    gpui_component::{
        Disableable, Selectable,
        button::{Button, ButtonVariants},
    },
    gui_keys as gui,
    preferences::AccentColor,
    tokens::accent_swatch,
};

use super::{SystemView, setting_row, settings_icon};
use crate::screens::common::{ScreenContext, color, row};

impl SystemView {
    pub(super) fn accent_picker(&self, screen: &ScreenContext<'_>, cx: &mut Context<Self>) -> Div {
        let locale = screen.locale;
        let control = row(locale).gap_1().flex_wrap().children(
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
                let selected = self.accent() == accent;
                let label = key.text(locale);
                let [deep, light, _] = accent_swatch(accent);
                let fill = if screen.palette.is_dark() {
                    light
                } else {
                    deep
                };
                let dot = div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(22.))
                    .rounded_full()
                    .bg(marsa_ui::gpui::Hsla::from(fill))
                    .children(selected.then(|| {
                        settings_icon(screen, "icons/check.svg", 13., ColorToken::TextOnAccent)
                    }));
                Button::new(("accent", accent as usize))
                    .ghost()
                    .p_0()
                    .size(px(34.))
                    .selected(selected)
                    .toggled(selected)
                    .accessibility_label(label)
                    .tooltip(label)
                    .disabled(self.busy() || self.settings.is_none())
                    .border_color(color(screen.palette, ColorToken::BorderSubtle))
                    .child(dot)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.save_settings(|settings| settings.ui.accent_color = accent, cx);
                    }))
            }),
        );
        setting_row(
            screen,
            gui::GUI_ACCENT_TITLE.text(locale),
            gui::GUI_ACCENT_DESCRIPTION.text(locale),
            control,
        )
    }
}
