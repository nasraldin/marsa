//! Live container metrics in bounded chronological charts.

use crate::screens::common::message;
use marsa_ui::gui_keys as gui;
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

use marsa_client::docker::StatsSample;
use marsa_ui::{
    ColorToken, Palette,
    direction::{LogicalStyle as _, isolate_technical_text, row},
    gpui::{
        AnyElement, App, Context, Div, FontWeight, IntoElement, ParentElement as _, PathBuilder,
        Styled as _, Task, canvas, div, point, px,
    },
    gpui_component::{IconName, button::Button},
};

use crate::{
    screens::common::{ScreenContext, label},
    services::Batches,
};

use super::{ContainerView, color, detail::card};

const HISTORY: Duration = Duration::from_secs(15 * 60);
const MAX_SAMPLES: usize = 1_600;

#[derive(Clone)]
struct Measurement {
    received: Instant,
    clock: Option<String>,
    cpu: Option<f64>,
    memory: Option<f64>,
    network_rate: Option<f64>,
    disk_rate: Option<f64>,
}

#[derive(Default)]
pub(super) struct StatsState {
    samples: VecDeque<Measurement>,
    last: Option<(Instant, u64, u64)>,
    paused: Option<VecDeque<Measurement>>,
    failed: bool,
    closed: bool,
}

impl StatsState {
    fn add(&mut self, sample: &StatsSample) {
        let now = Instant::now();
        let (rx, tx) = sample.network_bytes();
        let (reads, writes) = sample.block_bytes();
        let network = rx.saturating_add(tx);
        let disk = reads.saturating_add(writes);
        let (network_rate, disk_rate) =
            self.last
                .map_or((None, None), |(previous, old_network, old_disk)| {
                    let elapsed = now.saturating_duration_since(previous).as_secs_f64();
                    let rate = |current: u64, prior: u64| {
                        (elapsed > 0.0)
                            .then(|| {
                                current
                                    .checked_sub(prior)
                                    .map(|delta| delta as f64 / elapsed)
                            })
                            .flatten()
                    };
                    (rate(network, old_network), rate(disk, old_disk))
                });
        self.last = Some((now, network, disk));
        let clock = sample
            .read
            .as_deref()
            .and_then(|time| time.split_once('T'))
            .and_then(|(_, clock)| clock.get(..5))
            .filter(|clock| {
                clock
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || byte == b':')
            })
            .map(str::to_owned);
        self.samples.push_back(Measurement {
            received: now,
            clock,
            cpu: sample.cpu_percent(),
            memory: sample
                .memory_stats
                .used_without_cache()
                .map(|bytes| bytes as f64),
            network_rate: (!sample.networks.is_empty())
                .then_some(network_rate)
                .flatten(),
            disk_rate: (!sample.blkio_stats.io_service_bytes_recursive.is_empty())
                .then_some(disk_rate)
                .flatten(),
        });
        while self.samples.len() > MAX_SAMPLES
            || self
                .samples
                .front()
                .is_some_and(|first| now.duration_since(first.received) > HISTORY)
        {
            self.samples.pop_front();
        }
    }
}

pub(super) fn follow(mut stream: Batches<StatsSample>, cx: &Context<ContainerView>) -> Task<()> {
    cx.spawn(async move |view, cx| {
        while let Some(batch) = stream.next().await {
            if view
                .update(cx, |view, cx| {
                    match batch {
                        Ok(samples) => {
                            if let Some(sample) = samples.last() {
                                view.stats.add(sample);
                            }
                        }
                        Err(_) => view.stats.failed = true,
                    }
                    cx.notify();
                })
                .is_err()
            {
                return;
            }
        }
        let _ = view.update(cx, |view, cx| {
            view.stats.closed = true;
            cx.notify();
        });
    })
}

pub(super) fn render(
    view: &ContainerView,
    screen: &ScreenContext<'_>,
    cx: &mut Context<ContainerView>,
) -> AnyElement {
    let samples = view.stats.paused.as_ref().unwrap_or(&view.stats.samples);
    let mut body = div().flex().flex_col().gap_3().min_h_0().child(
        row(cx)
            .items_center()
            .gap_2()
            .child(
                div()
                    .text_size(px(12.))
                    .text_start(cx)
                    .child(message(screen.locale, gui::GUI_DESKTOP_LAST_15_MINUTES)),
            )
            .child(div().flex_1())
            .child(
                Button::new("pause-container-stats")
                    .outline()
                    .size(px(36.))
                    .p_0()
                    .bg(color(screen.palette, ColorToken::BgSubtle))
                    .icon(if view.stats.paused.is_some() {
                        IconName::Play
                    } else {
                        IconName::Pause
                    })
                    .accessibility_label(if view.stats.paused.is_some() {
                        message(screen.locale, gui::GUI_DESKTOP_RESUME_STATS)
                    } else {
                        message(screen.locale, gui::GUI_DESKTOP_PAUSE_STATS)
                    })
                    .on_click(cx.listener(|view, _, _, cx| {
                        view.stats.paused = view
                            .stats
                            .paused
                            .is_none()
                            .then(|| view.stats.samples.clone());
                        cx.notify();
                    })),
            ),
    );
    if view.stats.failed || view.stats.closed || samples.is_empty() {
        let (en, ar) = if view.stats.failed {
            (
                "Couldn’t read live statistics.",
                "تعذّرت قراءة الإحصاءات المباشرة.",
            )
        } else if view.stats.closed {
            ("The statistics stream ended.", "انتهى بث الإحصاءات.")
        } else {
            ("Waiting for statistics…", "جارٍ انتظار الإحصاءات…")
        };
        body = body.child(
            div()
                .text_size(px(12.))
                .text_start(cx)
                .child(label(screen, en, ar)),
        );
    }
    body.child(
        row(cx)
            .gap_3()
            .child(metric(
                screen,
                ("CPU", "المعالج"),
                cx,
                ColorToken::ChartCpu,
                samples,
                |sample| sample.cpu,
                |value| format!("{value:.1}%"),
            ))
            .child(metric(
                screen,
                ("Memory", "الذاكرة"),
                cx,
                ColorToken::ChartMemory,
                samples,
                |sample| sample.memory,
                |value| bytes(value, false),
            )),
    )
    .child(
        row(cx)
            .gap_3()
            .child(metric(
                screen,
                ("Network I/O", "نقل البيانات عبر الشبكة"),
                cx,
                ColorToken::ChartNetwork,
                samples,
                |sample| sample.network_rate,
                |value| bytes(value, true),
            ))
            .child(metric(
                screen,
                ("Disk I/O", "قراءة القرص وكتابته"),
                cx,
                ColorToken::ChartDisk,
                samples,
                |sample| sample.disk_rate,
                |value| bytes(value, true),
            )),
    )
    .into_any_element()
}

fn metric(
    screen: &ScreenContext<'_>,
    title: (&'static str, &'static str),
    cx: &App,
    token: ColorToken,
    data: &VecDeque<Measurement>,
    value: fn(&Measurement) -> Option<f64>,
    format_value: impl FnOnce(f64) -> String,
) -> Div {
    let current = data
        .back()
        .and_then(value)
        .map(format_value)
        .unwrap_or_else(|| "—".into());
    let first = data
        .front()
        .and_then(|sample| sample.clock.as_deref())
        .unwrap_or("—");
    let last = data
        .back()
        .and_then(|sample| sample.clock.as_deref())
        .unwrap_or("—");
    let mut points = Vec::with_capacity(data.len());
    if let (Some(first), Some(last)) = (data.front(), data.back()) {
        let total = last.received.duration_since(first.received).as_secs_f64();
        for sample in data {
            if let Some(y) = value(sample).filter(|number| number.is_finite()) {
                let elapsed = sample.received.duration_since(first.received).as_secs_f64();
                points.push((
                    if total == 0. {
                        0.
                    } else {
                        (elapsed / total) as f32
                    },
                    y,
                ));
            }
        }
    }
    card(screen, title.0, title.1)
        .text_start(cx)
        .child(
            div()
                .font_family("IBM Plex Mono")
                .font_weight(FontWeight::MEDIUM)
                .text_size(px(13.))
                .child(isolate_technical_text(&current)),
        )
        .child(chart(screen.palette, token, points))
        .child(
            div()
                .flex()
                .justify_between()
                .text_size(px(11.))
                .font_family("IBM Plex Mono")
                .text_color(color(screen.palette, ColorToken::TextMuted))
                .child(isolate_technical_text(first))
                .child(isolate_technical_text(last)),
        )
}

fn chart(palette: Palette, token: ColorToken, values: Vec<(f32, f64)>) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, (), window, _: &mut App| {
            for fraction in [0., 0.33, 0.66, 1.] {
                let y = bounds.origin.y + bounds.size.height * fraction;
                let mut path = PathBuilder::stroke(px(1.));
                path.move_to(point(bounds.origin.x, y));
                path.line_to(point(bounds.origin.x + bounds.size.width, y));
                if let Ok(path) = path.build() {
                    window.paint_path(path, color(palette, ColorToken::BorderSubtle));
                }
            }
            let maximum = values.iter().map(|(_, y)| *y).fold(1.0, f64::max);
            let mut path = PathBuilder::stroke(px(2.));
            for (index, (x, y)) in values.into_iter().enumerate() {
                let y = (y / maximum).clamp(0.0, 1.0) as f32;
                let location = point(
                    bounds.origin.x + bounds.size.width * x,
                    bounds.origin.y + bounds.size.height * (0.90 - y * 0.8),
                );
                if index == 0 {
                    path.move_to(location);
                } else {
                    path.line_to(location);
                }
            }
            if let Ok(path) = path.build() {
                window.paint_path(path, color(palette, token));
            }
        },
    )
    .w_full()
    .h(px(72.))
}

fn bytes(bytes: f64, rate: bool) -> String {
    let (value, unit) = if bytes >= 1024. * 1024. * 1024. {
        (bytes / (1024. * 1024. * 1024.), "GiB")
    } else if bytes >= 1024. * 1024. {
        (bytes / (1024. * 1024.), "MiB")
    } else if bytes >= 1024. {
        (bytes / 1024., "KiB")
    } else {
        (bytes, "B")
    };
    let suffix = if rate { "/s" } else { "" };
    format!("{value:.1} {unit}{suffix}")
}
