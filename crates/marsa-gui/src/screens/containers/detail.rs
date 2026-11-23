//! Container detail header, overview, and bounded inspection panels.

use crate::screens::common::message;
use marsa_client::docker::{ContainerAction, ContainerDetails, ContainerSummary};
use marsa_ui::gui_keys as gui;
use marsa_ui::{
    ColorToken,
    direction::{LogicalStyle as _, isolate_technical_text, row},
    gpui::{
        AnyElement, App, Context, Div, FontWeight, InteractiveElement as _, IntoElement,
        ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Window,
        div, px,
    },
    gpui_component::{
        Disableable as _, Icon, IconName, Sizable as _,
        button::{Button, ButtonVariants as _},
    },
    tokens::typography::Role,
};

use crate::screens::common::{ScreenContext, label, panel};

use super::{ContainerTarget, ContainerView, Inspection, Tab, color, list, logs, name, stats};

pub(super) fn render(
    view: &ContainerView,
    target: &ContainerTarget,
    entry: &ContainerSummary,
    window: &Window,
    cx: &mut Context<ContainerView>,
) -> AnyElement {
    let screen = view.screen();
    let mut title = row(cx)
        .gap_2()
        .items_center()
        .child(
            list::type_style(Role::Title, screen.locale)
                .apply(div())
                .min_w_0()
                .child(isolate_technical_text(name(entry))),
        )
        .children((!view.external_inventory.is_empty()).then(|| {
            let badge = target
                .endpoint()
                .and_then(|endpoint| view.workspace(endpoint))
                .map_or_else(
                    || "Marsa".into(),
                    |inventory| isolate_technical_text(&inventory.source),
                );
            crate::screens::external_inventory::source_badge(screen.palette, badge)
        }))
        .child(list::state_pill(
            &entry.state,
            screen.palette,
            screen.locale,
            cx,
        ))
        .child(div().flex_1())
        .child(
            Button::new("detail-global-search")
                .ghost()
                .size(px(36.))
                .p_0()
                .bg(color(screen.palette, ColorToken::BgSubtle))
                .icon(IconName::Search)
                .tooltip(gui::GUI_DESKTOP_SEARCH_K.text(screen.locale))
                .accessibility_label(gui::GUI_DESKTOP_SEARCH_COMMANDS.text(screen.locale))
                .on_click(|_, window, cx| {
                    window.dispatch_action(Box::new(crate::desktop::OpenCommandPalette), cx)
                }),
        );
    let entity = cx.entity();
    if super::eligible(entry, ContainerAction::Stop) {
        title = title.child(list::action_button(
            entry,
            target.clone(),
            ContainerAction::Stop,
            view.command_pending,
            (screen.locale, screen.palette),
            &entity,
        ));
    } else {
        title = title.child(list::action_button(
            entry,
            target.clone(),
            ContainerAction::Start,
            view.command_pending,
            (screen.locale, screen.palette),
            &entity,
        ));
    }
    title = title.child(list::action_button(
        entry,
        target.clone(),
        ContainerAction::Restart,
        view.command_pending,
        (screen.locale, screen.palette),
        &entity,
    ));

    let short_id = entry.id.as_str().get(..12).unwrap_or(entry.id.as_str());
    let subtitle = format!("{}  ·  {short_id}", entry.image);
    let tabs = row(cx)
        .gap_1()
        .pb_2()
        .border_b_1()
        .border_color(color(screen.palette, ColorToken::BorderSubtle))
        .children(
            [
                Tab::Overview,
                Tab::Logs,
                Tab::Terminal,
                Tab::Inspect,
                Tab::Stats,
            ]
            .map(|tab| tab_button(tab, tab == view.tab, &screen, cx)),
        );
    let content = match view.tab {
        Tab::Overview => overview(entry, &view.inspection, &screen, cx).into_any_element(),
        Tab::Logs => logs::render(view, entry, &screen, cx),
        Tab::Terminal => super::terminal::render(view, entry, &screen, window, cx),
        Tab::Inspect => inspect(view, entry, &view.inspection, &screen, cx).into_any_element(),
        Tab::Stats => stats::render(view, &screen, cx),
    };
    let root = div()
        .flex()
        .flex_col()
        .size_full()
        .min_h_0()
        .gap_2()
        .child(
            row(cx).child(
                Button::new("back-to-containers")
                    .ghost()
                    .small()
                    .compact()
                    .icon(IconName::ChevronLeft)
                    .label(message(screen.locale, gui::GUI_DESKTOP_CONTAINERS))
                    .on_click(cx.listener(|view, _, _, cx| view.close_detail(cx))),
            ),
        )
        .child(title)
        .child(
            div()
                .text_start(cx)
                .font_family("IBM Plex Mono")
                .text_size(px(12.))
                .text_color(color(screen.palette, ColorToken::TextMuted))
                .child(isolate_technical_text(&subtitle)),
        )
        .child(tabs);

    root.child(content).into_any_element()
}

fn tab_button(
    tab: Tab,
    active: bool,
    screen: &ScreenContext<'_>,
    cx: &Context<ContainerView>,
) -> Button {
    let (id, en, ar, icon) = match tab {
        Tab::Overview => (
            "container-overview",
            "Overview",
            "نظرة عامة",
            "icons/panels-top-left.svg",
        ),
        Tab::Logs => ("container-logs", "Logs", "السجلات", "icons/list.svg"),
        Tab::Terminal => (
            "container-terminal",
            "Terminal",
            "الطرفية",
            "icons/terminal.svg",
        ),
        Tab::Inspect => ("container-inspect", "Inspect", "الفحص", "icons/info.svg"),
        Tab::Stats => ("container-stats", "Stats", "الإحصاءات", "icons/activity.svg"),
    };
    let button = Button::new(id)
        .small()
        .icon(Icon::default().path(icon))
        .label(label(screen, en, ar));
    let button = if active {
        button
            .ghost()
            .bg(color(screen.palette, ColorToken::AccentSubtle))
            .text_color(color(screen.palette, ColorToken::AccentDefault))
    } else {
        button.ghost()
    };
    button.on_click(cx.listener(move |view, _, _, cx| {
        if view.tab != tab {
            view.set_tab(tab, cx);
        }
    }))
}

fn overview_action(tab: Tab, screen: &ScreenContext<'_>, cx: &Context<ContainerView>) -> Button {
    let (id, icon, label) = match tab {
        Tab::Logs => (
            "overview-logs",
            "icons/list.svg",
            gui::GUI_DESKTOP_CONTAINERS_LOGS_TAB.text(screen.locale),
        ),
        Tab::Terminal => (
            "overview-terminal",
            "icons/terminal.svg",
            gui::GUI_DESKTOP_CONTAINERS_TERMINAL_TAB.text(screen.locale),
        ),
        _ => (
            "overview-inspect",
            "icons/info.svg",
            gui::GUI_DESKTOP_CONTAINERS_INSPECT_TAB.text(screen.locale),
        ),
    };
    Button::new(id)
        .outline()
        .size(px(36.))
        .p_0()
        .icon(Icon::empty().path(icon))
        .tooltip(label)
        .accessibility_label(label)
        .on_click(cx.listener(move |view, _, _, cx| view.set_tab(tab, cx)))
}

fn overview(
    entry: &ContainerSummary,
    inspection: &Inspection,
    screen: &ScreenContext<'_>,
    cx: &Context<ContainerView>,
) -> Div {
    let details = match inspection {
        Inspection::Ready(details) => Some(details.as_ref()),
        _ => None,
    };
    let networks = details
        .and_then(|details| details.network_settings.as_ref())
        .map(|settings| {
            settings
                .networks
                .keys()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        })
        .filter(|value| !value.is_empty());
    let state = super::state_label(&entry.state);
    let configuration = card(screen, "Container configuration", "إعدادات الحاوية")
        .min_h(px(214.))
        .text_start(cx)
        .child(field(
            screen,
            cx,
            "Image",
            "الصورة",
            isolate_technical_text(&entry.image),
        ))
        .child(field(
            screen,
            cx,
            "Container ID",
            "معرّف الحاوية",
            isolate_technical_text(entry.id.as_str()),
        ))
        .child(field(
            screen,
            cx,
            "Compose project",
            "مشروع Compose",
            technical(entry.compose_project()),
        ))
        .child(field(
            screen,
            cx,
            "Network",
            "الشبكة",
            technical(networks.as_deref()),
        ));
    let runtime = card(screen, "Runtime", "التشغيل")
        .min_h(px(214.))
        .text_start(cx)
        .child(field(
            screen,
            cx,
            "State",
            "الحالة",
            label(screen, state.0, state.1),
        ))
        .child(field(
            screen,
            cx,
            "Started",
            "وقت التشغيل",
            technical(
                details
                    .and_then(|details| details.state.as_ref())
                    .map(|state| state.started_at.as_str())
                    .filter(|value| !value.starts_with("0001-"))
                    .map(|value| {
                        value
                            .split_once('T')
                            .and_then(|(_, time)| time.get(..8))
                            .unwrap_or(value)
                    }),
            ),
        ))
        .child(field(
            screen,
            cx,
            "Published ports",
            "المنافذ المنشورة",
            list::port_text(entry),
        ))
        .child(field(
            screen,
            cx,
            "Age",
            "العمر",
            list::age_text(entry.created, screen.locale),
        ));
    div()
        .flex()
        .flex_col()
        .gap_3()
        .child(row(cx).gap_3().child(configuration).child(runtime))
        .child(
            card(screen, "Work with this container", "العمل مع هذه الحاوية")
                .text_start(cx)
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(color(screen.palette, ColorToken::TextMuted))
                        .child(message(screen.locale, gui::GUI_DESKTOP_REVIEW_OUTPUT_OPEN_A_SHELL_OR_CHECK_HOW_THE_CONTAINER_RUNS)),
                )
                .child(
                    row(cx).gap_2().children(
                        [Tab::Logs, Tab::Terminal, Tab::Inspect]
                            .into_iter()
                            .map(|tab| overview_action(tab, screen, cx)),
                    ),
                ),
        )
}

fn inspect(
    view: &ContainerView,
    entry: &ContainerSummary,
    inspection: &Inspection,
    screen: &ScreenContext<'_>,
    cx: &Context<ContainerView>,
) -> impl IntoElement {
    let mut body = div()
        .flex()
        .flex_col()
        .gap_3()
        .min_h_0()
        .id("inspect-scroll")
        .overflow_y_scroll();
    let details = match inspection {
        Inspection::Ready(details) => details,
        Inspection::Failed => {
            return body.child(
                card(screen, "Inspect unavailable", "الفحص غير متاح")
                    .child(message(
                        screen.locale,
                        gui::GUI_DESKTOP_COULDN_T_READ_THE_CONTAINER_FROM_THE_ENGINE,
                    ))
                    .child(
                        row(cx).child(
                            Button::new("retry-inspect")
                                .outline()
                                .small()
                                .label(message(screen.locale, gui::GUI_DESKTOP_RETRY))
                                .on_click(
                                    cx.listener(|view, _, _, cx| view.set_tab(Tab::Inspect, cx)),
                                ),
                        ),
                    ),
            );
        }
        Inspection::Idle | Inspection::Loading => {
            return body.child(card(screen, "Inspect", "الفحص").child(message(
                screen.locale,
                gui::GUI_DESKTOP_READING_CONTAINER_DETAILS,
            )));
        }
    };

    let mut env = card(screen, "Environment variables", "متغيرات البيئة")
        .text_start(cx)
        .min_h(px(152.));
    for variable in &details.environment {
        let key = variable.name.clone();
        let current = view.revealed.as_ref().filter(|(name, _)| name == &key);
        let summary = match current {
            Some((_, value)) => format!("{key}={}", value.value()),
            None if variable.has_value => format!("{key}=••••••••"),
            None => key.clone(),
        };
        let revealed = current.is_some();
        let reveal = if revealed {
            message(screen.locale, gui::GUI_DESKTOP_HIDE_VALUE)
        } else {
            message(screen.locale, gui::GUI_DESKTOP_REVEAL_VALUE)
        };
        let control = Button::new((
            marsa_ui::gpui::ElementId::from("inspect-reveal"),
            SharedString::from(key.clone()),
        ))
        .link()
        .small()
        .label(reveal)
        .disabled(!variable.has_value)
        .on_click(cx.listener(move |view, _, _, cx| {
            if revealed {
                view.revealed = None;
                cx.notify();
            } else {
                view.reveal_environment(&key, cx);
            }
        }));
        env = env.child(
            row(cx)
                .gap_2()
                .items_center()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(12.))
                        .font_family("IBM Plex Mono")
                        .child(isolate_technical_text(&summary)),
                )
                .children(
                    (view
                        .focused
                        .as_ref()
                        .is_some_and(|target| target.endpoint().is_none()))
                    .then_some(control),
                ),
        );
    }
    if details.environment.is_empty() {
        env = env.child(message(
            screen.locale,
            gui::GUI_DESKTOP_NO_ENVIRONMENT_VARIABLES,
        ));
    }
    if view.reveal_failed {
        env = env.child(message(
            screen.locale,
            gui::GUI_DESKTOP_COULDN_T_REVEAL_THE_SELECTED_VALUE,
        ));
    }
    let mut network = card(screen, "Ports and networks", "المنافذ والشبكات")
        .text_start(cx)
        .min_h(px(152.));
    if let Some(settings) = &details.network_settings {
        for (port, bindings) in &settings.ports {
            let value = bindings
                .as_ref()
                .map(|bindings| {
                    bindings
                        .iter()
                        .map(|binding| {
                            format!("{}:{} → {port}", binding.host_ip, binding.host_port)
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .filter(|value| !value.is_empty());
            network = network.child(field(
                screen,
                cx,
                "Port",
                "المنفذ",
                technical(value.as_deref().or(Some(port))),
            ));
        }
        for (name, endpoint) in &settings.networks {
            let value = [name.as_str(), &endpoint.ipv4, &endpoint.ipv6]
                .into_iter()
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join(" · ");
            network = network.child(field(
                screen,
                cx,
                "Network",
                "الشبكة",
                isolate_technical_text(&value),
            ));
        }
        if settings.ports.is_empty() && settings.networks.is_empty() {
            network = network.child(message(
                screen.locale,
                gui::GUI_DESKTOP_NO_PORTS_OR_ATTACHED_NETWORKS,
            ));
        }
    } else {
        network = network.child(message(
            screen.locale,
            gui::GUI_DESKTOP_NETWORK_INFORMATION_UNAVAILABLE,
        ));
    }

    body = body.child(row(cx).gap_3().child(env).child(network));
    body = body.child(mounts(entry, details, screen, cx));
    let json = details.redacted_inspect_json().ok();
    let copy_json = json.clone();
    body.child(
        panel(screen)
            .bg(color(screen.palette, ColorToken::BgCode))
            .child(
                row(cx)
                    .items_center()
                    .child(
                        list::type_style(Role::Label, screen.locale)
                            .apply(div())
                            .flex_1()
                            .child(
                                gui::GUI_DESKTOP_CONTAINERS_INSPECT_REDACTED_JSON
                                    .text(screen.locale),
                            ),
                    )
                    .child(
                        Button::new("copy-redacted-inspect")
                            .outline()
                            .small()
                            .icon(IconName::Copy)
                            .tooltip(
                                gui::GUI_DESKTOP_COPY_REDACTED_INSPECT_JSON.text(screen.locale),
                            )
                            .accessibility_label(
                                gui::GUI_DESKTOP_COPY_REDACTED_INSPECT_JSON.text(screen.locale),
                            )
                            .disabled(copy_json.is_none())
                            .on_click(move |_, _, cx| {
                                if let Some(json) = &copy_json {
                                    cx.write_to_clipboard(
                                        marsa_ui::gpui::ClipboardItem::new_string(json.clone()),
                                    );
                                }
                            }),
                    ),
            )
            .child(
                div()
                    .id("redacted-inspect-preview")
                    .max_h(px(136.))
                    .overflow_y_scroll()
                    .text_size(px(12.))
                    .font_family("IBM Plex Mono")
                    .children(json.into_iter().flat_map(|json| {
                        json.lines()
                            .map(|line| div().child(isolate_technical_text(line)))
                            .collect::<Vec<_>>()
                    })),
            ),
    )
}

fn mounts(
    entry: &ContainerSummary,
    details: &ContainerDetails,
    screen: &ScreenContext<'_>,
    cx: &App,
) -> Div {
    let mut card = card(screen, "Mounts and labels", "عمليات الربط والتسميات").text_start(cx);
    for mount in &details.mounts {
        let mode = if mount.writable { "rw" } else { "ro" };
        let value = format!("{} → {} ({mode})", mount.source, mount.destination);
        card = card.child(field(
            screen,
            cx,
            "Mount",
            "الربط",
            isolate_technical_text(&value),
        ));
    }
    if let Some(project) = entry.compose_project() {
        card = card.child(field(
            screen,
            cx,
            "Compose project",
            "مشروع Compose",
            isolate_technical_text(project),
        ));
    }
    if let Some(service) = entry.compose_service() {
        card = card.child(field(
            screen,
            cx,
            "Compose service",
            "خدمة Compose",
            isolate_technical_text(service),
        ));
    }
    for label_name in &details.label_names {
        card = card.child(field(
            screen,
            cx,
            "Label",
            "التسمية",
            isolate_technical_text(label_name),
        ));
    }
    if details.mounts.is_empty()
        && details.label_names.is_empty()
        && entry.compose_project().is_none()
        && entry.compose_service().is_none()
    {
        card = card.child(message(
            screen.locale,
            gui::GUI_DESKTOP_NO_MOUNTS_OR_COMPOSE_LABELS,
        ));
    }
    card
}

pub(super) fn card(screen: &ScreenContext<'_>, en: &'static str, ar: &'static str) -> Div {
    panel(screen)
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .text_size(px(16.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(label(screen, en, ar)),
        )
}

fn field(
    screen: &ScreenContext<'_>,
    cx: &App,
    en: &'static str,
    ar: &'static str,
    value: SharedString,
) -> Div {
    row(cx)
        .gap_2()
        .items_start()
        .text_start(cx)
        .child(
            div()
                .w(px(118.))
                .flex_shrink_0()
                .text_size(px(12.))
                .text_color(color(screen.palette, ColorToken::TextMuted))
                .child(label(screen, en, ar)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(px(12.))
                .font_family("IBM Plex Mono")
                .child(value),
        )
}

fn technical(value: Option<&str>) -> SharedString {
    value
        .map(isolate_technical_text)
        .unwrap_or_else(|| "—".into())
}
