//! Decide guest bind requests from currently mounted and still-approved host roots.

use std::{
    env,
    path::{Path, PathBuf},
};

use marsa_core::settings::Language;
use marsa_share::{Allowlist, ShareError, SharePolicy};
use tokio::runtime::Handle;

use super::{bind_prompts, events::Bus};

pub(super) fn evaluate(
    events: &Bus,
    policy: &SharePolicy,
    runtime: &Handle,
    sources: Vec<String>,
) -> Option<&'static str> {
    let (roots, settings, revision) = events.bind_configuration();
    let locale = settings.ui.language;
    let denied = shown(
        locale,
        "Marsa denied this bind mount. Use the canonical path to a shared file or folder, or use a Docker named volume.",
        "رفض مارسـا هذا الوصل. استخدم المسار الفعلي لملف أو مجلد مشترك، أو استخدم وحدة تخزين Docker مسماة.",
    );
    let Ok(allowlist) = Allowlist::from_paths(policy.clone(), roots) else {
        return Some(denied);
    };
    let mut approvals = Vec::new();
    for source in sources {
        let source = Path::new(&source);
        match allowlist.authorize(source) {
            Ok(canonical) if canonical == source => (),
            Err(ShareError::NotAllowed) => {
                let Ok(directory) = policy.bind_approval_directory(source) else {
                    return Some(denied);
                };
                approvals.push(directory);
            }
            _ => return Some(denied),
        }
    }
    if events.settings_snapshot().revision != revision {
        return Some(shown(
            locale,
            "Shared folders changed during this check. Retry the Docker command.",
            "تغيّرت المجلدات المشتركة أثناء الفحص. أعد تنفيذ أمر Docker.",
        ));
    }
    if approvals.is_empty() {
        return None;
    }
    enqueue(events, runtime, approvals);
    Some(shown(
        locale,
        "This folder is outside the active Marsa shares. Review the share request in Marsa, apply the share and retry the Docker command.",
        "هذا المجلد خارج مشاركات مارسـا النشطة. راجع طلب المشاركة في مارسـا، ثم طبّق المشاركة وأعد تنفيذ أمر Docker.",
    ))
}

fn enqueue(events: &Bus, runtime: &Handle, folders: Vec<PathBuf>) {
    if let Ok(Some(id)) = events.bind_prompt(folders) {
        let events = events.clone();
        runtime.spawn(async move {
            tokio::time::sleep(bind_prompts::LIFETIME).await;
            events.expire_bind_prompt(&id);
        });
    }
}

pub(super) fn shown(
    language: Language,
    english: &'static str,
    arabic: &'static str,
) -> &'static str {
    match language {
        Language::Arabic => arabic,
        Language::English => english,
        Language::System => {
            let locale = ["LC_ALL", "LC_MESSAGES", "LANG"]
                .into_iter()
                .filter_map(|name| env::var(name).ok())
                .find(|locale| !locale.is_empty());
            match locale
                .as_deref()
                .and_then(|locale| locale.split(['_', '-', '.', '@']).next())
            {
                Some("ar") => arabic,
                _ => english,
            }
        }
    }
}
