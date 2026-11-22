//! Decide guest bind requests from currently mounted and still-approved host roots.

use std::{
    env,
    path::{Path, PathBuf},
    sync::Arc,
};

use marsa_core::settings::Language;
use marsa_share::{Allowlist, ShareError, SharePolicy};
use marsa_vm::async_trait;

use crate::guest_control::BindPolicy;

use super::{bind_prompts, events::Bus};

pub(super) struct Mounted {
    pub(super) allowlist: Arc<Allowlist>,
    pub(super) approved: Vec<PathBuf>,
    pub(super) language: Language,
    pub(super) revision: u64,
}

pub(super) struct Policy {
    events: Bus,
    protected: SharePolicy,
}

impl Policy {
    pub(super) fn new(events: Bus, protected: SharePolicy) -> Self {
        Self { events, protected }
    }
}

#[async_trait]
impl BindPolicy for Policy {
    async fn authorize(&self, sources: Vec<String>) -> Option<&'static str> {
        let Some(mounted) = self.events.bind_configuration() else {
            return Some(changed(Language::System));
        };
        let locale = mounted.language;
        let protected = self.protected.clone();
        let (mounted, verdict) = match tokio::task::spawn_blocking(move || {
            let verdict = inspect(&mounted, &protected, sources);
            (mounted, verdict)
        })
        .await
        {
            Ok(output) => output,
            Err(_) => return Some(changed(locale)),
        };
        if !self.events.bindings_current(&mounted) {
            return Some(changed(locale));
        }
        match verdict {
            Verdict::Allow => None,
            Verdict::Reject => Some(shown(
                locale,
                "Marsa denied this bind mount. Use the canonical path to a shared file or folder, or use a Docker named volume.",
                "رفض مارسا هذا الوصل. استخدم المسار الفعلي لملف أو مجلد مشترك، أو استخدم وحدة تخزين Docker مسماة.",
            )),
            Verdict::Ask(folders) => {
                enqueue(&self.events, &mounted, folders);
                Some(shown(
                    locale,
                    "This folder is outside the active Marsa shares. Review the share request in Marsa, apply the share and retry the Docker command.",
                    "هذا المجلد خارج مشاركات مارسا النشطة. راجع طلب المشاركة في مارسا، ثم طبّق المشاركة وأعد تنفيذ أمر Docker.",
                ))
            }
        }
    }
}

enum Verdict {
    Allow,
    Reject,
    Ask(Arc<Allowlist>),
}

fn inspect(mounted: &Mounted, policy: &SharePolicy, sources: Vec<String>) -> Verdict {
    let mut approvals = Vec::new();
    for source in sources {
        let source = Path::new(&source);
        match mounted.allowlist.authorize_if(source, |root| {
            mounted.approved.iter().any(|path| path == root)
        }) {
            Ok(canonical) if canonical == source => (),
            Err(ShareError::NotAllowed) => {
                let Ok(directory) = policy.bind_approval_directory(source) else {
                    return Verdict::Reject;
                };
                approvals.push(directory);
            }
            _ => return Verdict::Reject,
        }
    }
    if approvals.is_empty() {
        Verdict::Allow
    } else {
        match Allowlist::from_paths(policy.clone(), &approvals) {
            Ok(roots) if roots.roots().iter().all(|root| approvals.contains(root)) => {
                Verdict::Ask(Arc::new(roots))
            }
            _ => Verdict::Reject,
        }
    }
}

fn enqueue(events: &Bus, mounted: &Mounted, folders: Arc<Allowlist>) {
    if let Ok(Some(id)) = events.bind_prompt(mounted, folders) {
        let events = events.clone();
        tokio::spawn(async move {
            tokio::time::sleep(bind_prompts::LIFETIME).await;
            events.expire_bind_prompt(&id);
        });
    }
}

fn changed(language: Language) -> &'static str {
    shown(
        language,
        "Marsa could not check the active shared folders. Retry the Docker command once the VM is ready.",
        "تعذّر على مارسا التحقق من المجلدات المشتركة النشطة. أعد تنفيذ أمر Docker بعد جاهزية الآلة الافتراضية.",
    )
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
