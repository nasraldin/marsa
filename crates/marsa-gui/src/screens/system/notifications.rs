//! Redact and deduplicate daemon alerts before publishing to Notification Center.

use std::collections::BTreeSet;

use marsa_client::api::{DaemonSnapshot, EngineState, NetworkMode};
use marsa_ui::{
    Locale,
    gpui::{App, SystemNotification, SystemNotificationAction, SystemNotificationResponse},
    gui_keys as gui,
    preferences::NotificationSettings,
};

const ACTION_REVIEW: &str = "review-in-marsa";

#[derive(Clone, Copy)]
pub(super) enum Kind {
    Engine,
    Update,
    Approval,
    Network,
}

impl Kind {
    pub(super) const ALL: [Self; 4] = [Self::Engine, Self::Update, Self::Approval, Self::Network];

    const fn tag(self) -> &'static str {
        match self {
            Self::Engine => "marsa-engine-failure",
            Self::Update => "marsa-update-available",
            Self::Approval => "marsa-approval-pending",
            Self::Network => "marsa-network-mode-changed",
        }
    }

    pub(super) fn title(self, locale: Locale) -> &'static str {
        match self {
            Self::Engine => gui::GUI_DESKTOP_NOTIFICATION_ENGINE_TITLE.text(locale),
            Self::Update => gui::GUI_DESKTOP_NOTIFICATION_UPDATE_TITLE.text(locale),
            Self::Approval => gui::GUI_DESKTOP_NOTIFICATION_APPROVAL_TITLE.text(locale),
            Self::Network => gui::GUI_DESKTOP_NOTIFICATION_NETWORK_TITLE.text(locale),
        }
    }

    fn body(self, locale: Locale) -> &'static str {
        match self {
            Self::Engine => gui::GUI_DESKTOP_NOTIFICATION_ENGINE_BODY.text(locale),
            Self::Update => gui::GUI_DESKTOP_NOTIFICATION_UPDATE_BODY.text(locale),
            Self::Approval => gui::GUI_DESKTOP_NOTIFICATION_APPROVAL_BODY.text(locale),
            Self::Network => gui::GUI_DESKTOP_NOTIFICATION_NETWORK_BODY.text(locale),
        }
    }

    pub(super) fn enabled(self, settings: NotificationSettings) -> bool {
        match self {
            Self::Engine => settings.engine_failure,
            Self::Update => settings.update_available,
            Self::Approval => settings.approval_pending,
            Self::Network => settings.network_mode_changed,
        }
    }

    pub(super) fn set(self, settings: &mut NotificationSettings, enabled: bool) {
        match self {
            Self::Engine => settings.engine_failure = enabled,
            Self::Update => settings.update_available = enabled,
            Self::Approval => settings.approval_pending = enabled,
            Self::Network => settings.network_mode_changed = enabled,
        }
    }

    const fn index(self) -> usize {
        self as usize
    }
}

/// Review-only destinations for activations originating in Notification Center.
pub(crate) enum NotificationDestination {
    Machine,
    Updates,
    Activity,
    Network,
}

pub(crate) fn notification_destination(
    response: &SystemNotificationResponse,
) -> Option<NotificationDestination> {
    if response
        .action_id
        .as_deref()
        .is_some_and(|id| id != ACTION_REVIEW)
    {
        return None;
    }
    match Kind::ALL
        .into_iter()
        .find(|kind| kind.tag() == response.tag.as_ref())?
    {
        Kind::Engine => Some(NotificationDestination::Machine),
        Kind::Update => Some(NotificationDestination::Updates),
        Kind::Approval => Some(NotificationDestination::Activity),
        Kind::Network => Some(NotificationDestination::Network),
    }
}

#[derive(Default)]
struct Observed {
    failed: bool,
    update: Option<String>,
    pending: BTreeSet<(bool, String)>,
    mode: Option<NetworkMode>,
}

pub(crate) struct Notifications {
    locale: Locale,
    preferences: NotificationSettings,
    observed: Observed,
    posted: [bool; 4],
}

impl Default for Notifications {
    fn default() -> Self {
        Self {
            locale: Locale::EnglishUs,
            preferences: NotificationSettings::default(),
            observed: Observed::default(),
            posted: [false; 4],
        }
    }
}

impl Notifications {
    pub(crate) fn set_locale(&mut self, locale: Locale) {
        self.locale = locale;
    }

    pub(crate) fn set_preferences(&mut self, settings: NotificationSettings, cx: &App) {
        let previous = std::mem::replace(&mut self.preferences, settings);
        for kind in Kind::ALL {
            if !kind.enabled(settings) {
                self.dismiss(kind, cx);
            } else if !kind.enabled(previous) && self.present(kind) {
                self.post(kind, cx);
            }
        }
    }

    pub(crate) fn observe(&mut self, snapshot: &DaemonSnapshot, cx: &App) {
        let pending = snapshot
            .approvals
            .iter()
            .flat_map(|list| &list.approvals)
            .map(|item| (false, item.id.clone()))
            .chain(
                snapshot
                    .bind_approvals
                    .iter()
                    .flat_map(|list| &list.approvals)
                    .map(|item| (true, item.id.clone())),
            )
            .collect::<BTreeSet<_>>();
        let next = Observed {
            failed: snapshot
                .engine
                .as_ref()
                .is_some_and(|engine| engine.state() == EngineState::Failed),
            update: snapshot
                .update
                .as_ref()
                .and_then(|update| update.available_version.clone()),
            pending,
            mode: snapshot
                .network
                .as_ref()
                .map(|network| network.active_mode())
                .filter(|mode| matches!(mode, NetworkMode::Nat | NetworkMode::Userspace))
                .or(self.observed.mode),
        };
        let previous = std::mem::replace(&mut self.observed, next);
        let observed = &self.observed;
        let changes = [
            observed.failed && !previous.failed,
            observed.update.is_some() && observed.update != previous.update,
            observed
                .pending
                .iter()
                .any(|item| !previous.pending.contains(item)),
            previous.mode.is_some() && observed.mode != previous.mode,
        ];

        for (kind, changed) in Kind::ALL.into_iter().zip(changes) {
            if changed && kind.enabled(self.preferences) {
                self.post(kind, cx);
            } else if !matches!(kind, Kind::Network) && !self.present(kind) {
                self.dismiss(kind, cx);
            }
        }
    }

    fn present(&self, kind: Kind) -> bool {
        match kind {
            Kind::Engine => self.observed.failed,
            Kind::Update => self.observed.update.is_some(),
            Kind::Approval => !self.observed.pending.is_empty(),
            Kind::Network => false,
        }
    }

    fn post(&mut self, kind: Kind, cx: &App) {
        // Use only fixed localized strings. Approval identifiers, arguments,
        // client names and paths stay in the authenticated desktop review.
        cx.show_system_notification(SystemNotification {
            tag: kind.tag().into(),
            title: kind.title(self.locale).into(),
            body: kind.body(self.locale).into(),
            actions: vec![SystemNotificationAction {
                id: ACTION_REVIEW.into(),
                label: gui::GUI_DESKTOP_NOTIFICATION_REVIEW
                    .text(self.locale)
                    .into(),
            }],
        });
        self.posted[kind.index()] = true;
    }

    fn dismiss(&mut self, kind: Kind, cx: &App) {
        if std::mem::take(&mut self.posted[kind.index()]) {
            drop(cx.dismiss_system_notification(kind.tag()));
        }
    }
}
