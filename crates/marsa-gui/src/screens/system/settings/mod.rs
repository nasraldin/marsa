//! Versioned preferences and owner controls.

mod integrations;
mod login;
mod resources;
mod sharing;

use crate::screens::common::message;
use marsa_client::api::UpdateSettingsRequest;
use marsa_ui::gui_keys as gui;
use marsa_ui::{
    ColorToken, Locale,
    gpui::{Context, Div, ParentElement, Styled, div},
    gpui_component::{
        Disableable, Selectable,
        button::{Button, ButtonVariants},
    },
    preferences::{Language, OperateApproval, Settings, Theme},
};

use crate::screens::common::{ScreenContext, color, copy, label, panel, row};

use super::{Notice, SystemView, machine::title_text};

pub(super) use resources::ResourceInputs;

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
        panel(screen)
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
                row(locale)
                    .gap_1()
                    .children(languages.into_iter().map(|(value, name)| {
                        Button::new(("language", value as usize))
                            .outline()
                            .selected(preferences.is_some_and(|(language, _)| language == value))
                            .disabled(disabled)
                            .label(name)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.save_settings(|settings| settings.ui.language = value, cx)
                            }))
                    })),
            ))
            .child(setting(
                screen,
                "Appearance",
                "المظهر",
                "Choose a theme or follow macOS.",
                "اختر مظهرًا أو اتبع إعداد macOS.",
                row(locale)
                    .gap_1()
                    .children(themes.into_iter().map(|(value, name)| {
                        Button::new(("appearance", value as usize))
                            .outline()
                            .selected(preferences.is_some_and(|(_, theme)| theme == value))
                            .disabled(disabled)
                            .label(name)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.save_settings(|settings| settings.ui.theme = value, cx)
                            }))
                    })),
            ))
            .child(self.login_setting(screen, cx))
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

    fn agent_settings(&self, screen: &ScreenContext<'_>, cx: &mut Context<Self>) -> Div {
        let preference = self
            .settings
            .as_ref()
            .map(|(settings, _)| settings.agents.operate_approval);
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
                            message(screen.locale, gui::GUI_DESKTOP_ALLOW_PROJECT_GRANTS),
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
    }
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
