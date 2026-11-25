//! Keep a short history of observed utilization for the selected engine.

use std::collections::{BTreeMap, VecDeque};

use marsa_client::docker::{ContainerId, OverviewUtilization};
use marsa_ui::{
    ColorToken,
    gpui::{Div, ParentElement as _, Styled as _, div, px},
};

use crate::screens::common::{ScreenContext, color, row};

const SAMPLE_COUNT: usize = 24;

#[derive(Clone, Copy)]
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
        self.engine.iter().rev().find_map(|sample| match metric {
            Metric::Cpu => sample.cpu,
            Metric::Memory => sample.memory.map(|bytes| bytes as f64),
        })
    }

    pub(super) fn bars(&self, screen: &ScreenContext<'_>, id: &ContainerId, metric: Metric) -> Div {
        sparkline(
            screen,
            self.values(id, metric),
            metric.color(),
            px(70.),
            px(18.),
            px(4.),
        )
    }

    pub(super) fn engine_sparkline(&self, screen: &ScreenContext<'_>, metric: Metric) -> Div {
        sparkline(
            screen,
            self.engine_values(metric),
            metric.color(),
            px(220.),
            px(36.),
            px(5.),
        )
    }
}

fn sparkline(
    screen: &ScreenContext<'_>,
    values: impl Iterator<Item = Option<f64>>,
    token: ColorToken,
    width: marsa_ui::gpui::Pixels,
    height: marsa_ui::gpui::Pixels,
    bar_width: marsa_ui::gpui::Pixels,
) -> Div {
    let samples: Vec<_> = values.collect();
    let maximum = samples.iter().copied().flatten().fold(0., f64::max).max(1.);
    let height_px = f64::from(height);
    row(screen.locale)
        .items_end()
        .justify_end()
        .gap(px(2.))
        .w(width)
        .h(height)
        .flex_shrink_0()
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
    fn history_keeps_recent_samples_and_preserves_missing_measurements() {
        let id = ContainerId::parse("a".repeat(64)).unwrap();
        let sample = |value| OverviewUtilization {
            running: 1,
            usage: vec![ContainerUtilization {
                id: id.clone(),
                name: "fixture".into(),
                cpu_percent: Some(value),
                memory_bytes: Some((value as u64) * 1024),
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
        // Keep the last measured engine total while a sample is missing.
        assert_eq!(history.latest_engine(Metric::Cpu), Some(23.));
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
