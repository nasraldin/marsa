//! Session ownership and execution of configured cleanup reviews.

use std::{
    collections::BTreeMap,
    time::{Duration, SystemTime},
};

use marsa_client::{
    api::{PrunePreviewRequest, PrunePreviewResponse},
    docker::{ManagedEndpointStore, ScheduledCleanup},
};
use marsa_ui::{
    gpui::{Context, Task},
    preferences::{MaintenanceReview, ReviewTarget},
};

use super::{SystemView, disk::cleanup_request};

const TICK: Duration = Duration::from_secs(15);

pub(super) enum ReviewOutcome {
    Offline,
    Skipped,
    Failed,
    Reviewed(PrunePreviewResponse),
}

impl From<ScheduledCleanup> for ReviewOutcome {
    fn from(result: ScheduledCleanup) -> Self {
        match result {
            ScheduledCleanup::Offline => Self::Offline,
            ScheduledCleanup::Skipped => Self::Skipped,
            ScheduledCleanup::Reviewed(preview) => Self::Reviewed(preview),
        }
    }
}

struct ReviewRun {
    key: String,
    target: ReviewTarget,
    generation: u64,
}

pub(super) struct ReviewJob {
    pub(super) schedule: MaintenanceReview,
    pub(super) next_due: SystemTime,
    pub(super) running: bool,
    pub(super) last: Option<(SystemTime, ReviewOutcome)>,
    generation: u64,
}

#[derive(Default)]
pub(super) struct ReviewScheduler {
    pub(super) jobs: BTreeMap<String, ReviewJob>,
    generation: u64,
    task: Option<Task<()>>,
}

impl ReviewScheduler {
    fn reconcile(&mut self, configured: &BTreeMap<String, MaintenanceReview>, now: SystemTime) {
        self.jobs.retain(|key, _| configured.contains_key(key));
        for (key, schedule) in configured {
            if self
                .jobs
                .get(key)
                .is_some_and(|job| job.schedule == *schedule)
            {
                continue;
            }
            self.generation = self.generation.wrapping_add(1);
            self.jobs.insert(
                key.clone(),
                ReviewJob {
                    schedule: schedule.clone(),
                    next_due: now + schedule.cadence.interval(),
                    running: false,
                    last: None,
                    generation: self.generation,
                },
            );
        }
    }

    fn take_due(&mut self, now: SystemTime) -> Option<ReviewRun> {
        let (key, job) = self
            .jobs
            .iter_mut()
            .filter(|(_, job)| !job.running && job.next_due <= now)
            .min_by_key(|(_, job)| job.next_due)?;
        job.running = true;
        job.next_due = now + job.schedule.cadence.interval();
        Some(ReviewRun {
            key: key.clone(),
            target: job.schedule.target.clone(),
            generation: job.generation,
        })
    }

    fn finish(&mut self, run: ReviewRun, outcome: ReviewOutcome, now: SystemTime) {
        if let Some(job) = self.jobs.get_mut(&run.key)
            && job.generation == run.generation
        {
            job.running = false;
            job.last = Some((now, outcome));
        }
    }
}

impl SystemView {
    pub(super) fn reconcile_maintenance_reviews(&mut self, cx: &mut Context<Self>) {
        let (Some((settings, _)), Some(services)) = (&self.settings, &self.services) else {
            return;
        };
        self.maintenance
            .reconcile(&settings.ui.maintenance_reviews, SystemTime::now());
        if self.maintenance.jobs.is_empty() {
            self.maintenance.task = None;
            return;
        }
        if self.maintenance.task.is_some() {
            return;
        }

        let services = services.clone();
        let timer = cx.background_executor().clone();
        self.maintenance.task = Some(cx.spawn(async move |this, cx| {
            loop {
                let Ok(due) = this.update(cx, |this, cx| {
                    let run = this.maintenance.take_due(SystemTime::now())?;
                    let volumes = this
                        .settings
                        .as_ref()
                        .and_then(|(settings, _)| settings.ui.environment_monitoring.get(&run.key))
                        .is_some_and(|options| options.show_volume_candidates);
                    cx.notify();
                    Some((run, volumes, this.disk_engine_ready))
                }) else {
                    break;
                };
                let Some((run, volumes, builtin_ready)) = due else {
                    timer.timer(TICK).await;
                    continue;
                };
                let request = cleanup_request(volumes);
                let outcome = match &run.target {
                    ReviewTarget::Builtin if !builtin_ready => ReviewOutcome::Offline,
                    ReviewTarget::Builtin => services
                        .scheduled_builtin_cleanup(request)
                        .result()
                        .await
                        .and_then(Result::ok)
                        .map_or(ReviewOutcome::Failed, ReviewOutcome::from),
                    ReviewTarget::ManagedUnix { socket, engine_id } => {
                        let (key, socket, engine_id) =
                            (run.key.clone(), socket.clone(), engine_id.clone());
                        services
                            .pending(async move {
                                Ok(run_external(key, socket, engine_id, request).await)
                            })
                            .result()
                            .await
                            .and_then(Result::ok)
                            .unwrap_or(ReviewOutcome::Failed)
                    }
                };
                if this
                    .update(cx, |this, cx| {
                        this.maintenance.finish(run, outcome, SystemTime::now());
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        }));
    }
}

async fn run_external(
    key: String,
    socket: std::path::PathBuf,
    engine_id: String,
    request: PrunePreviewRequest,
) -> ReviewOutcome {
    let Ok(store) = ManagedEndpointStore::discover() else {
        return ReviewOutcome::Failed;
    };
    store
        .scheduled_cleanup(&key, &socket, &engine_id, request)
        .await
        .map_or(ReviewOutcome::Failed, ReviewOutcome::from)
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        time::{Duration, UNIX_EPOCH},
    };

    use marsa_ui::preferences::{MaintenanceReview, ReviewCadence, ReviewTarget};

    use super::*;

    #[test]
    fn edits_reset_only_the_edited_schedule_and_drop_stale_review_results() {
        let start = UNIX_EPOCH + Duration::from_secs(1_000);
        let hourly = ReviewCadence::Hourly.interval();
        let daily = ReviewCadence::Daily.interval();
        let a = "marsa";
        let b = "managed:ec12316b-2ae3-4a50-a253-f04962f76a8c";
        let local = MaintenanceReview {
            cadence: ReviewCadence::Hourly,
            target: ReviewTarget::Builtin,
        };
        let external = MaintenanceReview {
            cadence: ReviewCadence::Hourly,
            target: ReviewTarget::ManagedUnix {
                socket: "/private/pinned/docker.sock".into(),
                engine_id: "pinned-engine".into(),
            },
        };
        let mut configured = BTreeMap::from([(a.to_owned(), local.clone())]);
        let mut scheduler = ReviewScheduler::default();
        scheduler.reconcile(&configured, start);
        configured.insert(b.to_owned(), external.clone());
        let offset = Duration::from_secs(1);
        scheduler.reconcile(&configured, start + offset);
        assert!(scheduler.take_due(start).is_none());

        let due = start + hourly;
        let local_run = scheduler.take_due(due).unwrap();
        assert_eq!(local_run.key, a);
        assert_eq!(local_run.target, local.target);

        configured.get_mut(a).unwrap().cadence = ReviewCadence::Daily;
        scheduler.reconcile(&configured, due);
        scheduler.finish(local_run, ReviewOutcome::Failed, due);

        assert!(scheduler.jobs[a].last.is_none());
        assert_eq!(scheduler.jobs[a].next_due, due + daily);
        let external_run = scheduler.take_due(due + offset).unwrap();
        assert_eq!(external_run.key, b);
        assert_eq!(external_run.target, external.target);
        scheduler.finish(external_run, ReviewOutcome::Offline, due);
        assert!(matches!(
            scheduler.jobs[b].last,
            Some((_, ReviewOutcome::Offline))
        ));
        assert!(scheduler.take_due(due).is_none());
    }

    #[test]
    fn late_wake_runs_once_and_reopening_starts_a_new_interval() {
        let start = UNIX_EPOCH + Duration::from_secs(1_000);
        let hourly = ReviewCadence::Hourly.interval();
        let now = start + hourly * 5;
        let key = "marsa";
        let configured = BTreeMap::from([(
            key.to_owned(),
            MaintenanceReview {
                cadence: ReviewCadence::Hourly,
                target: ReviewTarget::Builtin,
            },
        )]);
        let mut scheduler = ReviewScheduler::default();
        scheduler.reconcile(&configured, start);
        let run = scheduler.take_due(now).unwrap();
        assert!(scheduler.take_due(now).is_none());
        scheduler.finish(run, ReviewOutcome::Skipped, now);
        assert!(scheduler.take_due(now).is_none());
        assert_eq!(scheduler.jobs[key].next_due, now + hourly);

        let mut reopened = ReviewScheduler::default();
        reopened.reconcile(&configured, now);
        assert!(reopened.jobs[key].last.is_none());
        assert!(reopened.take_due(now).is_none());
        assert!(reopened.take_due(now + hourly).is_some());
    }
}
