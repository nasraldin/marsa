//! Keep a short history of observed utilization for the selected engine.

use std::collections::{BTreeMap, VecDeque};

use marsa_client::docker::{ContainerId, OverviewUtilization};
use marsa_ui::{
    ColorToken,
    gpui::{Div, ParentElement as _, Styled as _, div, px},
};

use crate::screens::common::{ScreenContext, color, row};

const SAMPLE_COUNT: usize = 12;

#[derive(Clone, Copy)]
pub(super) enum Metric {
    Cpu,
    Memory,
}

impl Metric {
    fn color(self) -> ColorToken {
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
pub(super) struct UsageHistory(VecDeque<BTreeMap<ContainerId, Values>>);

impl UsageHistory {
    pub(super) fn record(&mut self, sample: Option<&OverviewUtilization>) {
        if self.0.len() == SAMPLE_COUNT {
            self.0.pop_front();
        }
        self.0.push_back(
            sample
                .into_iter()
                .flat_map(|sample| &sample.usage)
                .map(|usage| {
                    (
                        usage.id.clone(),
                        Values {
                            cpu: usage.cpu_percent,
                            memory: usage.memory_bytes,
                        },
                    )
                })
                .collect(),
        );
    }

    fn values<'a>(
        &'a self,
        id: &'a ContainerId,
        metric: Metric,
    ) -> impl Iterator<Item = Option<f64>> + 'a {
        self.0.iter().map(move |sample| {
            let values = sample.get(id)?;
            match metric {
                Metric::Cpu => values.cpu,
                Metric::Memory => values.memory.map(|bytes| bytes as f64),
            }
        })
    }

    pub(super) fn bars(&self, screen: &ScreenContext<'_>, id: &ContainerId, metric: Metric) -> Div {
        let maximum = self.values(id, metric).flatten().fold(0., f64::max).max(1.);
        row(screen.locale)
            .items_end()
            .justify_end()
            .gap(px(2.))
            .w(px(70.))
            .h(px(18.))
            .flex_shrink_0()
            .children(self.values(id, metric).map(|value| {
                let bar = div().w(px(4.)).flex_shrink_0().rounded(px(1.));
                if let Some(value) = value {
                    bar.h(px((18. * value / maximum).clamp(1., 18.) as f32))
                        .bg(color(screen.palette, metric.color()))
                } else {
                    bar
                }
            }))
    }
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
                memory_bytes: None,
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
        assert!(
            history
                .values(&id, Metric::Memory)
                .all(|value| value.is_none())
        );
    }
}
