//! App-owned daemon subscription and notification routing across window lifetimes.

use std::sync::Arc;

use marsa_client::api::GetSettingsRequest;
use marsa_ui::{
    Locale,
    gpui::{
        AnyWindowHandle, App, AppContext, BorrowAppContext, Context, Global, Task, WeakEntity,
        Window,
    },
    preferences::{Language, Settings},
};

use super::{Desktop, bind_keys};
use crate::{
    screens::system::{Notifications, notification_destination},
    services::Services,
};

struct Session {
    _services: Option<Arc<Services>>,
    notifications: Notifications,
    _events: Option<Task<()>>,
}

impl Global for Session {}

#[derive(Clone)]
struct Workspace {
    window: AnyWindowHandle,
    desktop: WeakEntity<Desktop>,
}

impl Global for Workspace {}

pub(super) fn attach(
    window: &Window,
    locale: Locale,
    cx: &mut Context<Desktop>,
) -> Option<Arc<Services>> {
    let workspace = Workspace {
        window: window.window_handle(),
        desktop: cx.weak_entity(),
    };
    cx.set_global(workspace);

    if !cx.has_global::<Session>() {
        bind_keys(cx);
        let services = Services::start_notifications().ok().map(Arc::new);
        let mut notifications = Notifications::default();
        notifications.set_locale(locale);
        let events = services
            .as_ref()
            .map(|services| watch(Arc::clone(services), locale, cx));
        cx.set_global(Session {
            _services: services,
            notifications,
            _events: events,
        });

        cx.on_system_notification_response(|response, cx| {
            let Some(destination) = notification_destination(&response) else {
                return;
            };
            if marsa_ui::show_workspace(cx).is_err() {
                return;
            }
            let Some(workspace) = cx.try_global::<Workspace>().cloned() else {
                return;
            };
            let _ = cx.update_window(workspace.window, |_, _, cx| {
                let _ = workspace.desktop.update(cx, |this, cx| {
                    let route = this.system.update(cx, |view, _| {
                        view.prepare_notification_destination(destination)
                    });
                    this.navigate(route, cx);
                });
            });
        });
    }
    let services = Arc::new(Services::start().ok()?);
    let window_services = Arc::downgrade(&services);
    cx.on_release(move |_, _| {
        if let Some(services) = window_services.upgrade() {
            services.close();
        }
    })
    .detach();
    Some(services)
}

fn watch(services: Arc<Services>, system_locale: Locale, cx: &App) -> Task<()> {
    let mut selection = services.store().select(|state| state.daemon().cloned());
    cx.spawn(async move |cx| {
        let mut snapshot = selection.current().clone();
        let mut revision = None;
        loop {
            if let Some(snapshot) = &snapshot {
                cx.update(|cx| {
                    cx.update_global::<Session, _>(|session, cx| {
                        session.notifications.observe(snapshot, cx);
                    })
                });
                if revision != Some(snapshot.settings_revision) {
                    revision = Some(snapshot.settings_revision);
                    let mut request = services.request(|client| async move {
                        Ok(client
                            .settings()
                            .get(GetSettingsRequest {})
                            .await?
                            .into_inner())
                    });
                    if let Some(Ok(settings)) = request.result().await
                        && let Ok(settings) = Settings::from_toml(&settings.settings_toml)
                    {
                        let locale = match settings.ui.language {
                            Language::System => system_locale,
                            Language::English => Locale::EnglishUs,
                            Language::Arabic => Locale::Arabic,
                        };
                        cx.update(|cx| {
                            cx.update_global::<Session, _>(|session, cx| {
                                session.notifications.set_locale(locale);
                                session
                                    .notifications
                                    .set_preferences(settings.ui.notifications, cx);
                            })
                        });
                    }
                }
            } else {
                revision = None;
            }
            let Some(next) = selection.changed().await else {
                break;
            };
            snapshot = next.clone();
        }
    })
}
