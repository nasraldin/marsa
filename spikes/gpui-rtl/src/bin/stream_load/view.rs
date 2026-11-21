//! Render both fixture streams together and measure the actual window cadence.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{
    Context, IntoElement, ParentElement as _, Render, ScrollStrategy, SharedString, Styled as _,
    UniformListScrollHandle, Window, div, px, uniform_list,
};

use super::metrics::FrameIntervals;
use super::workload::{BATCH_LINES, BYTE_CAP, History, LINE_CAP, LogLine, QUEUED_BATCHES, RATE};

const CONTAINERS: usize = 2000;
const SWEEP_STEP: usize = 8;
const WARMUP: Duration = Duration::from_secs(2);
const MAX_BATCHES_PER_RENDER: usize = 8;

struct Coverage {
    seen: [bool; CONTAINERS],
    peak_requested_rows: usize,
}

pub struct LoadView {
    seconds: usize,
    source: Receiver<Vec<LogLine>>,
    containers: Rc<[SharedString]>,
    history: Rc<RefCell<History>>,
    container_scroll: UniformListScrollHandle,
    log_scroll: UniformListScrollHandle,
    coverage: Rc<RefCell<Coverage>>,
    peak_log_rows: Rc<Cell<usize>>,
    latest_requested_log: Rc<Cell<usize>>,
    next_container: usize,
    started: Instant,
    last_frame: Option<Instant>,
    intervals: FrameIntervals,
}

impl LoadView {
    pub fn new(seconds: usize, source: Receiver<Vec<LogLine>>) -> Self {
        Self {
            seconds,
            source,
            containers: (0..CONTAINERS)
                .map(|id| {
                    format!(
                        "container-{id:04}   alpine:stable   running   127.0.0.1:{}",
                        3000 + id
                    )
                    .into()
                })
                .collect(),
            history: Rc::default(),
            container_scroll: UniformListScrollHandle::new(),
            log_scroll: UniformListScrollHandle::new(),
            coverage: Rc::new(RefCell::new(Coverage {
                seen: [false; CONTAINERS],
                peak_requested_rows: 0,
            })),
            peak_log_rows: Rc::default(),
            latest_requested_log: Rc::default(),
            next_container: 0,
            started: Instant::now(),
            last_frame: None,
            intervals: FrameIntervals::default(),
        }
    }

    fn drain(&self) -> bool {
        let mut history = self.history.borrow_mut();
        for _ in 0..MAX_BATCHES_PER_RENDER {
            match self.source.try_recv() {
                Ok(batch) => history.extend(batch),
                Err(TryRecvError::Empty) => return false,
                Err(TryRecvError::Disconnected) => return true,
            }
        }
        false
    }

    fn finish(&self, elapsed: Duration) -> Result<String, String> {
        let history = self.history.borrow();
        let coverage = self.coverage.borrow();
        let seen = coverage.seen.iter().filter(|&&row| row).count();
        let latest_requested_log = self.latest_requested_log.get();
        let expected = self.seconds * RATE;
        if history.accepted != expected
            || self.intervals.count == 0
            || seen != CONTAINERS
            || latest_requested_log != expected
            || history.peak_lines > LINE_CAP
            || history.peak_bytes > BYTE_CAP
            || coverage.peak_requested_rows > 64
            || self.peak_log_rows.get() > 64
        {
            return Err(format!(
                "fixture workload incomplete: accepted {}/{expected} logs, last requested log {latest_requested_log}, visited {seen}/{CONTAINERS} rows, max requested {} containers / {} logs, {} intervals",
                history.accepted,
                coverage.peak_requested_rows,
                self.peak_log_rows.get(),
                self.intervals.count,
            ));
        }
        Ok(format!(
            "seconds={:.3} log_lines={} rate={:.0} batch_lines={BATCH_LINES} queue_batches={QUEUED_BATCHES} max_batches_per_render={MAX_BATCHES_PER_RENDER}\ncontainer_rows={seen} max_container_range={} max_log_range={} last_requested_log={} retained_lines={} peak_retained_lines={} peak_payload_bytes={} line_cap={LINE_CAP} byte_cap={BYTE_CAP}\nwarmup_seconds={} frame_intervals={} p50_ms={:.1} p95_ms={:.1} p99_ms={:.1} max_ms={:.2} over_20ms={} over_33ms={}",
            elapsed.as_secs_f64(),
            history.accepted,
            history.accepted as f64 / elapsed.as_secs_f64(),
            coverage.peak_requested_rows,
            self.peak_log_rows.get(),
            latest_requested_log,
            history.lines.len(),
            history.peak_lines,
            history.peak_bytes,
            WARMUP.as_secs(),
            self.intervals.count,
            self.intervals.percentile_ms(50),
            self.intervals.percentile_ms(95),
            self.intervals.percentile_ms(99),
            self.intervals.max.as_secs_f64() * 1000.,
            self.intervals.over_20ms,
            self.intervals.over_33ms,
        ))
    }
}

impl Render for LoadView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = Instant::now();
        if let Some(previous) = self.last_frame.replace(now)
            && previous.duration_since(self.started) >= WARMUP
        {
            self.intervals.add(now.duration_since(previous));
        }
        let finished = self.drain();
        if finished && self.latest_requested_log.get() == self.history.borrow().accepted {
            let summary = self.finish(now.duration_since(self.started));
            cx.defer(move |cx| match summary {
                Ok(summary) => {
                    println!("{summary}");
                    cx.quit();
                }
                Err(error) => super::fail(&error),
            });
        } else if now.duration_since(self.started).as_secs() > self.seconds as u64 + 15 {
            cx.defer(|_| super::fail("fixture producer or frame delivery timed out"));
        } else {
            window.request_animation_frame();
        }

        self.container_scroll
            .scroll_to_item_strict(self.next_container, ScrollStrategy::Top);
        self.next_container = (self.next_container + SWEEP_STEP) % CONTAINERS;
        let log_count = self.history.borrow().lines.len();
        if let Some(last) = log_count.checked_sub(1) {
            self.log_scroll
                .scroll_to_item_strict(last, ScrollStrategy::Bottom);
        }

        let rows = Rc::clone(&self.containers);
        let coverage = Rc::clone(&self.coverage);
        let containers = uniform_list("containers", CONTAINERS, move |range, _, _| {
            let mut coverage = coverage.borrow_mut();
            coverage.peak_requested_rows = coverage.peak_requested_rows.max(range.len());
            range
                .map(|index| {
                    coverage.seen[index] = true;
                    line(rows[index].clone())
                })
                .collect()
        })
        .track_scroll(&self.container_scroll)
        .size_full();

        let history = Rc::clone(&self.history);
        let peak = Rc::clone(&self.peak_log_rows);
        let greatest = Rc::clone(&self.latest_requested_log);
        let logs = uniform_list("logs", log_count, move |range, _, _| {
            peak.set(peak.get().max(range.len()));
            let history = history.borrow();
            range
                .filter_map(|index| history.lines.get(index))
                .map(|entry| {
                    greatest.set(greatest.get().max(entry.sequence));
                    line(entry.text.clone())
                })
                .collect()
        })
        .track_scroll(&self.log_scroll)
        .size_full();

        div()
            .size_full()
            .flex()
            .gap_3()
            .p_3()
            .text_color(cx.theme().foreground)
            .bg(cx.theme().background)
            .child(
                div()
                    .w(px(450.))
                    .h_full()
                    .overflow_hidden()
                    .child(containers),
            )
            .child(div().flex_1().h_full().overflow_hidden().child(logs))
    }
}

fn line(text: SharedString) -> impl IntoElement {
    div()
        .h(px(21.))
        .text_size(px(12.))
        .whitespace_nowrap()
        .overflow_hidden()
        .child(text)
}
