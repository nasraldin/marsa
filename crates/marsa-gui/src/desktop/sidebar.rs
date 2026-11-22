//! Mirrored workspace sidebar and live engine indicator.

use marsa_ui::gui_keys as gui;
use marsa_ui::{
    ColorToken,
    gpui::{
        Context, Div, FontWeight, InteractiveElement, ParentElement, StatefulInteractiveElement,
        Styled, div, img, prelude::FluentBuilder, px, svg,
    },
    gpui_component::button::{Button, ButtonVariants},
};

use crate::screens::common::{align, color, row};

use super::{Desktop, navigation::Route, status};

pub(super) fn render(desktop: &Desktop, cx: &mut Context<Desktop>) -> Div {
    let locale = desktop.locale;
    let palette = desktop.palette;
    let foreground = color(palette, ColorToken::TextMuted);
    let width = if desktop.collapsed { 78. } else { 216. };
    let collapse_label = if desktop.collapsed {
        gui::GUI_DESKTOP_EXPAND_SIDEBAR.text(locale)
    } else {
        gui::GUI_DESKTOP_COLLAPSE_SIDEBAR.text(locale)
    };
    let collapse = Button::new("sidebar-toggle")
        .ghost()
        .size(px(36.))
        .p_0()
        .bg(color(palette, ColorToken::GlassControl))
        .accessibility_label(collapse_label)
        .tooltip(collapse_label)
        .child(marsa_ui::direction::sidebar_toggle(
            locale,
            desktop.collapsed,
        ))
        .on_click(cx.listener(|desktop, _, _, cx| {
            desktop.collapsed = !desktop.collapsed;
            cx.notify();
        }));
    let brand = row(locale)
        .gap_2()
        .child(img("marsa/harbor.png").size(px(30.)))
        .children((!desktop.collapsed).then(|| {
            div()
                .text_size(px(20.))
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
                .gap(px(1.))
                .children((!desktop.collapsed).then(|| {
                    align(
                        div().text_size(px(11.)).text_color(foreground).px_2(),
                        locale,
                    )
                    .child(title)
                }))
                .children(routes.iter().copied().map(|route| {
                    let selected = route == desktop.route;
                    let label = route.name(locale);
                    let icon_color = if selected {
                        color(palette, ColorToken::AccentDefault)
                    } else {
                        foreground
                    };
                    let item = Button::new(("route", route as usize))
                        .ghost()
                        .accessibility_label(label)
                        .tooltip(label)
                        .w_full()
                        .h(px(32.))
                        .rounded(px(8.));
                    let item = if selected {
                        item.bg(color(palette, ColorToken::AccentSubtle))
                    } else {
                        item
                    };
                    item.child(
                        row(locale)
                            .w_full()
                            .when(desktop.collapsed, |row| row.justify_center())
                            .gap_2()
                            .text_size(px(13.))
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
        row(locale).justify_center().child(
            Button::new("sidebar-engine-state")
                .ghost()
                .tooltip(status::text(locale, engine))
                .accessibility_label(status::text(locale, engine))
                .child(
                    svg()
                        .path("icons/server.svg")
                        .size(px(17.))
                        .text_color(foreground),
                )
                .on_click(cx.listener(|desktop, _, _, cx| desktop.navigate(Route::Machine, cx))),
        )
    } else {
        div()
            .rounded(px(10.))
            .border_1()
            .border_color(color(palette, ColorToken::GlassEdge))
            .bg(color(palette, ColorToken::GlassControl))
            .p_2()
            .flex()
            .flex_col()
            .gap_1()
            .child(status::inline_badge(locale, palette, engine))
            .child(
                row(locale)
                    .justify_between()
                    .text_xs()
                    .text_color(foreground)
                    .child(gui::GUI_DESKTOP_DOCKER_CONTEXT.text(locale))
                    .child("marsa"),
            )
            .child(
                row(locale)
                    .gap_3()
                    .text_xs()
                    .text_color(foreground)
                    .child("arm64")
                    .child(gui::GUI_DESKTOP_LOCAL_ENGINE.text(locale)),
            )
    };
    div()
        .flex()
        .flex_col()
        .gap_2()
        .w(px(width))
        .flex_shrink_0()
        .h_full()
        .p_2()
        .rounded(px(14.))
        .border_1()
        .border_color(color(palette, ColorToken::GlassEdge))
        .bg(color(palette, ColorToken::GlassRail))
        .child(if desktop.collapsed {
            div()
                .flex()
                .flex_col()
                .items_center()
                .pt(px(36.))
                .gap_2()
                .child(collapse)
                .child(brand)
        } else {
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(row(locale).h(px(40.)).justify_end().child(collapse))
                .child(row(locale).px_2().pb_3().child(brand))
        })
        .child(navigation)
        .child(footer)
}
