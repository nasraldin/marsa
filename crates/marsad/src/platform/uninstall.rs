//! Remove only the current owner's confirmed Marsa installation state.

use std::{
    fs,
    io::{self, BufRead, Read, Write},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

use marsa_core::paths::MarsaPaths;

use super::{files, login, require_unprivileged};
use crate::diagnostics::localized;

const CONFIRMATION: &str = "UNINSTALL MARSA";
const BUNDLE_ID: &str = "io.github.nasraldin.marsa";
const SYSTEM_DOCKER_SOCKET: &str = "/var/run/docker.sock";

pub(crate) fn run() -> io::Result<()> {
    let uid = require_unprivileged()?.as_raw();
    confirm(&mut io::stdin().lock(), &mut io::stderr().lock())?;
    let home = std::env::home_dir().ok_or_else(|| {
        message(
            "Could not find your home folder",
            "تعذّر العثور على مجلدك الشخصي",
        )
    })?;
    let paths = MarsaPaths::from_home(&home).map_err(io::Error::other)?;
    let home = home.canonicalize()?;
    if !owned_directory(&home, uid)? {
        return Err(io::ErrorKind::NotFound.into());
    }
    let (_, identifier, _) = login::bundled_agent()?;
    if identifier != BUNDLE_ID {
        return Err(message(
            "Run uninstall from the installed Marsa app bundle",
            "شغّل إلغاء التثبيت من حزمة تطبيق مرسى المثبّتة",
        ));
    }
    clear(&home, &paths, uid)?;
    println!(
        "{}",
        localized(
            "Marsa user data and integrations were removed. Remove the app to finish uninstalling.",
            "أزيلت بيانات مرسى وتكاملاته. أزل التطبيق لإكمال إلغاء التثبيت.",
        )
    );
    Ok(())
}

fn confirm(input: &mut impl BufRead, output: &mut impl Write) -> io::Result<()> {
    writeln!(
        output,
        "{}",
        localized(
            "This permanently deletes Marsa workloads, settings, logs and grants. Type UNINSTALL MARSA to continue.",
            "يحذف ذلك نهائيًا أعباء العمل والإعدادات والسجلات والأذونات الخاصة بمرسى. اكتب UNINSTALL MARSA للمتابعة.",
        )
    )?;
    output.flush()?;
    let mut answer = String::new();
    input.take(64).read_line(&mut answer)?;
    if answer
        .strip_suffix("\r\n")
        .or_else(|| answer.strip_suffix('\n'))
        == Some(CONFIRMATION)
    {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::Interrupted,
            localized("Uninstall canceled", "أُلغي إلغاء التثبيت"),
        ))
    }
}

fn clear(home: &Path, paths: &MarsaPaths, uid: u32) -> io::Result<()> {
    require_removed_socket(Path::new(SYSTEM_DOCKER_SOCKET), &paths.docker_socket())?;
    let canonical_paths = MarsaPaths::from_home(home).map_err(io::Error::other)?;
    require_removed_socket(
        Path::new(SYSTEM_DOCKER_SOCKET),
        &canonical_paths.docker_socket(),
    )?;
    let data = owned_directory(paths.root(), uid)?;
    let artifacts = user_artifacts(home, uid)?;
    let docker = std::env::var_os("DOCKER_CONFIG")
        .filter(|value| !value.is_empty())
        .map_or_else(|| home.join(".docker"), PathBuf::from);
    let context = marsad::typed_api::prepare_context_removal(&docker, &paths.docker_socket(), uid)
        .map_err(context_error)?;
    require_desktop_closed()?;
    login::disable_installed_agent()?;
    let _instance = data
        .then(|| files::Instance::acquire(paths, uid))
        .transpose()
        .map_err(|error| {
            if error.kind() == io::ErrorKind::AlreadyExists {
                message(
                    "Marsa is still running. Quit Marsa and retry uninstall.",
                    "ما زال مرسى قيد التشغيل. أنهِ مرسى وأعد محاولة إلغاء التثبيت.",
                )
            } else {
                error
            }
        })?;
    context.remove().map_err(context_error)?;
    for (path, directory) in artifacts {
        if directory {
            fs::remove_dir_all(path)?;
        } else {
            fs::remove_file(path)?;
        }
    }
    if data {
        fs::remove_dir_all(paths.root())?;
    }
    Ok(())
}

fn require_removed_socket(link: &Path, socket: &Path) -> io::Result<()> {
    match fs::read_link(link) {
        Ok(target) if target == socket => Err(message(
            "Remove the Docker socket link in Marsa Settings, then quit Marsa and retry.",
            "أزل رابط مقبس دوكر من إعدادات مرسى، ثم أنهِ مرسى وأعد المحاولة.",
        )),
        Ok(_) => Ok(()),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::InvalidInput
            ) =>
        {
            Ok(())
        }
        Err(error) => Err(error),
    }
}

fn require_desktop_closed() -> io::Result<()> {
    use objc2::rc::autoreleasepool;
    use objc2_app_kit::NSRunningApplication;
    use objc2_foundation::NSString;

    let running = autoreleasepool(|_| {
        let applications = NSRunningApplication::runningApplicationsWithBundleIdentifier(
            &NSString::from_str(BUNDLE_ID),
        );
        applications.iter().any(|application| {
            application
                .executableURL()
                .and_then(|url| url.lastPathComponent())
                .is_none_or(|name| name.to_string() == "marsa-gui")
        })
    });
    if running {
        return Err(message(
            "Quit the Marsa desktop before uninstalling.",
            "أنه تطبيق مرسى لسطح المكتب قبل إلغاء التثبيت.",
        ));
    }
    Ok(())
}

fn user_artifacts(home: &Path, uid: u32) -> io::Result<Vec<(PathBuf, bool)>> {
    let library = home.join("Library");
    let mut artifacts = Vec::new();
    if !owned_directory(&library, uid)? {
        return Ok(artifacts);
    }
    for (parent, member, directory) in [
        ("Application Support", BUNDLE_ID, true),
        ("Caches", BUNDLE_ID, true),
        ("Preferences", "io.github.nasraldin.marsa.plist", false),
        (
            "Saved Application State",
            "io.github.nasraldin.marsa.savedState",
            true,
        ),
    ] {
        let parent = library.join(parent);
        if !owned_directory(&parent, uid)? {
            continue;
        }
        let path = parent.join(member);
        if owned_entry(&path, uid, directory)? {
            artifacts.push((path, directory));
        }
    }
    Ok(artifacts)
}

fn owned_directory(path: &Path, uid: u32) -> io::Result<bool> {
    owned_entry(path, uid, true)
}

fn owned_entry(path: &Path, uid: u32, directory: bool) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata)
            if metadata.uid() == uid
                && metadata.mode() & 0o022 == 0
                && ((directory && metadata.is_dir()) || (!directory && metadata.is_file())) =>
        {
            Ok(true)
        }
        Ok(_) => Err(message(
            "A Marsa path is not a safe owner-controlled location",
            "أحد مسارات مرسى ليس موقعًا آمنًا يملكه المستخدم",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn context_error(error: marsad::typed_api::ContextRemovalError) -> io::Error {
    match error {
        marsad::typed_api::ContextRemovalError::Active => message(
            "Switch Docker to a different context using docker context use, then retry.",
            "بدّل دوكر إلى سياق آخر باستخدام docker context use، ثم أعد المحاولة.",
        ),
        marsad::typed_api::ContextRemovalError::Unsafe => message(
            "Could not safely inspect or remove the Marsa Docker context",
            "تعذّر فحص سياق دوكر الخاص بمرسى أو إزالته بأمان",
        ),
        marsad::typed_api::ContextRemovalError::Io(error) => error,
    }
}

fn message(english: &'static str, arabic: &'static str) -> io::Error {
    io::Error::other(localized(english, arabic))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn a_matching_even_dangling_system_link_requires_the_settings_removal() {
        let fixture = tempfile::tempdir().unwrap();
        let owner = fixture.path().join(".marsa/run/docker.sock");
        let public = fixture.path().join("docker.sock");
        symlink(&owner, &public).unwrap();
        assert!(require_removed_socket(&public, &owner).is_err());
        assert_eq!(fs::read_link(&public).unwrap(), owner);
    }

    #[test]
    fn another_engines_socket_and_an_absent_link_are_safe_to_leave() {
        let fixture = tempfile::tempdir().unwrap();
        let owner = fixture.path().join(".marsa/run/docker.sock");
        let public = fixture.path().join("docker.sock");
        let foreign = fixture.path().join("other-engine.sock");
        require_removed_socket(&public, &owner).unwrap();
        symlink(&foreign, &public).unwrap();
        require_removed_socket(&public, &owner).unwrap();
        assert_eq!(fs::read_link(&public).unwrap(), foreign);
    }

    #[test]
    fn bundled_artifact_selection_preserves_foreign_paths_and_rejects_symlinks() {
        let home = tempfile::tempdir().unwrap();
        let prefs = home.path().join("Library/Preferences");
        fs::create_dir_all(&prefs).unwrap();
        let other = prefs.join("org.example.plist");
        fs::write(&other, "unrelated").unwrap();
        let ours = prefs.join("io.github.nasraldin.marsa.plist");
        fs::write(&ours, "owned").unwrap();

        let uid = nix::unistd::geteuid().as_raw();
        assert_eq!(
            user_artifacts(home.path(), uid).unwrap(),
            [(ours.clone(), false)]
        );
        fs::remove_file(&ours).unwrap();
        symlink(&other, &ours).unwrap();
        assert!(user_artifacts(home.path(), uid).is_err());
        assert_eq!(fs::read_to_string(other).unwrap(), "unrelated");
    }
}
