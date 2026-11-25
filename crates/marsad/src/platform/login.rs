//! Apply a user's login preference to the app's own macOS LaunchAgent.

#![allow(unsafe_code)]

use std::{ffi::OsStr, fs, io, path::PathBuf};

use objc2::rc::{Retained, autoreleasepool};
use objc2_foundation::{NSBundle, NSError, NSString};
use objc2_service_management::{SMAppService, SMAppServiceStatus};

use crate::diagnostics::{LOGIN_USAGE, localized, usage};

pub(super) fn run(action: &OsStr) -> io::Result<()> {
    let action = match action.to_str() {
        Some("status") => Action::Status,
        Some("enable") => Action::Enable,
        Some("disable") => Action::Disable,
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                usage(LOGIN_USAGE),
            ));
        }
    };
    autoreleasepool(|_| {
        let service = app_service()?;
        let status = apply(&service, action)?;
        println!("{}", status.as_str());
        if status == Status::RequiresApproval {
            eprintln!(
                "marsad: {}",
                localized(
                    "Allow Marsa's background item in System Settings under General > Login Items & Extensions",
                    "اسمح لمرسى بالعمل في الخلفية من إعدادات النظام، ضمن عام ثم عناصر تسجيل الدخول والامتدادات",
                )
            );
        }
        Ok(())
    })
}

#[derive(Clone, Copy)]
enum Action {
    Status,
    Enable,
    Disable,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Status {
    Enabled,
    Disabled,
    RequiresApproval,
}

impl Status {
    fn as_str(self) -> &'static str {
        match self {
            Self::Enabled => "enabled",
            Self::Disabled => "disabled",
            Self::RequiresApproval => "requires-approval",
        }
    }
}

pub(super) fn bundled_agent() -> io::Result<(PathBuf, String, String)> {
    let bundle = NSBundle::mainBundle();
    let path = PathBuf::from(bundle.bundlePath().to_string());
    let identifier = bundle
        .bundleIdentifier()
        .ok_or_else(missing_bundle)?
        .to_string();
    if path.extension() != Some(OsStr::new("app")) || !valid_identifier(&identifier) {
        return Err(missing_bundle());
    }
    let name = format!("{identifier}.marsad.plist");
    let plist = path.join("Contents/Library/LaunchAgents").join(&name);
    if !fs::symlink_metadata(plist).is_ok_and(|metadata| metadata.is_file()) {
        return Err(missing_bundle());
    }
    Ok((path, identifier, name))
}

pub(super) fn disable_installed_agent() -> io::Result<()> {
    let service = app_service()?;
    apply(&service, Action::Disable).map(|_| ())
}

fn app_service() -> io::Result<Retained<SMAppService>> {
    let (_, _, name) = bundled_agent()?;
    // SAFETY: The name is a single plist filename in the calling app's
    // LaunchAgents directory. Service Management verifies the app signature.
    Ok(unsafe { SMAppService::agentServiceWithPlistName(&NSString::from_str(&name)) })
}

fn valid_identifier(value: &str) -> bool {
    value.len() <= 128
        && value.split('.').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}

fn apply(service: &SMAppService, action: Action) -> io::Result<Status> {
    let before = status(service)?;
    let change = match (action, before) {
        (Action::Enable, Status::Disabled) => {
            // SAFETY: A validated, live bundled LaunchAgent and explicit
            // user request, executed on the command's main thread.
            unsafe { service.registerAndReturnError() }
        }
        (Action::Disable, Status::Enabled | Status::RequiresApproval) => {
            // SAFETY: The same validated LaunchAgent is being unregistered
            // at the user's explicit request on the command's main thread.
            unsafe { service.unregisterAndReturnError() }
        }
        _ => return Ok(before),
    };
    let after = status(service)?;
    if matches!(
        (action, after),
        (Action::Enable, Status::Enabled | Status::RequiresApproval)
            | (Action::Disable, Status::Disabled)
    ) {
        return Ok(after);
    }
    change.map_err(|error| service_error(&error))?;
    Err(io::Error::other(localized(
        "Service Management did not apply the login preference",
        "لم تطبّق إدارة الخدمات إعداد بدء التشغيل عند تسجيل الدخول",
    )))
}

fn status(service: &SMAppService) -> io::Result<Status> {
    // SAFETY: `service` is the live object returned for this signed app's plist.
    let status = unsafe { service.status() };
    match status {
        SMAppServiceStatus::Enabled => Ok(Status::Enabled),
        // macOS may return NotFound before a new bundled agent has a
        // registration, even after the plist has passed our bundle check.
        SMAppServiceStatus::NotRegistered | SMAppServiceStatus::NotFound => Ok(Status::Disabled),
        SMAppServiceStatus::RequiresApproval => Ok(Status::RequiresApproval),
        _ => Err(io::Error::other(format!(
            "{} ({})",
            localized(
                "Service Management returned an unknown status",
                "أعادت إدارة الخدمات حالة غير معروفة",
            ),
            status.0
        ))),
    }
}

fn service_error(error: &NSError) -> io::Error {
    io::Error::other(format!(
        "{} {} {}: {}",
        localized("Service Management error", "خطأ في إدارة الخدمات"),
        error.domain(),
        error.code(),
        error.localizedDescription()
    ))
}

fn missing_bundle() -> io::Error {
    io::Error::new(
        io::ErrorKind::NotFound,
        localized(
            "Login commands must run from inside a signed Marsa app bundle with its LaunchAgent",
            "شغّل أوامر تسجيل الدخول من داخل حزمة تطبيق مرسى الموقّعة التي تحتوي على وكيل بدء التشغيل",
        ),
    )
}
