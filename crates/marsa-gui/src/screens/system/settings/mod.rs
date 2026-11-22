//! Versioned preferences and owner controls.

mod accents;
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
    gpui::{Context, Div, ParentElement, PromptButton, PromptLevel, Styled, Window, div},
    gpui_component::{
        Disableable, IconName, Selectable,
        button::{Button, ButtonVariants},
        menu::{DropdownMenu, PopupMenuItem},
        switch::Switch,
    },
    preferences::{Language, OperateApproval, Settings, Theme},
};

use crate::screens::common::{ScreenContext, color, copy, label, panel, row};

use super::{Notice, SystemView, machine::title_text, notifications::Kind};

pub(super) use resources::ResourceInputs;
pub(super) use update::UpdatePanel;

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum SettingsTab {
    General,
    Resources,
    Sharing,
    Network,
    Docker,
    Agents,
    Updates,
    About,
}

impl SettingsTab {
    fn title(self, locale: Locale) -> &'static str {
        let (en, ar) = match self {
            Self::General => ("General", "عام"),
            Self::Resources => ("Resources", "الموارد"),
            Self::Sharing => ("Sharing", "المشاركة"),
            Self::Network => ("Network", "الشبكة"),
            Self::Docker => ("Docker", "Docker"),
            Self::Agents => ("Agents", "الوكلاء"),
            Self::Updates => ("Updates", "التحديثات"),
            Self::About => ("About", "حول"),
        };
        copy(locale, en, ar)
    }
}

impl SystemView {
    pub(super) fn settings(&self, screen: &ScreenContext<'_>, cx: &mut Context<Self>) -> Div {
        let locale = screen.locale;
        let tabs = row(locale)
            .flex_wrap()
            .gap_1()
            .border_b_1()
            .border_color(color(screen.palette, ColorToken::BorderSubtle))
            .pb_2()
            .children(
                [
                    SettingsTab::General,
                    SettingsTab::Resources,
                    SettingsTab::Sharing,
                    SettingsTab::Network,
                    SettingsTab::Docker,
                    SettingsTab::Agents,
                    SettingsTab::Updates,
                    SettingsTab::About,
                ]
                .into_iter()
                .map(|tab| {
                    Button::new(("settings-tab", tab as usize))
                        .ghost()
                        .selected(tab == self.settings_tab)
                        .label(tab.title(locale))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.settings_tab = tab;
                            if matches!(tab, SettingsTab::Resources | SettingsTab::Network) {
                                this.show_resource_inputs(window, cx);
                            }
                            if tab == SettingsTab::Updates {
                                this.load_update_status(cx);
                            }
                            if tab == SettingsTab::About {
                                this.load_version(cx);
                            }
                            if tab == SettingsTab::General {
                                this.load_login(cx);
                            }
                            cx.notify();
                        }))
                }),
            );
        let content = match self.settings_tab {
            SettingsTab::General => self.general(screen, cx),
            SettingsTab::Resources => panel(screen)
                .child(title_text(
                    gui::GUI_DESKTOP_VIRTUAL_MACHINE_RESOURCES.text(screen.locale),
                ))
                .child(message(
                    screen.locale,
                    gui::GUI_DESKTOP_CHANGES_APPLY_AT_THE_NEXT_START,
                ))
                .child(self.resource_editor(screen, cx)),
            SettingsTab::Sharing => self.sharing(screen, cx),
            SettingsTab::Network => self.network(screen, cx),
            SettingsTab::Docker => self.docker(screen, cx),
            SettingsTab::Agents => self.agent_settings(screen, cx),
            SettingsTab::Updates => self.updates(screen, cx),
            SettingsTab::About => self.about(screen),
        };
        div().flex().flex_col().gap_4().child(tabs).child(content)
    }

    fn general(&self, screen: &ScreenContext<'_>, cx: &mut Context<Self>) -> Div {
        let locale = screen.locale;
        let preferences = self.preferences();
        let disabled = self.busy() || preferences.is_none();
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
        let general = panel(screen)
            .child(title_text(gui::GUI_DESKTOP_GENERAL.text(screen.locale)))
            .child(message(
                screen.locale,
                gui::GUI_DESKTOP_CHANGES_APPLY_IMMEDIATELY,
            ))
            .child(setting(
                screen,
                "Language",
                "اللغة",
                "Uses the selected language throughout the app.",
                "تُستخدم اللغة المحدّدة في جميع أنحاء التطبيق.",
                setting_picker(
                    "language-picker",
                    gui::GUI_DESKTOP_SETTINGS_LANGUAGE.text(locale),
                    languages,
                    preferences.map(|(language, _)| language),
                    disabled,
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
                    "appearance-picker",
                    gui::GUI_DESKTOP_SETTINGS_APPEARANCE.text(locale),
                    themes,
                    preferences.map(|(_, theme)| theme),
                    disabled,
                    |settings, value| settings.ui.theme = value,
                    cx,
                ),
            ))
            .child(self.login_setting(screen, cx));
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(general)
            .child(self.accent_picker(screen, cx))
            .child(self.notification_settings(screen, cx))
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
        panel(screen)
            .child(title_text(
                gui::GUI_DESKTOP_NOTIFICATION_SETTINGS_TITLE.text(locale),
            ))
            .child(message(locale, gui::GUI_DESKTOP_NOTIFICATION_SETTINGS_BODY))
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
        panel(screen)
            .child(title_text(
                gui::GUI_DESKTOP_AGENT_PERMISSIONS.text(screen.locale),
            ))
            .child(message(
                screen.locale,
                gui::GUI_DESKTOP_MANAGE_GRANTS_AND_REVIEW_REQUESTS_IN_ACTIVITY,
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
    id: &'static str,
    name: &'static str,
    options: [(T, &'static str); 3],
    selected: Option<T>,
    disabled: bool,
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
            .label(selected_label.unwrap_or(name))
            .accessibility_label(name)
            .disabled(disabled)
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
        .gap_4()
        .py_3()
        .border_t_1()
        .border_color(color(screen.palette, ColorToken::BorderSubtle))
        .child(
            div()
                .flex_1()
                .child(title_text(copy(screen.locale, en, ar)))
                .child(
                    div()
                        .text_sm()
                        .text_color(color(screen.palette, ColorToken::TextMuted))
                        .child(label(screen, en_help, ar_help)),
                ),
        )
        .child(control)
}
