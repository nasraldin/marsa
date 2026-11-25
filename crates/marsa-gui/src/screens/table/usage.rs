//! Image usage filter buckets and helpers.

use marsa_ui::{ColorToken, Locale, gpui::SharedString, gui_keys as gui};

use crate::screens::common::message;

/// Usage filter applied to the Images table.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum UsageFilter {
    #[default]
    All,
    InUse,
    SomeUnused,
    Unused,
}

impl UsageFilter {
    pub(crate) const ALL: [Self; 4] = [Self::All, Self::InUse, Self::SomeUnused, Self::Unused];

    pub(crate) fn label(self, locale: Locale) -> SharedString {
        message(
            locale,
            match self {
                Self::All => gui::GUI_DESKTOP_ALL,
                Self::InUse => gui::GUI_DESKTOP_IN_USE,
                Self::SomeUnused => gui::GUI_DESKTOP_SOME_UNUSED,
                Self::Unused => gui::GUI_DESKTOP_UNUSED,
            },
        )
    }

    pub(crate) fn icon(self) -> (&'static str, ColorToken) {
        match self {
            Self::All => ("icons/circle-dashed.svg", ColorToken::TextMuted),
            Self::InUse => ("icons/circle-dot.svg", ColorToken::StatusRunning),
            Self::SomeUnused => ("icons/circle-dot.svg", ColorToken::StatusWarning),
            Self::Unused => ("icons/circle.svg", ColorToken::TextMuted),
        }
    }

    pub(crate) fn matches(self, bucket: UsageBucket) -> bool {
        match self {
            Self::All => true,
            Self::InUse => bucket == UsageBucket::InUse,
            Self::SomeUnused => bucket == UsageBucket::SomeUnused,
            Self::Unused => bucket == UsageBucket::Unused,
        }
    }
}

/// Derived usage for one image (after tagging vs container references).
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum UsageBucket {
    InUse,
    SomeUnused,
    Unused,
}

impl UsageBucket {
    /// Classify an image from whether containers use it and which tags they reference.
    ///
    /// `tag_used[i]` is true when `tags[i]` is referenced by at least one container
    /// that also uses this image content id. `image_used` is true when any container
    /// references the image id.
    pub(crate) fn classify(image_used: bool, tag_used: &[bool]) -> Self {
        if !image_used {
            return Self::Unused;
        }
        let has_tags = !tag_used.is_empty();
        let any_unused_tag = has_tags && tag_used.iter().any(|used| !*used);
        if any_unused_tag {
            Self::SomeUnused
        } else {
            Self::InUse
        }
    }

    pub(crate) fn label(self, locale: Locale) -> SharedString {
        message(
            locale,
            match self {
                Self::InUse => gui::GUI_DESKTOP_IN_USE,
                Self::SomeUnused => gui::GUI_DESKTOP_SOME_UNUSED,
                Self::Unused => gui::GUI_DESKTOP_UNUSED,
            },
        )
    }

    pub(crate) fn tone(self) -> (ColorToken, ColorToken) {
        match self {
            Self::InUse => (ColorToken::StatusRunning, ColorToken::StatusRunningBg),
            Self::SomeUnused => (ColorToken::StatusWarning, ColorToken::StatusWarningBg),
            Self::Unused => (ColorToken::StatusStopped, ColorToken::StatusStoppedBg),
        }
    }
}

/// Prefer a tag that a container currently references; otherwise the first real tag.
pub(crate) fn preferred_in_use_tag<'a>(tags: &'a [String], tag_used: &[bool]) -> Option<&'a str> {
    tags.iter()
        .zip(tag_used.iter().copied())
        .find(|(_, used)| *used)
        .or_else(|| tags.first().map(|tag| (tag, true)))
        .map(|(tag, _)| tag.as_str())
        .filter(|tag| *tag != "<none>:<none>")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unused_when_no_containers() {
        assert_eq!(
            UsageBucket::classify(false, &[true, false]),
            UsageBucket::Unused
        );
        assert_eq!(UsageBucket::classify(false, &[]), UsageBucket::Unused);
    }

    #[test]
    fn in_use_when_all_tags_referenced_or_untagged() {
        assert_eq!(
            UsageBucket::classify(true, &[true, true]),
            UsageBucket::InUse
        );
        assert_eq!(UsageBucket::classify(true, &[]), UsageBucket::InUse);
        assert_eq!(UsageBucket::classify(true, &[true]), UsageBucket::InUse);
    }

    #[test]
    fn some_unused_when_partial_tags() {
        assert_eq!(
            UsageBucket::classify(true, &[true, false]),
            UsageBucket::SomeUnused
        );
        assert_eq!(
            UsageBucket::classify(true, &[false]),
            UsageBucket::SomeUnused
        );
    }

    #[test]
    fn preferred_tag_picks_used_first() {
        let tags = vec!["a:1".into(), "b:2".into()];
        assert_eq!(preferred_in_use_tag(&tags, &[false, true]), Some("b:2"));
        assert_eq!(preferred_in_use_tag(&tags, &[false, false]), Some("a:1"));
    }

    #[test]
    fn usage_filter_matches_buckets() {
        assert!(UsageFilter::All.matches(UsageBucket::Unused));
        assert!(UsageFilter::InUse.matches(UsageBucket::InUse));
        assert!(!UsageFilter::InUse.matches(UsageBucket::Unused));
        assert!(UsageFilter::SomeUnused.matches(UsageBucket::SomeUnused));
        assert!(UsageFilter::Unused.matches(UsageBucket::Unused));
    }
}
