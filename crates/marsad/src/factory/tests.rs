use std::{
    env, fs,
    os::unix::{fs::PermissionsExt, process::ExitStatusExt},
    process::Command,
};

use super::*;

const FIXTURE: &str = "MARSA_FACTORY_STORAGE_FIXTURE";
const CUT: &str = "MARSA_FACTORY_STORAGE_CUT";
const ROLLBACK: &str = "MARSA_FACTORY_STORAGE_ROLLBACK";
const ORIGINAL: [&[u8]; 3] = [b"original settings", b"old disk", b"old grants"];
const DEFAULT_CONFIG: &str = "schema_version = 1\n";

fn uid() -> u32 {
    nix::unistd::geteuid().as_raw()
}

fn fixture(mask: u32) -> (tempfile::TempDir, MarsaPaths, [Option<Inode>; 3]) {
    let scratch = tempfile::tempdir().unwrap();
    let paths = MarsaPaths::from_root(scratch.path()).unwrap();
    for dir in [paths.root().to_owned(), paths.vm_dir(), paths.state_dir()] {
        fs::create_dir_all(&dir).unwrap();
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).unwrap();
    }
    fs::write(paths.root().join("unrelated"), b"untouched").unwrap();
    let layout = Layout::open(&paths, uid()).unwrap();
    let mut original = [None; 3];
    for (index, member) in MEMBERS.into_iter().enumerate() {
        if mask & (1 << index) != 0 {
            let mut file = layout
                .directory(member)
                .create_file(member.names().0)
                .unwrap();
            file.write_all(ORIGINAL[index]).unwrap();
            if matches!(member, Member::Disk) {
                file.set_len(512).unwrap();
            }
            file.sync_all().unwrap();
            original[index] = Some(Inode::from(&file.metadata().unwrap()));
        }
    }
    (scratch, paths, original)
}

fn options() -> Settings {
    let mut defaults = Settings::default();
    defaults.disk.data_size_limit = 1024 * 1024 * 1024;
    defaults
}

fn kill_at(paths: &MarsaPaths, cut: u32, rollback: bool) {
    let result = Command::new(env::current_exe().unwrap())
        .args(["--exact", "factory::tests::fault_child", "--nocapture"])
        .env(FIXTURE, paths.root())
        .env(CUT, cut.to_string())
        .env(ROLLBACK, if rollback { "yes" } else { "no" })
        .output()
        .unwrap();
    assert_eq!(
        result.status.signal(),
        Some(9),
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn every_install_commit_and_cleanup_boundary_recovers_one_generation() {
    for mask in 0_u32..8 {
        for cut in 0..10 + mask.count_ones() {
            let (_scratch, paths, original) = fixture(mask);
            kill_at(&paths, cut, false);

            recover(&paths, uid()).unwrap();
            recover(&paths, uid()).unwrap();
            assert_generation(&paths, original, cut >= 8);
        }
    }
}

#[test]
fn rollback_can_itself_be_interrupted_and_retried() {
    for mask in 0_u32..8 {
        for cut in 0..3 + mask.count_ones() {
            let (_scratch, paths, original) = fixture(mask);
            kill_at(&paths, 7, false);
            kill_at(&paths, cut, true);
            recover(&paths, uid()).unwrap();
            assert_generation(&paths, original, false);
        }
    }
}

#[test]
fn unexpected_recovery_member_is_neither_removed_nor_overwritten() {
    let (_scratch, paths, _) = fixture(7);
    kill_at(&paths, 7, false);
    fs::remove_file(paths.config_file()).unwrap();
    fs::write(paths.config_file(), b"unrelated replacement").unwrap();
    assert!(recover(&paths, uid()).is_err());
    assert_eq!(
        fs::read(paths.config_file()).unwrap(),
        b"unrelated replacement"
    );
    assert_eq!(
        fs::read(paths.root().join(ACTIVE).join("old-config")).unwrap(),
        ORIGINAL[0],
    );
    assert!(paths.root().join(ACTIVE).join("old-disk").exists());
}

#[test]
fn stale_configuration_or_a_locked_disk_leaves_all_live_members_intact() {
    let (_scratch, paths, original) = fixture(7);
    let prepared = Prepared::capture(&paths, uid()).unwrap();
    let disk = fs::File::options()
        .read(true)
        .write(true)
        .open(paths.data_disk())
        .unwrap();
    disk.try_lock().unwrap();
    assert!(matches!(
        prepared.apply(&paths, uid(), &options(), DEFAULT_CONFIG),
        Err(ResetError::Refused(_))
    ));
    drop(disk);
    assert_generation(&paths, original, false);

    let prepared = Prepared::capture(&paths, uid()).unwrap();
    fs::write(paths.config_file(), b"edited after preparation").unwrap();
    assert!(matches!(
        prepared.apply(&paths, uid(), &options(), DEFAULT_CONFIG),
        Err(ResetError::Refused(_))
    ));
    assert_eq!(
        fs::read(paths.config_file()).unwrap(),
        b"edited after preparation"
    );
    assert!(!paths.root().join(ACTIVE).exists());
}

#[test]
fn fault_child() {
    let Some(root) = env::var_os(FIXTURE) else {
        return;
    };
    let paths = MarsaPaths::from_root(std::path::PathBuf::from(root)).unwrap();
    let cut: u32 = env::var(CUT).unwrap().parse().unwrap();
    let mut boundary = 0;
    let mut checkpoint = || {
        if cut == boundary {
            nix::sys::signal::kill(nix::unistd::getpid(), nix::sys::signal::Signal::SIGKILL)
                .unwrap();
            loop {
                std::thread::park();
            }
        }
        boundary += 1;
    };
    if env::var(ROLLBACK).unwrap() == "yes" {
        recover_layout(&Layout::open(&paths, uid()).unwrap(), &mut checkpoint).unwrap();
    } else {
        Prepared::capture(&paths, uid())
            .unwrap()
            .apply_with(&paths, uid(), &options(), DEFAULT_CONFIG, checkpoint)
            .unwrap();
    }
    panic!("fixture did not reach requested interruption boundary");
}

fn assert_generation(paths: &MarsaPaths, original: [Option<Inode>; 3], committed: bool) {
    let layout = Layout::open(paths, uid()).unwrap();
    for (index, member) in MEMBERS.into_iter().enumerate() {
        let live = layout.directory(member);
        let name = member.names().0;
        let file = live.entry(name, false).unwrap();
        if committed {
            match member {
                Member::Config => {
                    let mut text = String::new();
                    file.unwrap().read_to_string(&mut text).unwrap();
                    assert_eq!(text, DEFAULT_CONFIG);
                }
                Member::Disk => {
                    let metadata = file.unwrap().metadata().unwrap();
                    assert_eq!(metadata.len(), options().disk.data_size_limit);
                    assert!(metadata.blocks() * 512 < metadata.len());
                    assert_ne!(Some(Inode::from(&metadata)), original[index]);
                }
                Member::Grants => assert!(file.is_none()),
            }
        } else if let Some(identity) = original[index] {
            let file = file.unwrap();
            assert_eq!(Inode::from(&file.metadata().unwrap()), identity);
            let mut bytes = vec![0; ORIGINAL[index].len()];
            (&file).read_exact(&mut bytes).unwrap();
            assert_eq!(bytes, ORIGINAL[index]);
        } else {
            assert!(file.is_none(), "{name} unexpectedly exists");
        }
    }
    assert_eq!(
        fs::read(paths.root().join("unrelated")).unwrap(),
        b"untouched"
    );
    assert!(!paths.root().join(ACTIVE).exists());
    assert!(!paths.root().join(FINISHED).exists());
}
