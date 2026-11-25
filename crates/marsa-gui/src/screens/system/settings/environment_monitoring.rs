//! Edit observation and maintenance preferences for the selected engine.

use marsa_ui::{
    ColorToken, TextKey,
    gpui::{Context, Div, ParentElement as _, Styled as _, div},
    gpui_component::{
        Disableable as _,
        button::{Toggle, ToggleGroup, ToggleVariants as _},
        switch::Switch,
    },
    gui_keys as gui,
    preferences::{ActivityRetention, EnvironmentMonitoring},
    tokens::typography::Role,
};

use super::{SystemView, setting_row, settings_card, settings_heading, settings_text};
use crate::screens::common::{ScreenContext, technical};

type MonitoringSwitch = (
    TextKey,
    TextKey,
    bool,
    bool,
    fn(&mut EnvironmentMonitoring, bool),
);

impl SystemView {
    pub(super) fn environment_monitoring(
        &self,
        screen: &ScreenContext<'_>,
        cx: &Context<Self>,
    ) -> Div {
        let locale = screen.locale;
        let key = self.active_environment.settings_key();
        let managed = self.active_environment.includes_marsa();
        let target = self
            .external_inventory
            .iter()
            .find(|workspace| self.active_environment.includes_external(workspace))
            .map(|workspace| self.environment_presentation(Some(workspace)).0)
            .unwrap_or_else(|| {
                if managed {
                    self.environment_presentation(None).0
                } else {
                    self.active_environment.name().to_owned()
                }
            });
        let preferences = self.monitoring();
        let disabled = self.busy() || self.settings.is_none();
        let options = [
            (
                ActivityRetention::OneHour,
                gui::GUI_ENVIRONMENT_MONITORING_ONE_HOUR.text(locale),
            ),
            (
                ActivityRetention::SixHours,
                gui::GUI_ENVIRONMENT_MONITORING_SIX_HOURS.text(locale),
            ),
            (
                ActivityRetention::OneDay,
                gui::GUI_ENVIRONMENT_MONITORING_ONE_DAY.text(locale),
            ),
        ];
        let history_key = key.clone();
        let view = cx.weak_entity();
        let history = ToggleGroup::new("environment-activity-retention")
            .outline()
            .segmented()
            .single()
            .disabled(disabled || !preferences.collect_activity)
            .children(options.iter().enumerate().map(|(index, (value, label))| {
                Toggle::new(("activity-retention", index))
                    .label(*label)
                    .tooltip(*label)
                    .checked(preferences.activity_retention == *value)
            }))
            .on_click(move |checks, _, cx| {
                if let Some(index) = checks.iter().position(|checked| *checked) {
                    let _ = view.update(cx, |this, cx| {
                        this.save_monitoring(
                            &history_key,
                            |entry| entry.activity_retention = options[index].0,
                            cx,
                        );
                    });
                }
            });
        let mut switches: Vec<MonitoringSwitch> = vec![
            (
                gui::GUI_ENVIRONMENT_MONITORING_COLLECT,
                gui::GUI_ENVIRONMENT_MONITORING_COLLECT_HELP,
                preferences.collect_activity,
                disabled,
                |entry, value| entry.collect_activity = value,
            ),
            (
                gui::GUI_ENVIRONMENT_MONITORING_NOTIFY,
                gui::GUI_ENVIRONMENT_MONITORING_NOTIFY_HELP,
                preferences.notify_unhealthy,
                disabled || !preferences.collect_activity,
                |entry, value| entry.notify_unhealthy = value,
            ),
            (
                gui::GUI_ENVIRONMENT_MONITORING_IMAGE_UPDATES,
                if managed {
                    gui::GUI_ENVIRONMENT_MONITORING_LOCAL_IMAGE_UPDATES_HELP
                } else {
                    gui::GUI_ENVIRONMENT_MONITORING_IMAGE_UPDATES_HELP
                },
                preferences.check_image_updates,
                disabled,
                |entry, value| entry.check_image_updates = value,
            ),
            (
                gui::GUI_ENVIRONMENT_MONITORING_VOLUMES,
                gui::GUI_ENVIRONMENT_MONITORING_VOLUMES_HELP,
                preferences.show_volume_candidates,
                disabled,
                |entry, value| entry.show_volume_candidates = value,
            ),
        ];
        if managed {
            switches.insert(
                3,
                (
                    gui::GUI_ENVIRONMENT_MONITORING_MANAGED_REGISTRY_UPDATES,
                    gui::GUI_ENVIRONMENT_MONITORING_MANAGED_REGISTRY_UPDATES_HELP,
                    preferences.check_managed_registry_updates,
                    disabled || !preferences.check_image_updates,
                    |entry, value| entry.check_managed_registry_updates = value,
                ),
            );
        }
        settings_card(screen)
            .child(settings_heading(
                screen,
                gui::GUI_ENVIRONMENT_MONITORING_TITLE.text(locale),
                gui::GUI_ENVIRONMENT_SCOPE.text(locale),
            ))
            .child(settings_text(
                screen,
                Role::Small,
                technical(&target),
                ColorToken::TextPrimary,
            ))
            .children(switches.into_iter().enumerate().map(
                |(index, (label, help, enabled, disabled, edit))| {
                    let key = key.clone();
                    setting_row(
                        screen,
                        label.text(locale),
                        help.text(locale),
                        div().flex_shrink_0().child(
                            Switch::new(("environment-monitoring", index))
                                .accessibility_label(label.text(locale))
                                .checked(enabled)
                                .disabled(disabled)
                                .on_change(cx.listener(move |this, enabled: &bool, _, cx| {
                                    this.save_monitoring(&key, |entry| edit(entry, *enabled), cx);
                                })),
                        ),
                    )
                },
            ))
            .child(setting_row(
                screen,
                gui::GUI_ENVIRONMENT_MONITORING_RETENTION.text(locale),
                gui::GUI_ENVIRONMENT_MONITORING_RETENTION_HELP.text(locale),
                div().flex_shrink_0().child(history),
            ))
    }

    fn save_monitoring(
        &mut self,
        key: &str,
        edit: impl FnOnce(&mut EnvironmentMonitoring),
        cx: &mut Context<Self>,
    ) {
        if key != self.active_environment.settings_key() {
            return;
        }
        let mut updated = self.monitoring();
        edit(&mut updated);
        self.save_settings(
            |settings| {
                if updated == EnvironmentMonitoring::default() {
                    settings.ui.environment_monitoring.remove(key);
                } else {
                    settings
                        .ui
                        .environment_monitoring
                        .insert(key.to_owned(), updated);
                }
            },
            cx,
        );
    }
}
