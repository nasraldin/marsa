//! Versioned preferences and owner controls.

mod about;
mod accents;
mod appearance;
mod compose_template;
mod config_sets;
mod external_docker;
mod git_sources;
mod integrations;
mod login;
mod proxy;
mod registries;
mod resources;
mod sharing;
mod update;

use crate::screens::common::message;
use marsa_client::api::UpdateSettingsRequest;
use marsa_ui::gui_keys as gui;
use marsa_ui::{
    ColorToken, Locale, TextKey,
    direction::LogicalStyle,
    gpui::{
        Anchor, Context, Div, InteractiveElement, ParentElement, PromptButton, PromptLevel,
        SharedString, Styled, Window, div, px, svg,
    },
    gpui_component::{
        Disableable, Selectable,
        button::{Button, ButtonVariants, Toggle, ToggleGroup, ToggleVariants},
        menu::{DropdownMenu, PopupMenuItem},
        scroll::ScrollableElement,
        switch::Switch,
    },
    preferences::{DownloadFormat, Language, LogBufferSize, OperateApproval, Settings, Theme},
    tokens::{
        scale::{Radius, Space},
        typography::Role,
    },
};

use crate::screens::common::{ScreenContext, align, color, copy, panel, row};

use super::{Notice, SystemView, machine::title_text, notifications::Kind};

pub(crate) use external_docker::icon_path as environment_icon_path;
pub(super) use external_docker::{EnvironmentSettings, Observations, VmEditor};
pub(super) use git_sources::GitSources;
pub(super) use registries::RegistrySettings;
pub(super) use resources::ResourceInputs;
pub(super) use update::UpdatePanel;

/// A destination in the Settings workspace.
#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum SettingsTab {
    General,
    Sharing,
    Environments,
    GitSources,
    Registries,
    ConfigSets,
    Notifications,
    Logs,
    Agents,
    Updates,
    ComposeTemplate,
    About,
}

impl SettingsTab {
    /// The same destination groups used in the rail and compact picker.
    pub(crate) const GROUPS: [(TextKey, &'static [Self]); 4] = [
        (
            gui::GUI_SETTINGS_SECTION_APPLICATION,
            &[Self::General, Self::Notifications, Self::Logs],
        ),
        (
            gui::GUI_SETTINGS_SECTION_INFRASTRUCTURE,
            &[Self::Environments, Self::Sharing, Self::Agents],
        ),
        (
            gui::GUI_SETTINGS_SECTION_DEPLOYMENT,
            &[
                Self::Registries,
                Self::GitSources,
                Self::ComposeTemplate,
                Self::ConfigSets,
            ],
        ),
        (
            gui::GUI_SETTINGS_SECTION_SUPPORT,
            &[Self::Updates, Self::About],
        ),
    ];

    /// Icon shared by the Settings rail and command search.
    pub(crate) fn icon(self) -> &'static str {
        match self {
            Self::General => "icons/sliders-horizontal.svg",
            Self::Sharing => "icons/folder-open.svg",
            Self::Environments => "icons/server.svg",
            Self::GitSources => "icons/folder-open.svg",
            Self::Registries => "icons/layers.svg",
            Self::ConfigSets => "icons/container.svg",
            Self::Notifications => "icons/bell.svg",
            Self::Logs => "icons/file-text.svg",
            Self::Agents => "icons/shield-check.svg",
            Self::Updates => "icons/refresh-cw.svg",
            Self::ComposeTemplate => "icons/layout-template.svg",
            Self::About => "icons/info.svg",
        }
    }

    /// Localized Settings destination label.
    pub(crate) fn title(self, locale: Locale) -> &'static str {
        let (en, ar) = match self {
            Self::General => ("General", "عام"),
            Self::Sharing => ("Sharing", "المشاركة"),
            Self::Environments => return gui::GUI_SETTINGS_ENVIRONMENTS.text(locale),
            Self::GitSources => return gui::GUI_GIT_SETTINGS.text(locale),
            Self::Registries => return gui::GUI_REGISTRIES_TITLE.text(locale),
            Self::ConfigSets => return gui::GUI_CONFIG_SETS.text(locale),
            Self::Notifications => return gui::GUI_SETTINGS_NOTIFICATIONS.text(locale),
            Self::Logs => return gui::GUI_DESKTOP_LOG_SETTINGS_TITLE.text(locale),
            Self::Agents => ("Agents", "الوكلاء"),
            Self::Updates => ("Updates", "التحديثات"),
            Self::ComposeTemplate => return gui::GUI_COMPOSE_TEMPLATE_TITLE.text(locale),
            Self::About => return gui::GUI_SETTINGS_ABOUT.text(locale),
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
        let selected = self.settings_tab;
        let navigation =
            if compact {
                div()
                    .flex_shrink_0()
                    .child(self.settings_section_picker(screen, cx))
            } else {
                div()
                    .w(px(188.))
                    .h_full()
                    .flex_shrink_0()
                    .border_end(px(1.), cx)
                    .border_color(color(screen.palette, ColorToken::BorderSubtle))
                    .child(
                        div()
                            .id("settings-navigation")
                            .h_full()
                            .min_h_0()
                            .px_2()
                            .overflow_y_scrollbar()
                            .child(div().flex().flex_col().gap_4().pb_3().children(
                                SettingsTab::GROUPS.into_iter().map(|(group, tabs)| {
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap_1()
                                        .child(
                                            settings_text(
                                                screen,
                                                Role::Small,
                                                group.text(locale),
                                                ColorToken::TextMuted,
                                            )
                                            .px_2()
                                            .pb_1(),
                                        )
                                        .children(tabs.iter().copied().map(|tab| {
                                            self.settings_navigation_item(tab, screen, cx)
                                        }))
                                }),
                            )),
                    )
            };
        let content = match self.settings_tab {
            SettingsTab::General => self.general(screen, cx),
            SettingsTab::Sharing => self.sharing(screen, cx),
            SettingsTab::Environments => self.external_docker(screen, cx),
            SettingsTab::GitSources => self.git_sources(screen, cx),
            SettingsTab::Registries => div()
                .flex()
                .flex_col()
                .gap_4()
                .child(self.registries(screen, cx))
                .child(self.registry_keychain(screen, cx)),
            SettingsTab::ConfigSets => self.config_sets_page(screen, cx),
            SettingsTab::Notifications => self.notification_settings(screen, cx),
            SettingsTab::Logs => self.log_settings_card(screen, cx),
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
        layout.h_full().min_h_0().gap_4().child(navigation).child(
            div()
                .id(("settings-page", selected as usize))
                .flex_1()
                .min_w_0()
                .min_h_0()
                .h_full()
                .px_3()
                .overflow_y_scrollbar()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_4()
                        .w_full()
                        .min_w_0()
                        .pb_5()
                        .child(settings_text(
                            screen,
                            Role::Lead,
                            selected.title(locale),
                            ColorToken::TextPrimary,
                        ))
                        .child(content),
                ),
        )
    }

    fn settings_section_picker(
        &self,
        screen: &ScreenContext<'_>,
        cx: &Context<Self>,
    ) -> impl marsa_ui::gpui::IntoElement {
        let locale = screen.locale;
        let current = self.settings_tab;
        let view = cx.weak_entity();
        let anchor = match locale {
            Locale::EnglishUs => Anchor::TopLeft,
            Locale::Arabic => Anchor::TopRight,
        };
        Button::new("settings-section-picker")
            .outline()
            .icon(marsa_ui::gpui_component::Icon::default().path(current.icon()))
            .label(current.title(locale))
            .dropdown_caret(true)
            .accessibility_label(gui::GUI_SETTINGS_CHOOSE_SECTION.text(locale))
            .dropdown_menu_with_anchor(anchor, move |mut menu, _, _| {
                for (label, items) in SettingsTab::GROUPS {
                    menu = menu.label(label.text(locale));
                    for &tab in items {
                        let view = view.clone();
                        menu = menu.item(
                            PopupMenuItem::new(tab.title(locale))
                                .checked(tab == current)
                                .on_click(move |_, _, cx| {
                                    let _ = view
                                        .update(cx, |this, cx| this.select_settings_tab(tab, cx));
                                }),
                        );
                    }
                }
                menu.max_h(px(420.))
            })
    }

    fn settings_navigation_item(
        &self,
        tab: SettingsTab,
        screen: &ScreenContext<'_>,
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
            .toggled(selected)
            .w_full()
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
        let button = if selected {
            button.bg(color(screen.palette, ColorToken::AccentSubtle))
        } else {
            button
        };
        button.on_click(cx.listener(move |this, _, _, cx| this.select_settings_tab(tab, cx)))
    }

    /// Open a Settings section and load its latest state when needed.
    pub(crate) fn select_settings_tab(&mut self, tab: SettingsTab, cx: &mut Context<Self>) {
        self.settings_tab = tab;
        match tab {
            SettingsTab::Updates => self.load_update_status(cx),
            SettingsTab::Environments => self.refresh_environments(cx),
            SettingsTab::About => self.load_about(cx),
            SettingsTab::GitSources => self.load_git_sources(cx),
            SettingsTab::Registries => self.load_registries(cx),
            SettingsTab::General => self.load_login(cx),
            _ => {}
        }
        cx.notify();
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
        let language = gui::GUI_DESKTOP_SETTINGS_LANGUAGE.text(locale);
        let appearance = gui::GUI_DESKTOP_SETTINGS_APPEARANCE.text(locale);
        let look = preference_section(screen, gui::GUI_SETTINGS_APPEARANCE_SECTION)
            .child(setting_row(
                screen,
                appearance,
                gui::GUI_SETTINGS_THEME_HELP.text(locale),
                setting_choices(
                    ("appearance-picker", appearance),
                    themes,
                    preferences.map(|(_, theme)| theme),
                    self.busy(),
                    |settings, value| settings.ui.theme = value,
                    cx,
                ),
            ))
            .child(self.accent_picker(screen, cx));
        let typography = preference_section(screen, gui::GUI_SETTINGS_TYPOGRAPHY_SECTION)
            .children(self.typography_settings(screen, cx));
        let region = preference_section(screen, gui::GUI_SETTINGS_REGION_SECTION)
            .child(setting_row(
                screen,
                language,
                gui::GUI_SETTINGS_LANGUAGE_HELP.text(locale),
                setting_choices(
                    ("language-picker", language),
                    languages,
                    preferences.map(|(language, _)| language),
                    self.busy(),
                    |settings, value| settings.ui.language = value,
                    cx,
                ),
            ))
            .child(self.time_format_setting(screen, cx));
        let startup = preference_section(screen, gui::GUI_SETTINGS_STARTUP_SECTION)
            .child(self.login_setting(screen, cx));
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(look)
            .child(typography)
            .child(region)
            .child(startup)
    }

    fn log_settings_card(&self, screen: &ScreenContext<'_>, cx: &mut Context<Self>) -> Div {
        let locale = screen.locale;
        let settings = self.settings.as_ref().map(|(settings, _)| settings.ui.logs);
        let buffer = gui::GUI_DESKTOP_LOG_BUFFER_SIZE.text(locale);
        let timestamp = gui::GUI_DESKTOP_LOG_LOCAL_TIMESTAMPS.text(locale);
        let options = [
            (
                LogBufferSize::Compact,
                gui::GUI_DESKTOP_LOG_BUFFER_COMPACT.text(locale),
            ),
            (
                LogBufferSize::Medium,
                gui::GUI_DESKTOP_LOG_BUFFER_MEDIUM.text(locale),
            ),
            (
                LogBufferSize::Full,
                gui::GUI_DESKTOP_LOG_BUFFER_FULL.text(locale),
            ),
        ];
        settings_card(screen)
            .child(settings_heading(
                screen,
                gui::GUI_DESKTOP_LOG_SETTINGS_TITLE.text(locale),
                gui::GUI_DESKTOP_LOG_SETTINGS_BODY.text(locale),
            ))
            .child(setting_row(
                screen,
                buffer,
                gui::GUI_DESKTOP_LOG_BUFFER_BODY.text(locale),
                setting_choices(
                    ("log-buffer-size", buffer),
                    options,
                    settings.map(|settings| settings.buffer),
                    self.busy(),
                    |settings, value| settings.ui.logs.buffer = value,
                    cx,
                ),
            ))
            .child(setting_row(
                screen,
                timestamp,
                gui::GUI_DESKTOP_LOG_LOCAL_TIMESTAMPS_BODY.text(locale),
                div().child(
                    Switch::new("log-local-timestamps")
                        .accessibility_label(timestamp)
                        .checked(settings.is_some_and(|settings| settings.local_timestamps))
                        .disabled(self.busy() || settings.is_none())
                        .on_change(cx.listener(|this, enabled: &bool, _, cx| {
                            this.save_settings(
                                |settings| settings.ui.logs.local_timestamps = *enabled,
                                cx,
                            );
                        })),
                ),
            ))
            .child(setting_row(
                screen,
                gui::GUI_CONTAINER_DOWNLOAD_FORMAT.text(locale),
                gui::GUI_CONTAINER_DOWNLOAD_FORMAT_HELP.text(locale),
                setting_choices(
                    (
                        "download-format",
                        gui::GUI_CONTAINER_DOWNLOAD_FORMAT.text(locale),
                    ),
                    [
                        (DownloadFormat::Tar, "TAR"),
                        (
                            DownloadFormat::SingleFile,
                            gui::GUI_CONTAINER_DOWNLOAD_PLAIN.text(locale),
                        ),
                    ],
                    settings.map(|settings| settings.download_format),
                    self.busy(),
                    |settings, value| settings.ui.logs.download_format = value,
                    cx,
                ),
            ))
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
                row(screen.locale).gap_2().flex_wrap().children(
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
                            .toggled(preference == Some(value))
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

fn setting_choices<T: Copy + Eq + 'static, const N: usize>(
    (id, name): (&'static str, &'static str),
    options: [(T, &'static str); N],
    selected: Option<T>,
    busy: bool,
    edit: fn(&mut Settings, T),
    cx: &Context<SystemView>,
) -> Div {
    let view = cx.weak_entity();
    div().flex_shrink_0().child(
        ToggleGroup::new(id)
            .outline()
            .segmented()
            .single()
            .disabled(selected.is_none() || busy)
            .children(options.iter().enumerate().map(|(index, (value, label))| {
                Toggle::new((id, index))
                    .label(*label)
                    .tooltip(format!("{name} · {label}"))
                    .checked(Some(*value) == selected)
            }))
            .on_click(move |checks, _, cx| {
                let Some(index) = checks.iter().position(|checked| *checked) else {
                    return;
                };
                let (value, _) = options[index];
                if Some(value) != selected {
                    let _ = view.update(cx, |view, cx| {
                        view.save_settings(|settings| edit(settings, value), cx);
                    });
                }
            }),
    )
}

fn preference_section(screen: &ScreenContext<'_>, title: TextKey) -> Div {
    settings_card(screen).gap_1().child(
        settings_text(
            screen,
            Role::Label,
            title.text(screen.locale),
            ColorToken::TextPrimary,
        )
        .pb_2(),
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
    let style = screen.typography(role);
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
    setting_row(
        screen,
        copy(screen.locale, en, ar),
        copy(screen.locale, en_help, ar_help),
        control,
    )
}

fn setting_row(
    screen: &ScreenContext<'_>,
    title: &'static str,
    description: &'static str,
    control: Div,
) -> Div {
    row(screen.locale)
        .justify_between()
        .flex_wrap()
        .gap_x(px(Space::S16.pixels()))
        .gap_y(px(Space::S8.pixels()))
        .min_h(px(Space::S72.pixels() - Space::S12.pixels()))
        .py(px(Space::S8.pixels()))
        .border_t_1()
        .border_color(color(screen.palette, ColorToken::BorderSubtle))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(Space::S4.pixels()))
                .min_w(px(180.))
                .flex_1()
                .child(settings_text(
                    screen,
                    Role::Label,
                    title,
                    ColorToken::TextPrimary,
                ))
                .child(settings_text(
                    screen,
                    Role::Small,
                    description,
                    ColorToken::TextMuted,
                )),
        )
        .child(control)
}
