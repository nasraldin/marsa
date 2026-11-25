//! Mirrored workspace sidebar and live engine indicator.

use marsa_ui::{
    ColorToken, brand_mark_for_source,
    gpui::{
        Anchor, AnyElement, Context, Div, FontWeight, InteractiveElement, IntoElement,
        ParentElement, Styled, div, img, prelude::FluentBuilder, px, svg,
    },
    gpui_component::{
        Icon, IconName, Selectable,
        button::{Button, ButtonVariants},
        menu::{DropdownMenu as _, PopupMenuItem},
    },
    is_brand_mark,
};
use marsa_ui::{gui_keys as gui, tokens::typography::ui_size};

use crate::screens::{
    common::{align, color, row},
    external_inventory::ExternalWorkspace,
    system::{SystemView, environment_icon_path},
};

use marsa_ui::gpui_component::scroll::ScrollableElement as _;

use super::{Desktop, Environment, navigation::Route, status};

pub(super) fn render(desktop: &Desktop, cx: &mut Context<Desktop>) -> Div {
    let locale = desktop.locale;
    let palette = desktop.palette;
    let foreground = color(palette, ColorToken::TextMuted);
    let width = if desktop.collapsed {
        marsa_ui::direction::COLLAPSED_SIDEBAR_WIDTH
    } else {
        216.
    };
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
                .text_size(ui_size(20.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(gui::GUI_DESKTOP_MARSA.text(locale))
        }));
    let collapsed = desktop.collapsed;
    let nav_icon = px(36.);
    let mut navigation = div()
        .id("sidebar-navigation")
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .gap_3()
        .when(collapsed, |nav| nav.items_center());
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
                .when(collapsed, |group| group.items_center().gap_2())
                .when(!collapsed, |group| group.gap_1())
                .children((!collapsed).then(|| {
                    align(
                        div().text_size(ui_size(11.)).text_color(foreground).px_2(),
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
                    let label_color = if selected {
                        icon_color
                    } else {
                        color(palette, ColorToken::TextPrimary)
                    };
                    let item = Button::new(("route", route as usize))
                        .ghost()
                        .selected(selected)
                        .toggled(selected)
                        .accessibility_label(label)
                        .tooltip(label)
                        .rounded(px(10.))
                        .p_0();
                    let item = if collapsed {
                        item.size(nav_icon)
                    } else {
                        item.w_full().h(nav_icon).px_2()
                    };
                    let item = if selected {
                        item.bg(color(palette, ColorToken::AccentSubtle))
                    } else {
                        item
                    };
                    item.child(
                        row(locale)
                            .when(collapsed, |row| row.size(nav_icon).justify_center())
                            .when(!collapsed, |row| row.w_full().gap_2())
                            .items_center()
                            .text_size(ui_size(13.))
                            .text_color(label_color)
                            .child(
                                svg()
                                    .path(route.icon())
                                    .size(px(18.))
                                    .text_color(icon_color),
                            )
                            .children((!collapsed).then_some(label)),
                    )
                    .on_click(cx.listener(move |desktop, _, _, cx| desktop.navigate(route, cx)))
                })),
        );
    }
    let footer = context_card(desktop, cx);
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
        .border_color(color(palette, ColorToken::BorderSubtle))
        .bg(color(palette, ColorToken::BgSidebar))
        .child(if collapsed {
            // Clear the traffic lights (y≈18, ~14px tall) before the toggle and brand.
            div()
                .flex()
                .flex_col()
                .items_center()
                .w_full()
                .pt(px(40.))
                .gap_3()
                .pb_2()
                .child(collapse)
                .child(brand)
        } else {
            div()
                .flex()
                .flex_col()
                .gap_2()
                .pt(px(4.))
                .child(row(locale).h(px(36.)).justify_end().child(collapse))
                .child(row(locale).px_2().pb_3().child(brand))
        })
        // Collapsed rail still scrolls if needed; square icon buttons keep equal side padding.
        .child(navigation.overflow_y_scrollbar())
        .child(footer)
}

fn context_card(desktop: &Desktop, cx: &mut Context<Desktop>) -> impl marsa_ui::gpui::IntoElement {
    let locale = desktop.locale;
    let palette = desktop.palette;
    let engine = desktop.context_engine(cx);
    let current = desktop.environment.read(cx).clone();
    let workspace = desktop
        .external_inventory
        .iter()
        .find(|workspace| current.includes_external(workspace));
    let context = if current.includes_marsa() || workspace.is_some() {
        let (name, _) = desktop
            .system
            .read(cx)
            .environment_presentation(workspace.map(AsRef::as_ref));
        crate::screens::common::technical(&name)
    } else {
        current.display()
    };
    let label = gui::GUI_DESKTOP_DOCKER_CONTEXT.text(locale);
    let state_text = desktop.context_status_text(engine, cx);
    let description = format!("{label}: {context} · {state_text}");
    let system = desktop.system.read(cx);
    let active_icon = if current.includes_marsa() {
        "marsa/harbor.png"
    } else {
        workspace.map_or("icons/server.svg", |workspace| {
            context_icon(system, workspace)
        })
    };
    let choices = std::iter::once((
        Environment::Marsa,
        system.environment_presentation(None).0,
        "marsa/harbor.png",
    ))
    .chain(desktop.external_inventory.iter().map(|workspace| {
        (
            Environment::from(workspace.as_ref()),
            system.environment_presentation(Some(workspace)).0,
            context_icon(system, workspace),
        )
    }))
    .collect::<Vec<_>>();
    let desktop_entity = cx.entity().downgrade();
    let button = Button::new("sidebar-engine-state")
        .ghost()
        .w_full()
        .rounded(px(10.))
        .border_1()
        .border_color(color(palette, ColorToken::GlassEdge))
        .bg(color(palette, ColorToken::BgElevated))
        .accessibility_label(description.clone())
        .tooltip(description);
    let (status_text, status_surface) = status::themes(engine);
    let engine_icon = div()
        .size(px(40.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(9.))
        .bg(color(palette, status_surface))
        .child(engine_mark(
            active_icon,
            26.,
            Some(color(palette, status_text)),
        ));
    let button = if desktop.collapsed {
        button.h(px(52.)).p_0().child(engine_icon)
    } else {
        button.h(px(58.)).px_2().child(
            row(locale)
                .w_full()
                .gap_2()
                .child(engine_icon)
                .child(
                    align(div().min_w_0().flex_1().flex().flex_col().gap_1(), locale)
                        .child(
                            div()
                                .truncate()
                                .text_size(ui_size(13.))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(color(palette, ColorToken::TextPrimary))
                                .child(context),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_size(ui_size(12.))
                                .text_color(color(palette, status_text))
                                .child(state_text),
                        ),
                )
                .child(
                    Icon::new(IconName::ChevronUp)
                        .size(px(14.))
                        .text_color(color(palette, ColorToken::TextMuted)),
                ),
        )
    };
    let anchor = match locale {
        marsa_ui::Locale::Arabic => Anchor::BottomRight,
        marsa_ui::Locale::EnglishUs => Anchor::BottomLeft,
    };
    button.dropdown_menu_with_anchor(anchor, move |menu, _, _| {
        let menu = choices.iter().fold(menu, |menu, (choice, name, icon)| {
            let picked = choice.clone();
            let desktop = desktop_entity.clone();
            let label = crate::screens::common::technical(name);
            let path = *icon;
            // Brand marks keep authored colors; monochrome glyphs use Icon tinting.
            let item = if is_brand_mark(path) {
                PopupMenuItem::element(move |_, _| {
                    row(locale)
                        .gap_2()
                        .items_center()
                        .child(img(path).size(px(16.)).flex_shrink_0())
                        .child(label.clone())
                })
            } else {
                PopupMenuItem::new(label).icon(Icon::default().path(path))
            };
            menu.item(
                item.checked(choice.same_identity(&current))
                    .on_click(move |_, _, cx| {
                        let _ = desktop.update(cx, |desktop, cx| {
                            desktop.select_environment(picked.clone(), cx)
                        });
                    }),
            )
        });
        let desktop = desktop_entity.clone();
        menu.separator().item(
            PopupMenuItem::new(gui::GUI_ENVIRONMENT_MANAGE.text(locale)).on_click(
                move |_, _, cx| {
                    let _ = desktop.update(cx, |desktop, cx| desktop.open_contexts(cx));
                },
            ),
        )
    })
}

/// Render a context glyph. Brand SVGs/PNGs use `img` so theme tinting cannot recolor them.
fn engine_mark(path: &'static str, size: f32, tint: Option<marsa_ui::gpui::Hsla>) -> AnyElement {
    if is_brand_mark(path) {
        img(path).size(px(size)).flex_shrink_0().into_any_element()
    } else {
        let mut glyph = svg().path(path).size(px(size));
        if let Some(tint) = tint {
            glyph = glyph.text_color(tint);
        }
        glyph.into_any_element()
    }
}

fn context_icon(system: &SystemView, workspace: &ExternalWorkspace) -> &'static str {
    let (name, icon) = system.environment_presentation(Some(workspace));
    if let Some(mark) =
        brand_mark_for_source(&name).or_else(|| brand_mark_for_source(&workspace.source))
    {
        return mark;
    }
    let lima_like =
        workspace.endpoint.lima_instance().is_some() || workspace.source.starts_with("Colima · ");
    if lima_like && workspace.endpoint.managed_id().is_none() {
        return "icons/laptop.svg";
    }
    environment_icon_path(icon)
}
