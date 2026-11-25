//! Local opt-in container health alerts for the window's selected engine.

use std::time::{Duration, SystemTime};

use marsa_client::{api::EngineState, docker::ExternalEndpoint};
use marsa_ui::{
    gpui::{App, Context, SystemNotification, Task},
    gui_keys as gui,
};

use super::Desktop;

const TAG: &str = "marsa-container-health";
const POLL_INTERVAL: Duration = Duration::from_secs(15);

#[derive(Clone, Eq, PartialEq)]
enum Engine {
    Marsa,
    External(Box<ExternalEndpoint>),
}

#[derive(Default)]
pub(super) struct HealthAlerts {
    engine: Option<Engine>,
    task: Option<Task<()>>,
    posted: bool,
}

impl Desktop {
    fn current_health_engine(&self, cx: &App) -> Option<Engine> {
        let monitoring = self.system.read(cx).monitoring();
        if self.onboarding.is_some() || !monitoring.collect_activity || !monitoring.notify_unhealthy
        {
            return None;
        }
        if self.environment.read(cx).includes_marsa() {
            matches!(self.engine(), Some(EngineState::Ready | EngineState::Idle))
                .then_some(Engine::Marsa)
        } else {
            self.active_workspace(cx)
                .map(|workspace| Engine::External(Box::new(workspace.endpoint.clone())))
        }
    }

    pub(super) fn monitor_environment_health(&mut self, cx: &mut Context<Self>) {
        let engine = self.current_health_engine(cx);
        if self.health_alerts.engine == engine {
            return;
        }
        if self.health_alerts.posted {
            drop(cx.dismiss_system_notification(TAG));
        }
        self.health_alerts = HealthAlerts {
            engine: engine.clone(),
            ..HealthAlerts::default()
        };
        let Some((engine, services)) = engine.zip(self.services.clone()) else {
            return;
        };
        let endpoint = match &engine {
            Engine::Marsa => None,
            Engine::External(endpoint) => Some(endpoint.as_ref().clone()),
        };
        let timer = cx.background_executor().clone();
        self.health_alerts.task = Some(cx.spawn(async move |this, cx| {
            let mut cursor = HealthCursor(unix_nanos());
            loop {
                if let Some(Ok(events)) =
                    services.engine_activity(endpoint.clone(), 1).result().await
                    && cursor.observe(
                        events
                            .iter()
                            .map(|event| (event.timestamp_nanos(), event.is_unhealthy())),
                        unix_nanos(),
                    )
                    && this
                        .update(cx, |this, cx| {
                            if this.current_health_engine(cx).as_ref() != Some(&engine) {
                                return false;
                            }
                            cx.show_system_notification(SystemNotification {
                                tag: TAG.into(),
                                title: gui::GUI_ENVIRONMENT_MONITORING_NOTIFICATION_TITLE
                                    .text(this.locale)
                                    .into(),
                                body: gui::GUI_ENVIRONMENT_MONITORING_NOTIFICATION_BODY
                                    .text(this.locale)
                                    .into(),
                                actions: Vec::new(),
                            });
                            this.health_alerts.posted = true;
                            true
                        })
                        .ok()
                        != Some(true)
                {
                    return;
                }
                timer.timer(POLL_INTERVAL).await;
            }
        }));
    }
}

struct HealthCursor(i64);

impl HealthCursor {
    fn observe(&mut self, events: impl IntoIterator<Item = (i64, bool)>, now: i64) -> bool {
        let previous = self.0;
        let mut alert = false;
        for (observed, unhealthy) in events {
            if observed > previous && observed <= now {
                self.0 = self.0.max(observed);
                alert |= unhealthy;
            }
        }
        alert
    }
}

fn unix_nanos() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |time| i64::try_from(time.as_nanos()).unwrap_or(i64::MAX))
}

#[cfg(test)]
mod tests {
    use super::HealthCursor;

    #[test]
    fn health_notifications_ignore_history_duplicates_and_future_entries() {
        let mut alpha = HealthCursor(30);
        assert!(!alpha.observe([(20, true), (40, false), (200, true)], 50));
        assert!(alpha.observe([(61, false), (60, true)], 65));
        assert!(!alpha.observe([(60, true), (61, false)], 65));

        let mut beta = HealthCursor(66);
        assert!(!beta.observe([(60, true), (65, true)], 70));
        assert!(beta.observe([(80, true)], 81));
    }
}
