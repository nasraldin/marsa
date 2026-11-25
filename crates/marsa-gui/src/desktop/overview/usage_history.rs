//! Keep a short history of observed utilization for the selected engine.

use std::collections::{BTreeMap, VecDeque};

use marsa_client::docker::{ContainerId, OverviewUtilization};
use marsa_ui::{
    ColorToken,
    direction::{LogicalStyle as _, technical_row},
    gpui::{
        App, Div, InteractiveElement as _, IntoElement, ParentElement as _, PathBuilder, Pixels,
        StatefulInteractiveElement as _, Styled as _, canvas, div, point, px,
    },
    gpui_component::tooltip::Tooltip,
};

use crate::screens::common::{ScreenContext, color, disk_bytes};

const SAMPLE_COUNT: usize = 24;
const SPARKLINE_WIDTH: Pixels = px(70.);
const SPARKLINE_HEIGHT: Pixels = px(18.);
const SPARKLINE_GAP: Pixels = px(2.);

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Metric {
    Cpu,
    Memory,
}

impl Metric {
    pub(super) fn color(self) -> ColorToken {
        match self {
            Self::Cpu => ColorToken::ChartCpu,
            Self::Memory => ColorToken::ChartMemory,
        }
    }
}

#[derive(Clone, Copy)]
struct Values {
    cpu: Option<f64>,
    memory: Option<u64>,
}

#[derive(Default)]
pub(super) struct UsageHistory {
    containers: VecDeque<BTreeMap<ContainerId, Values>>,
    engine: VecDeque<Values>,
}

impl UsageHistory {
    pub(super) fn record(&mut self, sample: Option<&OverviewUtilization>) {
        if self.containers.len() == SAMPLE_COUNT {
            self.containers.pop_front();
        }
        if self.engine.len() == SAMPLE_COUNT {
            self.engine.pop_front();
        }
        let mut cpu_total = None;
        let mut memory_total = None;
        let mut rows = BTreeMap::new();
        if let Some(sample) = sample {
            for usage in &sample.usage {
                if let Some(cpu) = usage.cpu_percent {
                    *cpu_total.get_or_insert(0.) += cpu;
                }
                if let Some(memory) = usage.memory_bytes {
                    *memory_total.get_or_insert(0) += memory;
                }
                rows.insert(
                    usage.id.clone(),
                    Values {
                        cpu: usage.cpu_percent,
                        memory: usage.memory_bytes,
                    },
                );
            }
        }
        self.containers.push_back(rows);
        self.engine.push_back(Values {
            cpu: cpu_total,
            memory: memory_total,
        });
    }

    fn values<'a>(
        &'a self,
        id: &'a ContainerId,
        metric: Metric,
    ) -> impl Iterator<Item = Option<f64>> + 'a {
        self.containers.iter().map(move |sample| {
            let values = sample.get(id)?;
            match metric {
                Metric::Cpu => values.cpu,
                Metric::Memory => values.memory.map(|bytes| bytes as f64),
            }
        })
    }

    fn engine_values(&self, metric: Metric) -> impl Iterator<Item = Option<f64>> + '_ {
        self.engine.iter().map(move |sample| match metric {
            Metric::Cpu => sample.cpu,
            Metric::Memory => sample.memory.map(|bytes| bytes as f64),
        })
    }

    pub(super) fn latest_engine(&self, metric: Metric) -> Option<f64> {
        self.engine.back().and_then(|sample| match metric {
            Metric::Cpu => sample.cpu,
            Metric::Memory => sample.memory.map(|bytes| bytes as f64),
        })
    }

    pub(super) fn bars(&self, screen: &ScreenContext<'_>, id: &ContainerId, metric: Metric) -> Div {
        sparkline(screen, self.values(id, metric), metric.color())
    }

    /// Thin line chart scaled to host capacity, with per-sample hover tooltips.
    ///
    /// `ceiling` is the absolute Y maximum (host CPUs × 100 for CPU, memory limit
    /// bytes for memory). Without it the chart falls back to the observed peak.
    pub(super) fn engine_line(
        &self,
        screen: &ScreenContext<'_>,
        metric: Metric,
        ceiling: Option<f64>,
    ) -> impl IntoElement {
        let samples: Vec<_> = self.engine_values(metric).collect();
        let tips: Vec<_> = samples
            .iter()
            .enumerate()
            .map(|(index, sample)| {
                let label = match (metric, sample) {
                    (Metric::Cpu, Some(value)) => format!("{value:.1}%"),
                    (Metric::Memory, Some(value)) => disk_bytes(*value as u64),
                    _ => "—".into(),
                };
                (index, label)
            })
            .collect();
        let peak = samples.iter().copied().flatten().fold(0., f64::max);
        // Prefer host capacity so idle usage sits low instead of filling the chart.
        let maximum = ceiling
            .filter(|value| value.is_finite() && *value > 0.)
            .unwrap_or(peak)
            .max(peak)
            .max(1.);
        let token = metric.color();
        let palette = screen.palette;
        let chart_id = match metric {
            Metric::Cpu => 0_u8,
            Metric::Memory => 1,
        };
        let painted = samples.clone();
        div()
            .relative()
            .w_full()
            .h(px(44.))
            .rounded(px(6.))
            .bg(color(screen.palette, ColorToken::BgSubtle))
            .overflow_hidden()
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, (), window, _: &mut App| {
                        let count = painted.len();
                        if count == 0 {
                            return;
                        }
                        let width = f32::from(bounds.size.width);
                        let height = f32::from(bounds.size.height);
                        let mut fill = PathBuilder::fill();
                        let mut stroke = PathBuilder::stroke(px(2.));
                        let mut started = false;
                        let mut first = None;
                        let mut last = None;
                        for (index, sample) in painted.iter().enumerate() {
                            let Some(value) = sample.filter(|value| value.is_finite()) else {
                                started = false;
                                continue;
                            };
                            let x = if count == 1 {
                                bounds.origin.x + bounds.size.width / 2.
                            } else {
                                bounds.origin.x + px(width * index as f32 / (count - 1) as f32)
                            };
                            let y = bounds.origin.y
                                + px(height
                                    * (1. - (value / maximum).clamp(0., 1.) as f32 * 0.82 - 0.08));
                            if started {
                                fill.line_to(point(x, y));
                                stroke.line_to(point(x, y));
                            } else {
                                fill.move_to(point(x, y));
                                stroke.move_to(point(x, y));
                                started = true;
                                first = Some(point(x, y));
                            }
                            last = Some(point(x, y));
                        }
                        if let (Some(start), Some(end)) = (first, last) {
                            let bottom = bounds.origin.y + bounds.size.height;
                            fill.line_to(point(end.x, bottom));
                            fill.line_to(point(start.x, bottom));
                            fill.close();
                            if let Ok(path) = fill.build() {
                                window.paint_path(path, color(palette, token).opacity(0.22));
                            }
                        }
                        if let Ok(path) = stroke.build() {
                            window.paint_path(path, color(palette, token));
                        }
                    },
                )
                .size_full(),
            )
            .child(
                technical_row()
                    .absolute()
                    .inset_0()
                    .children(tips.into_iter().map(|(index, tip)| {
                        div()
                            .id(("overview-history-hover", chart_id as usize * 64 + index))
                            .flex_1()
                            .h_full()
                            .hover(|style| {
                                style.bg(color(palette, ColorToken::AccentSubtle).opacity(0.55))
                            })
                            .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                    })),
            )
    }
}

fn sparkline_bar_width(samples: usize) -> Pixels {
    let available = SPARKLINE_WIDTH - SPARKLINE_GAP * samples.saturating_sub(1);
    px(4.).min(available * (1. / samples.max(1) as f32))
}

fn sparkline(
    screen: &ScreenContext<'_>,
    values: impl Iterator<Item = Option<f64>>,
    token: ColorToken,
) -> Div {
    let samples: Vec<_> = values.collect();
    let bar_width = sparkline_bar_width(samples.len());
    let maximum = samples.iter().copied().flatten().fold(0., f64::max).max(1.);
    let height_px = f64::from(SPARKLINE_HEIGHT);
    technical_row()
        .items_end()
        .justify_row_end()
        .gap(SPARKLINE_GAP)
        .w(SPARKLINE_WIDTH)
        .h(SPARKLINE_HEIGHT)
        .flex_shrink_0()
        .overflow_hidden()
        .children(samples.into_iter().map(|value| {
            let bar = div().w(bar_width).flex_shrink_0().rounded(px(1.));
            if let Some(value) = value {
                bar.h(px((height_px * value / maximum).clamp(2., height_px) as f32))
                    .bg(color(screen.palette, token))
            } else {
                bar.h(px(2.))
                    .bg(color(screen.palette, ColorToken::BorderSubtle))
            }
        }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use marsa_client::docker::ContainerUtilization;

    #[test]
    fn sparkline_fits_each_history_length() {
        assert_eq!(sparkline_bar_width(1), px(4.));
        for samples in 1..=SAMPLE_COUNT {
            let width = sparkline_bar_width(samples);
            let with_gaps = width * samples + SPARKLINE_GAP * (samples - 1);
            assert!(width >= px(1.), "{samples} samples: {width:?}");
            assert!(
                with_gaps <= SPARKLINE_WIDTH,
                "{samples} samples: {with_gaps:?}"
            );
        }
    }

    #[test]
    fn history_keeps_recent_samples_and_preserves_missing_measurements() {
        let id = ContainerId::parse("a".repeat(64)).unwrap();
        let sample = |value| OverviewUtilization {
            running: 1,
            usage: vec![ContainerUtilization {
                id: id.clone(),
                name: "fixture".into(),
                cpu_percent: Some(value),
                memory_bytes: Some((value as u64) * 1024),
                memory_limit: Some(1024 * 1024),
                online_cpus: Some(4),
            }],
        };
        let mut history = UsageHistory::default();
        for value in 0..SAMPLE_COUNT {
            history.record(Some(&sample(value as f64)));
        }
        history.record(None);
        let cpu: Vec<_> = history.values(&id, Metric::Cpu).collect();
        assert_eq!(cpu.len(), SAMPLE_COUNT);
        assert_eq!(cpu[0], Some(1.));
        assert_eq!(cpu.last(), Some(&None));
        // The chart retains prior points, while the current value is unavailable.
        assert_eq!(history.latest_engine(Metric::Cpu), None);
        assert_eq!(history.latest_engine(Metric::Memory), None);
        history.record(Some(&sample(4.)));
        assert_eq!(history.latest_engine(Metric::Cpu), Some(4.));
        assert_eq!(history.latest_engine(Metric::Memory), Some(4096.));
        assert!(
            history
                .values(&id, Metric::Memory)
                .any(|value| value == Some(4096.))
        );
    }
}
