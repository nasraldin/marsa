use std::{
    env, fs,
    os::unix::process::ExitStatusExt,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use super::*;

const INTERRUPT_ROOT: &str = "MARSA_TEST_SETTINGS_STAGE_ROOT";
const PREVIOUS: &str = "schema_version = 1\n[vm]\ncpus = 4\n";
const DEFAULTS: &str = "schema_version = 1\n";

fn private_root() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    fs::set_permissions(root.path(), Permissions::from_mode(0o700)).unwrap();
    root
}

fn uid() -> u32 {
    nix::unistd::geteuid().as_raw()
}

#[test]
fn reset_rechecks_the_member_after_staging() {
    let root = private_root();
    let config = root.path().join(CONFIG);
    fs::write(&config, PREVIOUS).unwrap();
    let prepared = prepare(root.path(), uid()).unwrap();
    let changed = PREVIOUS.replace('4', "5");

    let result = persist_with(root.path(), uid(), DEFAULTS, Some(&prepared), || {
        fs::write(&config, &changed)
    });
    assert!(matches!(result, Err(WriteError::Changed)));
    assert_eq!(fs::read_to_string(config).unwrap(), changed);
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
}

#[test]
fn a_kill_before_rename_keeps_the_complete_previous_document() {
    let root = private_root();
    let config = root.path().join(CONFIG);
    fs::write(&config, PREVIOUS).unwrap();
    let mut child = StagingProcess(
        Command::new(env::current_exe().unwrap())
            .args([
                "--exact",
                "typed_api::settings::storage::tests::staging_child",
                "--nocapture",
            ])
            .env(INTERRUPT_ROOT, root.path())
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while !root.path().join("stage-synced").exists() {
        assert!(child.0.try_wait().unwrap().is_none());
        assert!(Instant::now() < deadline, "staging fixture timed out");
        thread::sleep(Duration::from_millis(10));
    }
    child.0.kill().unwrap();
    assert_eq!(child.0.wait().unwrap().signal(), Some(9));
    assert_eq!(fs::read_to_string(&config).unwrap(), PREVIOUS);

    let prepared = prepare(root.path(), uid()).unwrap();
    persist(root.path(), uid(), DEFAULTS, Some(&prepared)).unwrap();
    assert_eq!(fs::read_to_string(config).unwrap(), DEFAULTS);
}

#[test]
fn staging_child() {
    let Some(root) = env::var_os(INTERRUPT_ROOT) else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    let prepared = prepare(&root, uid()).unwrap();
    persist_with(&root, uid(), DEFAULTS, Some(&prepared), || {
        fs::write(root.join("stage-synced"), [])?;
        loop {
            thread::park();
        }
    })
    .unwrap();
}

struct StagingProcess(Child);

impl Drop for StagingProcess {
    fn drop(&mut self) {
        if self.0.try_wait().is_ok_and(|status| status.is_none()) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

#[test]
fn a_replaced_staging_inode_is_neither_published_nor_removed() {
    let root = private_root();
    let config = root.path().join(CONFIG);
    fs::write(&config, PREVIOUS).unwrap();
    let prepared = prepare(root.path(), uid()).unwrap();
    let mut substituted = None;

    let result = persist_with(root.path(), uid(), DEFAULTS, Some(&prepared), || {
        let staging = fs::read_dir(root.path())?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| {
                path.file_name()
                    .unwrap()
                    .as_encoded_bytes()
                    .starts_with(b".settings.")
            })
            .unwrap();
        fs::rename(&staging, root.path().join("original-stage"))?;
        fs::write(&staging, b"substitute")?;
        substituted = Some(staging);
        Ok(())
    });
    assert!(matches!(result, Err(WriteError::Changed)));
    assert_eq!(fs::read_to_string(config).unwrap(), PREVIOUS);
    assert_eq!(fs::read(substituted.unwrap()).unwrap(), b"substitute");
}
