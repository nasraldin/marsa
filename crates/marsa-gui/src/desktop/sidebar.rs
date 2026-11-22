//! Mirrored workspace sidebar and live engine indicator.

use marsa_ui::gui_keys as gui;
use marsa_ui::{
    ColorToken, Locale,
    gpui::{
        Context, Div, FontWeight, InteractiveElement, ParentElement, StatefulInteractiveElement,
        Styled, div, px, svg,
    },
    gpui_component::{
        IconName, Selectable,
        button::{Button, ButtonVariants},
    },
};

use crate::screens::common::{align, color, row};

use super::{Desktop, navigation::Route, status};

pub(super) fn render(desktop: &Desktop, cx: &mut Context<Desktop>) -> Div {
    let locale = desktop.locale;
    let palette = desktop.palette;
    let foreground = color(palette, ColorToken::TextMuted);
    let width = if desktop.collapsed { 56. } else { 216. };
    let collapse_label = if desktop.collapsed {
        gui::GUI_DESKTOP_EXPAND_SIDEBAR.text(locale)
    } else {
        gui::GUI_DESKTOP_COLLAPSE_SIDEBAR.text(locale)
    };
    let collapse = Button::new("sidebar-toggle")
        .ghost()
        .tooltip(collapse_label)
        .child(IconName::PanelLeft)
        .on_click(cx.listener(|desktop, _, _, cx| {
            desktop.collapsed = !desktop.collapsed;
            cx.notify();
        }));
    let brand = row(locale)
        .gap_2()
        .child(svg().path("marsa/harbor.svg").size(px(30.)))
        .children((!desktop.collapsed).then(|| {
            div()
                .text_xl()
                .font_weight(FontWeight::SEMIBOLD)
                .child(gui::GUI_DESKTOP_MARSA.text(locale))
        }));
    let mut navigation = div()
        .id("sidebar-navigation")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .gap_2();
    for (title, routes) in [
        (
            gui::GUI_DESKTOP_WORKSPACE.text(locale),
            Route::WORKSPACE.as_slice(),
        ),
        (
            gui::GUI_DESKTOP_SYSTEM.text(locale),
            Route::SYSTEM.as_slice(),
        ),
    ] {
        navigation = navigation.child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .children((!desktop.collapsed).then(|| {
                    align(div().text_xs().text_color(foreground).px_2(), locale).child(title)
                }))
                .children(routes.iter().copied().map(|route| {
                    let selected = route == desktop.route;
                    let label = route.name(locale);
                    let icon_color = if selected {
                        color(palette, ColorToken::AccentDefault)
                    } else {
                        foreground
                    };
                    Button::new(("route", route as usize))
                        .ghost()
                        .selected(selected)
                        .tooltip(label)
                        .w_full()
                        .child(
                            row(locale)
                                .w_full()
                                .gap_2()
                                .text_color(icon_color)
                                .child(
                                    svg()
                                        .path(route.icon())
                                        .size(px(17.))
                                        .text_color(icon_color),
                                )
                                .children((!desktop.collapsed).then_some(label)),
                        )
                        .on_click(cx.listener(move |desktop, _, _, cx| desktop.navigate(route, cx)))
                })),
        );
    }
    let engine = desktop.engine();
    let footer = if desktop.collapsed {
        div().child(
            Button::new("sidebar-engine-state")
                .ghost()
                .tooltip(status::text(locale, engine))
                .child(svg().path("icons/server.svg").size(px(17.)))
                .on_click(cx.listener(|desktop, _, _, cx| desktop.navigate(Route::Machine, cx))),
        )
    } else {
        div()
            .rounded(px(10.))
            .border_1()
            .border_color(color(palette, ColorToken::BorderSubtle))
            .bg(color(palette, ColorToken::BgSurface))
            .p_2()
            .flex()
            .flex_col()
            .gap_1()
            .child(status::badge(locale, palette, engine))
            .child(
                row(locale)
                    .justify_between()
                    .text_xs()
                    .text_color(foreground)
                    .child(gui::GUI_DESKTOP_DOCKER_CONTEXT.text(locale))
                    .child("marsa"),
            )
    };
    div()
        .flex()
        .flex_col()
        .gap_3()
        .w(px(width))
        .flex_shrink_0()
        .h_full()
        .p_2()
        .rounded(px(14.))
        .border_1()
        .border_color(color(palette, ColorToken::BorderSubtle))
        .bg(color(palette, ColorToken::BgSidebar))
        .child(row(locale).justify_between().child(brand).child(collapse))
        .child(navigation)
        .child(footer)
}
