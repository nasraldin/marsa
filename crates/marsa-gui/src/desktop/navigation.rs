//! Routes shared by the sidebar, search and keyboard commands.

use marsa_ui::Locale;

use crate::screens::common::copy;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum Route {
    #[default]
    Overview,
    Containers,
    Images,
    Volumes,
    Networks,
    Compose,
    Machine,
    Disk,
    Activity,
    Settings,
}

impl Route {
    pub(crate) const WORKSPACE: [Self; 6] = [
        Self::Overview,
        Self::Containers,
        Self::Images,
        Self::Volumes,
        Self::Networks,
        Self::Compose,
    ];
    pub(crate) const SYSTEM: [Self; 4] =
        [Self::Machine, Self::Disk, Self::Activity, Self::Settings];

    pub(crate) fn name(self, locale: Locale) -> &'static str {
        let (en, ar) = match self {
            Self::Overview => ("Overview", "نظرة عامة"),
            Self::Containers => ("Containers", "الحاويات"),
            Self::Images => ("Images", "الصور"),
            Self::Volumes => ("Volumes", "وحدات التخزين"),
            Self::Networks => ("Networks", "الشبكات"),
            Self::Compose => ("Compose", "مشاريع Compose"),
            Self::Machine => ("Machine", "الجهاز"),
            Self::Disk => ("Disk & cleanup", "القرص والتنظيف"),
            Self::Activity => ("Activity", "النشاط"),
            Self::Settings => ("Settings", "الإعدادات"),
        };
        copy(locale, en, ar)
    }

    pub(crate) fn description(self, locale: Locale) -> &'static str {
        let (en, ar) = match self {
            Self::Overview => (
                "Local engine, resources, and recent activity.",
                "المحرّك المحلي والموارد والنشاط الأخير.",
            ),
            Self::Containers => (
                "Containers and Compose projects on this Mac.",
                "الحاويات ومشاريع Compose على هذا الـ Mac.",
            ),
            Self::Images => (
                "Manage local container images.",
                "إدارة صور الحاويات المحلية.",
            ),
            Self::Volumes => (
                "Manage persistent container storage.",
                "إدارة التخزين الدائم للحاويات.",
            ),
            Self::Networks => ("Manage how containers connect.", "إدارة اتصالات الحاويات."),
            Self::Compose => (
                "Manage services grouped by Compose project.",
                "إدارة الخدمات المجمّعة حسب مشروع Compose.",
            ),
            Self::Machine => (
                "Manage the Linux virtual machine that runs Marsa.",
                "إدارة جهاز Linux الافتراضي الذي يشغّل مرسى.",
            ),
            Self::Disk => (
                "Review storage usage and reclaim space on this Mac.",
                "مراجعة استخدام التخزين واسترداد المساحة على هذا الـ Mac.",
            ),
            Self::Activity => (
                "Review engine events and pending approvals.",
                "مراجعة أحداث المحرّك والموافقات المعلّقة.",
            ),
            Self::Settings => (
                "Manage how Marsa runs on this Mac.",
                "إدارة طريقة تشغيل مرسى على هذا الـ Mac.",
            ),
        };
        copy(locale, en, ar)
    }

    pub(crate) fn icon(self) -> &'static str {
        match self {
            Self::Overview => "icons/panels-top-left.svg",
            Self::Containers => "icons/box.svg",
            Self::Images => "icons/layers.svg",
            Self::Volumes => "icons/database.svg",
            Self::Networks => "icons/network.svg",
            Self::Compose => "icons/layout-template.svg",
            Self::Machine => "icons/server.svg",
            Self::Disk => "icons/chart-no-axes-combined.svg",
            Self::Activity => "icons/activity.svg",
            Self::Settings => "icons/sliders-horizontal.svg",
        }
    }
}
