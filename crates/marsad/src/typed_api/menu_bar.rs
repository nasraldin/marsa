//! Project the daemon's current state into the macOS menu bar.

use marsa_core::settings::Language;
use marsa_proto::host::v1::{EngineState, daemon_event::Body};
use tokio::sync::watch;
use tonic::Status;

use super::{TypedApi, engine::Action};

/// Owner state that the menu bar can display without receiving approval details.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MenuBarState {
    /// Engine lifecycle state.
    pub engine: EngineState,
    /// Current desktop language preference.
    pub language: Language,
    /// Number of requests waiting for approval.
    pub pending_approvals: usize,
}

impl MenuBarState {
    pub(super) fn apply(&mut self, event: &Body, language: Language) -> bool {
        let previous = *self;
        match event {
            Body::Engine(status) => self.engine = status.state(),
            Body::Approvals(pending) => self.pending_approvals = pending.approvals.len(),
            Body::Settings(_) => self.language = language,
            _ => return false,
        }
        *self != previous
    }
}

/// Engine commands available to the owner from the menu bar.
#[derive(Debug, Clone, Copy)]
pub enum MenuBarEngine {
    /// Start the packaged VM.
    Start,
    /// Stop the packaged VM.
    Stop,
}

impl TypedApi {
    /// Subscribe to the latest menu bar state and future relevant daemon changes.
    pub fn watch_menu_bar(&self) -> watch::Receiver<MenuBarState> {
        self.inner.events.watch_menu_bar()
    }

    /// Dispatch a command selected in the daemon's own macOS menu bar.
    pub async fn menu_bar_engine(&self, command: MenuBarEngine) -> Result<(), Status> {
        if *self.inner.closing.borrow() {
            return Err(Status::unavailable("daemon is shutting down"));
        }
        let action = match command {
            MenuBarEngine::Start => Action::Start,
            MenuBarEngine::Stop => Action::Stop,
        };
        self.perform_engine_action(action).await
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use marsa_core::settings::Settings;
    use marsa_proto::host::v1::PendingApproval;
    use tokio::time::Instant;

    use super::{super::events::Bus, *};
    use crate::supervisor::{State, Transition};

    #[tokio::test]
    async fn watch_reports_engine_approvals_and_language_from_live_owner_events() {
        let mut settings = Settings::default();
        let bus = Bus::new(settings.clone()).unwrap();
        let mut menu = bus.watch_menu_bar();
        assert_eq!(menu.borrow().engine, EngineState::Stopped);
        assert_eq!(menu.borrow().pending_approvals, 0);

        bus.engine(&Transition {
            previous: State::Stopped,
            current: State::Starting,
            failure: None,
            network: None,
        });
        menu.changed().await.unwrap();
        assert_eq!(menu.borrow().engine, EngineState::Starting);

        let (id, response) = bus
            .request_approval(
                PendingApproval::default(),
                Instant::now() + Duration::from_secs(5),
            )
            .unwrap();
        menu.changed().await.unwrap();
        assert_eq!(menu.borrow().pending_approvals, 1);
        bus.discard_approval(&id);
        menu.changed().await.unwrap();
        assert_eq!(menu.borrow().pending_approvals, 0);
        assert!(response.await.is_err());

        settings.ui.language = Language::Arabic;
        bus.update_settings(settings.clone(), settings.to_toml().unwrap(), 2, true);
        assert_eq!(menu.borrow_and_update().language, Language::Arabic);
        assert_eq!(bus.watch_menu_bar().borrow().language, Language::Arabic);
    }
}
