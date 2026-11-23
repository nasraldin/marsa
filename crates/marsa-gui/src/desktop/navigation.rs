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
    Reports,
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
    pub(crate) const SYSTEM: [Self; 5] = [
        Self::Machine,
        Self::Disk,
        Self::Activity,
        Self::Reports,
        Self::Settings,
    ];

    pub(crate) fn name(self, locale: Locale) -> &'static str {
        let (en, ar) = match self {
            Self::Overview => ("Overview", "نظرة عامة"),
            Self::Containers => ("Containers", "الحاويات"),
            Self::Images => ("Images", "الصور"),
            Self::Volumes => ("Volumes", "وحدات التخزين"),
            Self::Networks => ("Networks", "الشبكات"),
            Self::Compose => ("Stacks", "المكدسات"),
            Self::Machine => ("Machine", "الجهاز"),
            Self::Disk => ("Disk & cleanup", "القرص والتنظيف"),
            Self::Activity => ("Activity", "النشاط"),
            Self::Reports => ("Reports", "التقارير"),
            Self::Settings => ("Settings", "الإعدادات"),
        };
        copy(locale, en, ar)
    }

    pub(crate) fn description(self, locale: Locale) -> &'static str {
        let (en, ar) = match self {
            Self::Overview => return marsa_ui::gui_keys::GUI_OVERVIEW_DESCRIPTION.text(locale),
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
                "Deploy and manage Docker Compose stacks.",
                "انشر مكدسات Docker Compose وأدرها.",
            ),
            Self::Machine => (
                "Manage the Linux virtual machine that runs Marsa.",
                "إدارة جهاز Linux الافتراضي الذي يشغّل مرسى.",
            ),
            Self::Disk => (
                "Storage used by the local engine on this Mac.",
                "مساحة التخزين التي يستخدمها المحرّك المحلي على هذا الـ Mac.",
            ),
            Self::Activity => (
                "Review engine events and decisions for agent requests.",
                "راجع أحداث المحرّك والقرارات بشأن طلبات الوكلاء.",
            ),
            Self::Reports => (
                "Review the active engine’s inventory.",
                "راجع قائمة موارد المحرّك النشط.",
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
            Self::Reports => "icons/file-text.svg",
            Self::Settings => "icons/sliders-horizontal.svg",
        }
    }
}
