//! Appearance previews shared by first-run setup and Settings.

use marsa_ui::{
    ColorToken, Locale, Palette,
    gpui::{
        AnyElement, Context, Div, ElementId, IntoElement as _, ParentElement as _, Styled as _,
        div, px, relative, svg,
    },
    gpui_component::{Disableable as _, button::Button},
    gui_keys as gui,
    preferences::Theme,
};

use super::common::{color, row};

/// Selected-state mark used by setup and Settings appearance choices.
pub(crate) fn selection_check(palette: Palette) -> AnyElement {
    svg()
        .path("icons/circle-check.svg")
        .size(px(18.))
        .flex_shrink_0()
        .text_color(color(palette, ColorToken::StatusRunning))
        .into_any_element()
}

/// Render the localized visual theme choices with their current selection.
pub(crate) fn theme_picker<T: 'static>(
    id: &'static str,
    locale: Locale,
    palette: Palette,
    selected: Option<Theme>,
    disabled: bool,
    cx: &Context<T>,
    on_select: fn(&mut T, Theme, &mut Context<T>),
) -> Div {
    row(locale).gap_3().flex_wrap().children(
        [
            (
                Theme::System,
                gui::GUI_DESKTOP_SYSTEM,
                palette,
                "icons/monitor.svg",
            ),
            (
                Theme::Light,
                gui::GUI_DESKTOP_LIGHT,
                Palette::Light,
                "icons/sun.svg",
            ),
            (
                Theme::Dark,
                gui::GUI_DESKTOP_DARK,
                Palette::Dark,
                "icons/moon.svg",
            ),
        ]
        .into_iter()
        .enumerate()
        .map(|(index, (theme, name, preview, icon))| {
            let active = selected == Some(theme);
            let name = name.text(locale);
            choice_button((id, index), active, palette)
                .flex_1()
                .min_w(px(132.))
                .h(px(156.))
                .accessibility_label(name)
                .toggled(active)
                .disabled(disabled || selected.is_none())
                .child(
                    div()
                        .w_full()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(theme_thumbnail(locale, preview))
                        .child(
                            row(locale)
                                .justify_between()
                                .gap_2()
                                .child(
                                    row(locale)
                                        .gap_2()
                                        .child(
                                            svg()
                                                .path(icon)
                                                .size(px(16.))
                                                .flex_shrink_0()
                                                .text_color(color(palette, ColorToken::TextMuted)),
                                        )
                                        .child(name),
                                )
                                .children(active.then(|| selection_check(palette))),
                        ),
                )
                .on_click(cx.listener(move |view, _, _, cx| {
                    if !active {
                        on_select(view, theme, cx);
                    }
                }))
        }),
    )
}

/// Style a selectable card in the setup and appearance flows.
pub(crate) fn choice_button(id: impl Into<ElementId>, selected: bool, palette: Palette) -> Button {
    Button::new(id)
        .outline()
        .rounded(px(8.))
        .bg(color(
            palette,
            if selected {
                ColorToken::AccentSubtle
            } else {
                ColorToken::BgElevated
            },
        ))
        .border_color(color(
            palette,
            if selected {
                ColorToken::AccentFocus
            } else {
                ColorToken::BorderSubtle
            },
        ))
}

fn theme_thumbnail(locale: Locale, palette: Palette) -> Div {
    let content = div().flex_1().min_w_0().h_full().flex().flex_col();
    let content = match locale {
        Locale::EnglishUs => content.items_start(),
        Locale::Arabic => content.items_end(),
    };
    row(locale)
        .h(px(92.))
        .w_full()
        .rounded(px(6.))
        .overflow_hidden()
        .bg(color(palette, ColorToken::BgCanvas))
        .child(
            div()
                .w(px(44.))
                .h_full()
                .bg(color(palette, ColorToken::BgSidebar)),
        )
        .child(
            content
                .gap_3()
                .p_3()
                .child(
                    div()
                        .w(relative(0.8))
                        .max_w(px(74.))
                        .h(px(8.))
                        .rounded_full()
                        .bg(color(palette, ColorToken::AccentDefault)),
                )
                .child(
                    div()
                        .w_full()
                        .h(px(8.))
                        .rounded_full()
                        .bg(color(palette, ColorToken::BgSubtle)),
                )
                .child(
                    div()
                        .w(relative(0.65))
                        .max_w(px(60.))
                        .h(px(8.))
                        .rounded_full()
                        .bg(color(palette, ColorToken::BgSubtle)),
                ),
        )
}
