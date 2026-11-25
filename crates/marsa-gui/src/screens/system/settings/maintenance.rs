//! Show and edit the current environment's scheduled maintenance review.

use std::time::{SystemTime, UNIX_EPOCH};

use marsa_client::api::PruneApplyCapability;
use marsa_ui::{
    ColorToken, TextKey,
    direction::POPOVER_TOP_START,
    gpui::{Context, Div, ParentElement as _, Styled as _, div},
    gpui_component::{
        Disableable as _,
        button::Button,
        menu::{DropdownMenu as _, PopupMenuItem},
    },
    gui_keys as gui,
    locale_format::date_time_utc,
    preferences::{MaintenanceReview, ReviewCadence, ReviewTarget},
    tokens::typography::Role,
};

use super::{SystemView, setting_row, settings_text};
use crate::screens::{
    common::{ScreenContext, row},
    system::{
        disk::{ordered_candidates, show_cleanup_review},
        maintenance::{ReviewJob, ReviewOutcome},
    },
};

const CHOICES: [(Option<ReviewCadence>, TextKey); 4] = [
    (None, gui::GUI_MAINTENANCE_SCHEDULE_OFF),
    (
        Some(ReviewCadence::Hourly),
        gui::GUI_MAINTENANCE_SCHEDULE_HOURLY,
    ),
    (
        Some(ReviewCadence::Daily),
        gui::GUI_MAINTENANCE_SCHEDULE_DAILY,
    ),
    (
        Some(ReviewCadence::Weekly),
        gui::GUI_MAINTENANCE_SCHEDULE_WEEKLY,
    ),
];

impl SystemView {
    pub(super) fn maintenance_schedule(
        &self,
        screen: &ScreenContext<'_>,
        cx: &Context<Self>,
    ) -> Div {
        let locale = screen.locale;
        let key = self.active_environment.settings_key();
        let job = self.maintenance.jobs.get(&key);
        let selected = job.map(|job| job.schedule.cadence);
        let available = self.maintenance_target();
        let changed = job.is_some_and(|job| {
            available
                .as_ref()
                .is_some_and(|current| current != &job.schedule.target)
        });
        let target = job.map(|job| job.schedule.target.clone()).or(available);
        let help = match (target.is_some(), changed) {
            (_, true) => gui::GUI_MAINTENANCE_SCHEDULE_REBIND,
            (true, false) => gui::GUI_MAINTENANCE_SCHEDULE_HELP,
            (false, false) => gui::GUI_MAINTENANCE_SCHEDULE_UNSUPPORTED,
        };
        let title = gui::GUI_MAINTENANCE_SCHEDULE_TITLE.text(locale);
        let current = CHOICES
            .iter()
            .find(|(value, _)| *value == selected)
            .map_or(gui::GUI_MAINTENANCE_SCHEDULE_OFF, |(_, label)| *label);
        let view = cx.weak_entity();
        let menu_key = key.clone();
        let picker = Button::new("maintenance-review-cadence")
            .outline()
            .label(current.text(locale))
            .accessibility_label(title)
            .dropdown_caret(true)
            .disabled(self.busy() || self.settings.is_none() || target.is_none())
            .dropdown_menu_with_anchor(POPOVER_TOP_START, move |mut popup, _, _| {
                for (cadence, label) in CHOICES {
                    let view = view.clone();
                    let key = menu_key.clone();
                    let target = target.clone();
                    popup = popup.item(
                        PopupMenuItem::new(label.text(locale))
                            .checked(cadence == selected)
                            .disabled(changed && cadence.is_some())
                            .on_click(move |_, _, cx| {
                                if cadence != selected {
                                    let _ = view.update(cx, |this, cx| {
                                        this.save_maintenance_schedule(
                                            &key,
                                            cadence,
                                            target.clone(),
                                            cx,
                                        );
                                    });
                                }
                            }),
                    );
                }
                popup
            });
        div()
            .child(setting_row(
                screen,
                title,
                help.text(locale),
                div().flex_shrink_0().child(picker),
            ))
            .children(job.map(|job| self.maintenance_result(screen, job, key, changed, cx)))
    }

    fn maintenance_result(
        &self,
        screen: &ScreenContext<'_>,
        job: &ReviewJob,
        key: String,
        changed: bool,
        cx: &Context<Self>,
    ) -> Div {
        let locale = screen.locale;
        let palette = screen.palette;
        let view = cx.weak_entity();
        let status = match job.last.as_ref().map(|(_, outcome)| outcome) {
            None if job.running => gui::GUI_MAINTENANCE_REVIEW_RUNNING,
            None => gui::GUI_MAINTENANCE_REVIEW_WAITING,
            Some(ReviewOutcome::Offline) => gui::GUI_MAINTENANCE_REVIEW_OFFLINE,
            Some(ReviewOutcome::Skipped) => gui::GUI_MAINTENANCE_REVIEW_SKIPPED,
            Some(ReviewOutcome::Failed) => gui::GUI_MAINTENANCE_REVIEW_FAILED,
            Some(ReviewOutcome::Reviewed(review))
                if ordered_candidates(review, self.monitoring().show_volume_candidates)
                    .next()
                    .is_some() =>
            {
                gui::GUI_MAINTENANCE_REVIEW_READY
            }
            Some(ReviewOutcome::Reviewed(_)) => gui::GUI_MAINTENANCE_REVIEW_EMPTY,
        };
        let timestamp = |at: SystemTime| {
            let seconds = i64::try_from(at.duration_since(UNIX_EPOCH).ok()?.as_secs()).ok()?;
            date_time_utc(locale, screen.display.time_format, seconds)
        };
        div()
            .flex()
            .flex_col()
            .gap_2()
            .children((job.running && job.last.is_some()).then(|| {
                settings_text(
                    screen,
                    Role::Small,
                    gui::GUI_MAINTENANCE_REVIEW_RUNNING.text(locale),
                    ColorToken::TextMuted,
                )
            }))
            .child(
                row(locale)
                    .flex_wrap()
                    .gap_2()
                    .child(settings_text(
                        screen,
                        Role::Small,
                        status.text(locale),
                        ColorToken::TextPrimary,
                    ))
                    .children(
                        job.last
                            .as_ref()
                            .and_then(|(at, _)| timestamp(*at))
                            .map(|time| {
                                settings_text(screen, Role::Small, time, ColorToken::TextMuted)
                            }),
                    ),
            )
            .child(
                row(locale)
                    .flex_wrap()
                    .gap_2()
                    .child(settings_text(
                        screen,
                        Role::Small,
                        gui::GUI_MAINTENANCE_REVIEW_NEXT.text(locale),
                        ColorToken::TextMuted,
                    ))
                    .children(timestamp(job.next_due).map(|time| {
                        settings_text(screen, Role::Small, time, ColorToken::TextPrimary)
                    })),
            )
            .children(
                (!changed && status == gui::GUI_MAINTENANCE_REVIEW_READY).then(|| {
                    let label = gui::GUI_SYSTEM_DISK_REVIEW.text(locale);
                    row(locale).child(
                        Button::new("maintenance-open-latest-review")
                            .outline()
                            .label(label)
                            .on_click(move |_, window, cx| {
                                let _ = view.update(cx, |this, cx| {
                                    if this.active_environment.settings_key() != key {
                                        return;
                                    }
                                    let Some(job) = this.maintenance.jobs.get(&key) else {
                                        return;
                                    };
                                    let Some((_, ReviewOutcome::Reviewed(review))) = &job.last
                                    else {
                                        return;
                                    };
                                    if this
                                        .maintenance_target()
                                        .is_some_and(|current| current != job.schedule.target)
                                    {
                                        return;
                                    }
                                    let target = match &job.schedule.target {
                                        ReviewTarget::Builtin => None,
                                        ReviewTarget::ManagedUnix { socket, .. } => Some((
                                            this.active_environment.name().to_owned(),
                                            socket.to_string_lossy().into_owned(),
                                        )),
                                    };
                                    let candidates = ordered_candidates(
                                        review,
                                        this.monitoring().show_volume_candidates,
                                    )
                                    .cloned()
                                    .collect::<Vec<_>>();
                                    show_cleanup_review(
                                        locale,
                                        palette,
                                        target,
                                        PruneApplyCapability::ExactDeletionUnsupported,
                                        &candidates,
                                        window,
                                        cx,
                                    );
                                });
                            }),
                    )
                }),
            )
    }

    fn maintenance_target(&self) -> Option<ReviewTarget> {
        if self.active_environment.includes_marsa() {
            return Some(ReviewTarget::Builtin);
        }
        let engine = self
            .external_inventory
            .iter()
            .find(|workspace| self.active_environment.includes_external(workspace))?;
        engine.endpoint.managed_id()?;
        Some(ReviewTarget::ManagedUnix {
            socket: engine.endpoint.socket()?.to_path_buf(),
            engine_id: engine.verified_engine.as_ref()?.id.clone(),
        })
    }

    fn save_maintenance_schedule(
        &mut self,
        key: &str,
        cadence: Option<ReviewCadence>,
        target: Option<ReviewTarget>,
        cx: &mut Context<Self>,
    ) {
        if self.active_environment.settings_key() != key {
            return;
        }
        let schedule = match (cadence, target) {
            (None, _) => None,
            (Some(cadence), Some(target)) => {
                let available = self.maintenance_target();
                let valid = match available.as_ref() {
                    Some(current) => current == &target,
                    None => self
                        .maintenance
                        .jobs
                        .get(key)
                        .is_some_and(|job| job.schedule.target == target),
                };
                if !valid {
                    return;
                }
                Some(MaintenanceReview { cadence, target })
            }
            (Some(_), None) => return,
        };
        self.save_settings(
            |settings| {
                if let Some(schedule) = schedule {
                    settings
                        .ui
                        .maintenance_reviews
                        .insert(key.to_owned(), schedule);
                } else {
                    settings.ui.maintenance_reviews.remove(key);
                }
            },
            cx,
        );
    }
}
