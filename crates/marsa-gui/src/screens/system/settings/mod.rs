//! Versioned preferences and owner controls.

mod accents;
mod compose_template;
mod external_docker;
mod integrations;
mod login;
mod proxy;
mod resources;
mod sharing;
mod update;

use crate::screens::common::message;
use marsa_client::api::UpdateSettingsRequest;
use marsa_ui::gui_keys as gui;
use marsa_ui::{
    ColorToken, Locale,
    gpui::{
        Context, Div, ParentElement, PromptButton, PromptLevel, SharedString, Styled, Window, div,
        px, svg,
    },
    gpui_component::{
        Disableable, IconName, Selectable,
        button::{Button, ButtonVariants},
        menu::{DropdownMenu, PopupMenuItem},
        switch::Switch,
    },
    preferences::{Language, OperateApproval, Settings, Theme},
    tokens::{
        scale::{Radius, Space},
        typography::Role,
    },
};

use crate::screens::common::{ScreenContext, align, color, copy, panel, row};

use super::{Notice, SystemView, machine::title_text, notifications::Kind};

pub(super) use external_docker::EnvironmentSettings;
pub(super) use resources::ResourceInputs;
pub(super) use update::UpdatePanel;

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum SettingsTab {
    General,
    Resources,
    Sharing,
    Network,
    Docker,
    Environments,
    Notifications,
    Agents,
    Updates,
    ComposeTemplate,
    About,
}

impl SettingsTab {
    fn icon(self) -> &'static str {
        match self {
            Self::General => "icons/sliders-horizontal.svg",
            Self::Resources => "icons/cpu.svg",
            Self::Sharing => "icons/folder-open.svg",
            Self::Network => "icons/network.svg",
            Self::Docker => "icons/container.svg",
            Self::Environments => "icons/server.svg",
            Self::Notifications => "icons/bell.svg",
            Self::Agents => "icons/shield-check.svg",
            Self::Updates => "icons/refresh-cw.svg",
            Self::ComposeTemplate => "icons/layout-template.svg",
            Self::About => "icons/info.svg",
        }
    }

    fn title(self, locale: Locale) -> &'static str {
        let (en, ar) = match self {
            Self::General => ("General", "عام"),
            Self::Resources => ("Resources", "الموارد"),
            Self::Sharing => ("Sharing", "المشاركة"),
            Self::Network => ("Network", "الشبكة"),
            Self::Docker => return gui::GUI_SETTINGS_DOCKER_INTEGRATION.text(locale),
            Self::Environments => return gui::GUI_SETTINGS_ENVIRONMENTS.text(locale),
            Self::Notifications => return gui::GUI_SETTINGS_NOTIFICATIONS.text(locale),
            Self::Agents => ("Agents", "الوكلاء"),
            Self::Updates => ("Updates", "التحديثات"),
            Self::About => ("About", "حول"),
            Self::ComposeTemplate => return gui::GUI_COMPOSE_TEMPLATE_TITLE.text(locale),
        };
        copy(locale, en, ar)
    }
}

impl SystemView {
    pub(super) fn settings(
        &self,
        screen: &ScreenContext<'_>,
        compact: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let locale = screen.locale;
        let navigation = div().flex().flex_col().gap_4().flex_shrink_0();
        let navigation = if compact {
            navigation.w_full()
        } else {
            navigation.w(px(170.))
        };
        let navigation = navigation.children(
            [
                (
                    gui::GUI_SETTINGS_SECTION_APPLICATION.text(locale),
                    &[
                        SettingsTab::General,
                        SettingsTab::Notifications,
                        SettingsTab::Updates,
                        SettingsTab::About,
                    ][..],
                ),
                (
                    gui::GUI_SETTINGS_SECTION_INFRASTRUCTURE.text(locale),
                    &[
                        SettingsTab::Environments,
                        SettingsTab::Resources,
                        SettingsTab::Sharing,
                        SettingsTab::Network,
                        SettingsTab::Docker,
                        SettingsTab::ComposeTemplate,
                        SettingsTab::Agents,
                    ][..],
                ),
            ]
            .into_iter()
            .map(|(group, tabs)| {
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        settings_text(screen, Role::Small, group, ColorToken::TextMuted)
                            .px_2()
                            .pb_1(),
                    )
                    .child({
                        let items = if compact {
                            row(locale).flex_wrap().gap_1()
                        } else {
                            div().flex().flex_col().gap_1()
                        };
                        items.children(
                            tabs.iter()
                                .copied()
                                .map(|tab| self.settings_navigation_item(tab, screen, compact, cx)),
                        )
                    })
            }),
        );
        let content = match self.settings_tab {
            SettingsTab::General => self.general(screen, cx),
            SettingsTab::Resources => settings_card(screen)
                .child(settings_heading(
                    screen,
                    gui::GUI_DESKTOP_VIRTUAL_MACHINE_RESOURCES.text(locale),
                    gui::GUI_DESKTOP_CHANGES_APPLY_AT_THE_NEXT_START.text(locale),
                ))
                .child(self.resource_editor(screen, cx)),
            SettingsTab::Sharing => self.sharing(screen, cx),
            SettingsTab::Network => self.network(screen, cx),
            SettingsTab::Docker => self.docker(screen, cx),
            SettingsTab::Environments => self.external_docker(screen, cx),
            SettingsTab::Notifications => self.notification_settings(screen, cx),
            SettingsTab::Agents => self.agent_settings(screen, cx),
            SettingsTab::Updates => self.updates(screen, cx),
            SettingsTab::ComposeTemplate => self.compose_template_settings(screen, cx),
            SettingsTab::About => self.about(screen, cx),
        };
        let layout = if compact {
            div().flex().flex_col()
        } else {
            row(locale).items_start()
        };
        layout.gap_5().child(navigation).child(
            div()
                .flex()
                .flex_col()
                .gap_4()
                .flex_1()
                .min_w_0()
                .child(settings_text(
                    screen,
                    Role::Title,
                    self.settings_tab.title(locale),
                    ColorToken::TextPrimary,
                ))
                .child(content),
        )
    }

    fn settings_navigation_item(
        &self,
        tab: SettingsTab,
        screen: &ScreenContext<'_>,
        compact: bool,
        cx: &Context<Self>,
    ) -> Button {
        let locale = screen.locale;
        let selected = tab == self.settings_tab;
        let foreground = if selected {
            ColorToken::AccentDefault
        } else {
            ColorToken::TextPrimary
        };
        let button = Button::new(("settings-tab", tab as usize))
            .ghost()
            .selected(selected)
            .h(px(Space::S36.pixels()))
            .rounded(px(Radius::R8.pixels()))
            .px_2()
            .accessibility_label(tab.title(locale))
            .child(
                row(locale)
                    .w_full()
                    .gap_2()
                    .child(settings_icon(
                        screen,
                        tab.icon(),
                        Space::S16.pixels(),
                        foreground,
                    ))
                    .child(settings_text(
                        screen,
                        Role::Label,
                        tab.title(locale),
                        foreground,
                    )),
            );
        let button = if compact { button } else { button.w_full() };
        let button = if selected {
            button.bg(color(screen.palette, ColorToken::AccentSubtle))
        } else {
            button
        };
        button.on_click(cx.listener(move |this, _, window, cx| {
            this.settings_tab = tab;
            match tab {
                SettingsTab::Resources | SettingsTab::Network => {
                    this.show_resource_inputs(window, cx)
                }
                SettingsTab::Updates => this.load_update_status(cx),
                SettingsTab::Environments => this.refresh_environments(cx),
                SettingsTab::About => this.load_version(cx),
                SettingsTab::General => this.load_login(cx),
                _ => {}
            }
            cx.notify();
        }))
    }

    fn general(&self, screen: &ScreenContext<'_>, cx: &mut Context<Self>) -> Div {
        let locale = screen.locale;
        let preferences = self.preferences();
        let languages = [
            (Language::System, gui::GUI_DESKTOP_SYSTEM.text(locale)),
            (Language::English, "English (US)"),
            (Language::Arabic, "العربية"),
        ];
        let themes = [
            (Theme::System, gui::GUI_DESKTOP_SYSTEM.text(locale)),
            (Theme::Light, gui::GUI_DESKTOP_LIGHT.text(locale)),
            (Theme::Dark, gui::GUI_DESKTOP_DARK.text(locale)),
        ];
        let general = settings_card(screen)
            .pb(px(Space::S24.pixels()))
            .child(settings_heading(
                screen,
                gui::GUI_SETTINGS_PREFERENCES.text(locale),
                gui::GUI_DESKTOP_CHANGES_APPLY_IMMEDIATELY.text(locale),
            ))
            .child(setting(
                screen,
                "Language",
                "اللغة",
                "Uses the selected language throughout the app.",
                "تُستخدم اللغة المحدّدة في جميع أنحاء التطبيق.",
                setting_picker(
                    screen,
                    (
                        "language-picker",
                        gui::GUI_DESKTOP_SETTINGS_LANGUAGE.text(locale),
                    ),
                    languages,
                    preferences.map(|(language, _)| language),
                    self.busy(),
                    |settings, value| settings.ui.language = value,
                    cx,
                ),
            ))
            .child(setting(
                screen,
                "Appearance",
                "المظهر",
                "Choose a theme or follow macOS.",
                "اختر مظهرًا أو اتبع إعداد macOS.",
                setting_picker(
                    screen,
                    (
                        "appearance-picker",
                        gui::GUI_DESKTOP_SETTINGS_APPEARANCE.text(locale),
                    ),
                    themes,
                    preferences.map(|(_, theme)| theme),
                    self.busy(),
                    |settings, value| settings.ui.theme = value,
                    cx,
                ),
            ))
            .child(self.login_setting(screen, cx));
        div()
            .flex()
            .flex_col()
            .gap(px(Space::S16.pixels() + Space::S4.pixels()))
            .child(general)
            .child(self.accent_picker(screen, cx))
    }

    pub(super) fn save_settings(
        &mut self,
        edit: impl FnOnce(&mut Settings),
        cx: &mut Context<Self>,
    ) {
        if self.busy() {
            return;
        }
        let Some((mut settings, expected_revision)) = self.settings.clone() else {
            return;
        };
        edit(&mut settings);
        let Ok(settings_toml) = settings.to_toml() else {
            self.notice = Some(Notice::InvalidSettings);
            cx.notify();
            return;
        };
        let Some(request_id) = self.mutation_id(cx) else {
            return;
        };
        self.request(
            move |client| async move {
                Ok(client
                    .settings()
                    .update(UpdateSettingsRequest {
                        request_id,
                        settings_toml,
                        expected_revision,
                    })
                    .await?
                    .into_inner())
            },
            |this, snapshot, _| {
                let requires_restart = snapshot.restart_required;
                this.accept_settings(snapshot);
                if this.notice.is_none() {
                    this.notice = Some(if requires_restart {
                        Notice::AppliedNextStart
                    } else {
                        Notice::Saved
                    });
                }
            },
            cx,
        );
    }

    fn notification_settings(&self, screen: &ScreenContext<'_>, cx: &mut Context<Self>) -> Div {
        let settings = self
            .settings
            .as_ref()
            .map(|(settings, _)| settings.ui.notifications);
        let locale = screen.locale;
        settings_card(screen)
            .child(settings_heading(
                screen,
                gui::GUI_DESKTOP_NOTIFICATION_SETTINGS_TITLE.text(locale),
                gui::GUI_DESKTOP_NOTIFICATION_SETTINGS_BODY.text(locale),
            ))
            .children(Kind::ALL.into_iter().enumerate().map(|(index, kind)| {
                let title = kind.title(locale);
                row(locale)
                    .justify_between()
                    .gap_4()
                    .py_3()
                    .border_t_1()
                    .border_color(color(screen.palette, ColorToken::BorderSubtle))
                    .child(title_text(title))
                    .child(
                        Switch::new(("notification-setting", index))
                            .accessibility_label(title)
                            .checked(settings.is_some_and(|settings| kind.enabled(settings)))
                            .disabled(self.busy() || settings.is_none())
                            .on_change(cx.listener(move |this, enabled: &bool, _, cx| {
                                this.save_settings(
                                    |settings| kind.set(&mut settings.ui.notifications, *enabled),
                                    cx,
                                );
                            })),
                    )
            }))
    }

    fn agent_settings(&self, screen: &ScreenContext<'_>, cx: &mut Context<Self>) -> Div {
        let agents = self.settings.as_ref().map(|(settings, _)| &settings.agents);
        let preference = agents.map(|settings| settings.operate_approval);
        let advanced = agents.is_some_and(|settings| settings.advanced_access);
        let locale = screen.locale;
        settings_card(screen)
            .child(settings_heading(
                screen,
                gui::GUI_DESKTOP_AGENT_PERMISSIONS.text(locale),
                gui::GUI_DESKTOP_MANAGE_GRANTS_AND_REVIEW_REQUESTS_IN_ACTIVITY.text(locale),
            ))
            .child(
                row(screen.locale).gap_2().children(
                    [
                        (
                            OperateApproval::AlwaysAsk,
                            message(screen.locale, gui::GUI_DESKTOP_ASK_EVERY_TIME),
                        ),
                        (
                            OperateApproval::PerProject,
                            message(locale, gui::GUI_DESKTOP_ALLOW_PROJECT_GRANTS),
                        ),
                        (
                            OperateApproval::Deny,
                            message(locale, gui::GUI_DESKTOP_AGENT_DENY_OPERATION_REQUESTS),
                        ),
                    ]
                    .into_iter()
                    .map(|(value, label)| {
                        Button::new(("agent-approval", value as usize))
                            .outline()
                            .selected(preference == Some(value))
                            .label(label)
                            .disabled(self.busy() || preference.is_none())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.save_settings(
                                    |settings| settings.agents.operate_approval = value,
                                    cx,
                                )
                            }))
                    }),
                ),
            )
        .child(setting(
            screen,
            "Advanced approval requests",
            "طلبات الموافقة المتقدمة",
            "Allow clients to ask about destructive or host-affecting operations. You must approve each request.",
            "اسمح للعملاء بطلب تنفيذ عمليات إتلاف أو عمليات تؤثر على الجهاز المضيف. يجب أن توافق على كل طلب.",
            div().child(Switch::new("agent-advanced-access")
                .accessibility_label(gui::GUI_DESKTOP_AGENT_ADVANCED_REQUESTS.text(locale))
                .checked(advanced)
                .disabled(self.busy() || agents.is_none())
                .on_change(cx.listener(move |this, enabled: &bool, window, cx| {
                    if *enabled {
                        this.confirm_advanced_access(locale, window, cx);
                    } else {
                        this.save_settings(|settings| settings.agents.advanced_access = false, cx);
                    }
                }))),
        ))
    }

    fn confirm_advanced_access(
        &mut self,
        locale: Locale,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let answer = window.prompt(
            PromptLevel::Warning,
            gui::GUI_DESKTOP_AGENT_ADVANCED_REQUESTS.text(locale),
            Some(gui::GUI_DESKTOP_AGENT_ADVANCED_CONFIRMATION.text(locale)),
            &[
                PromptButton::Ok(gui::GUI_DESKTOP_ENABLE.text(locale).into()),
                PromptButton::Cancel(gui::GUI_DESKTOP_CANCEL.text(locale).into()),
            ],
            cx,
        );
        self.command = Some(cx.spawn(async move |this, cx| {
            if answer.await == Ok(0) {
                let _ = this.update(cx, |this, cx| {
                    this.save_settings(|settings| settings.agents.advanced_access = true, cx);
                });
            }
        }));
    }
}

fn setting_picker<T: Copy + Eq + 'static>(
    screen: &ScreenContext<'_>,
    (id, name): (&'static str, &'static str),
    options: [(T, &'static str); 3],
    selected: Option<T>,
    busy: bool,
    edit: fn(&mut Settings, T),
    cx: &Context<SystemView>,
) -> Div {
    let selected_label = options
        .iter()
        .find_map(|(value, label)| (Some(*value) == selected).then_some(*label));
    let view = cx.weak_entity();
    div().child(
        Button::new(id)
            .outline()
            .h(px(38.))
            .rounded(px(Radius::R8.pixels()))
            .border_color(color(screen.palette, ColorToken::BorderControl))
            .label(selected_label.unwrap_or(name))
            .accessibility_label(name)
            .disabled(selected.is_none() || busy)
            .child(IconName::ChevronDown)
            .dropdown_menu(move |menu, _, _| {
                options.into_iter().fold(menu, |menu, (value, label)| {
                    let view = view.clone();
                    menu.item(
                        PopupMenuItem::new(label)
                            .checked(Some(value) == selected)
                            .on_click(move |_, _, cx| {
                                let _ = view.update(cx, |view, cx| {
                                    view.save_settings(|settings| edit(settings, value), cx);
                                });
                            }),
                    )
                })
            }),
    )
}

fn settings_card(screen: &ScreenContext<'_>) -> Div {
    panel(screen)
        .gap(px(Space::S12.pixels()))
        .rounded(px(Radius::R8.pixels()))
        .pt(px(Space::S12.pixels()))
}

fn settings_icon(
    screen: &ScreenContext<'_>,
    path: &'static str,
    size: f32,
    role: ColorToken,
) -> impl marsa_ui::gpui::IntoElement + use<> {
    svg()
        .path(path)
        .size(px(size))
        .flex_shrink_0()
        .text_color(color(screen.palette, role))
}

fn settings_text(
    screen: &ScreenContext<'_>,
    role: Role,
    value: impl Into<SharedString>,
    token: ColorToken,
) -> Div {
    let style = if screen.locale == Locale::Arabic {
        role.arabic()
    } else {
        role.latin()
    };
    align(style.apply(div()), screen.locale)
        .text_color(color(screen.palette, token))
        .child(value.into())
}

fn settings_heading(
    screen: &ScreenContext<'_>,
    title: &'static str,
    subtitle: &'static str,
) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(Space::S4.pixels()))
        .child(settings_text(
            screen,
            Role::Lead,
            title,
            ColorToken::TextPrimary,
        ))
        .child(settings_text(
            screen,
            Role::Small,
            subtitle,
            ColorToken::TextMuted,
        ))
}

fn setting(
    screen: &ScreenContext<'_>,
    en: &'static str,
    ar: &'static str,
    en_help: &'static str,
    ar_help: &'static str,
    control: Div,
) -> Div {
    row(screen.locale)
        .justify_between()
        .gap(px(Space::S16.pixels()))
        .min_h(px(Space::S72.pixels() - Space::S12.pixels()))
        .py(px(Space::S8.pixels()))
        .border_t_1()
        .border_color(color(screen.palette, ColorToken::BorderSubtle))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(Space::S4.pixels()))
                .flex_1()
                .child(settings_text(
                    screen,
                    Role::Label,
                    copy(screen.locale, en, ar),
                    ColorToken::TextPrimary,
                ))
                .child(settings_text(
                    screen,
                    Role::Small,
                    copy(screen.locale, en_help, ar_help),
                    ColorToken::TextMuted,
                )),
        )
        .child(control)
}
