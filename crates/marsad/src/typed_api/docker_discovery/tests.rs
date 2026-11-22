//! Exercise context publication independently of the Docker executable.

use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::{Path, PathBuf},
};

use super::store::{self, Error, STORE_ID};
use tempfile::TempDir;

const HOST: &str = "unix:///Users/marsa-fixture/.marsa/run/docker.sock";

struct Fixture {
    _root: TempDir,
    config: PathBuf,
    stage: PathBuf,
    uid: u32,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let config = root.path().join("docker-config");
        let stage = root.path().join("stage");
        let record = record(&stage);
        fs::create_dir_all(record.parent().unwrap()).unwrap();
        fs::write(record, document(HOST)).unwrap();
        Self {
            _root: root,
            config,
            stage,
            uid: nix::unistd::geteuid().as_raw(),
        }
    }

    fn install(&self) -> Result<bool, Error> {
        store::install(&self.stage, &self.config, self.uid, HOST)
    }
}

fn record(config: &Path) -> PathBuf {
    config
        .join("contexts")
        .join("meta")
        .join(STORE_ID)
        .join("meta.json")
}

fn document(host: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "Name": "marsa",
        "Metadata": { "Description": "Marsa" },
        "Endpoints": { "docker": { "Host": host, "SkipTLSVerify": false } },
    }))
    .unwrap()
}

#[test]
fn installs_one_private_context_without_rewriting_config_or_other_contexts() {
    let f = Fixture::new();
    let other = f.config.join("contexts/meta/other/meta.json");
    fs::create_dir_all(other.parent().unwrap()).unwrap();
    fs::write(&other, b"immutable fixture").unwrap();
    let config = f.config.join("config.json");
    fs::write(
        &config,
        b"{ \"currentContext\" : \"other\", \"credsStore\" : \"fixture\" }",
    )
    .unwrap();
    let original = fs::read(&config).unwrap();
    assert!(f.install().unwrap());
    assert!(!f.install().unwrap());
    let installed = record(&f.config);
    assert_eq!(fs::read(&installed).unwrap(), document(HOST));
    assert_eq!(
        fs::metadata(&installed).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        fs::read_dir(installed.parent().unwrap()).unwrap().count(),
        1
    );
    assert_eq!(fs::read(other).unwrap(), b"immutable fixture");
    assert_eq!(fs::read(config).unwrap(), original);
}

#[test]
fn preserves_a_conflicting_marsa_record_or_tls_store() {
    let f = Fixture::new();
    let installed = record(&f.config);
    fs::create_dir_all(installed.parent().unwrap()).unwrap();
    let other_endpoint = document("unix:///Users/marsa-fixture/other.sock");
    fs::write(&installed, &other_endpoint).unwrap();
    assert!(matches!(f.install(), Err(Error::Conflict)));
    assert_eq!(fs::read(&installed).unwrap(), other_endpoint);

    fs::write(&installed, document(HOST)).unwrap();
    let tls = f.config.join("contexts/tls").join(STORE_ID);
    fs::create_dir_all(&tls).unwrap();
    fs::write(tls.join("fixture.pem"), b"fixture").unwrap();
    assert!(matches!(f.install(), Err(Error::Conflict)));
    assert!(tls.join("fixture.pem").exists());
    assert_eq!(fs::read(&installed).unwrap(), document(HOST));
}

#[test]
fn refuses_symlinked_or_group_writable_locations() {
    let f = Fixture::new();
    let outside = f._root.path().join("outside");
    fs::create_dir(&outside).unwrap();
    symlink(&outside, &f.config).unwrap();
    assert!(matches!(f.install(), Err(Error::Unsafe)));
    assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);

    fs::remove_file(&f.config).unwrap();
    let meta = f.config.join("contexts/meta");
    fs::create_dir_all(&meta).unwrap();
    symlink(&outside, meta.join(STORE_ID)).unwrap();
    assert!(matches!(f.install(), Err(Error::Unsafe)));
    assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);

    fs::set_permissions(&meta, fs::Permissions::from_mode(0o775)).unwrap();
    assert!(matches!(f.install(), Err(Error::Unsafe)));
}

#[test]
fn invalid_staged_metadata_creates_no_config_directory() {
    let f = Fixture::new();
    fs::write(record(&f.stage), document("unix:///tmp/wrong.sock")).unwrap();
    assert!(matches!(f.install(), Err(Error::Stage)));
    assert!(!f.config.exists());
}

#[test]
fn an_interrupted_stage_does_not_occupy_the_context_name() {
    let f = Fixture::new();
    let contexts = f.config.join("contexts");
    fs::create_dir_all(contexts.join("meta")).unwrap();
    let fd = fs::File::open(&contexts).unwrap();
    let mut partial = document(HOST);
    partial.truncate(12);
    let abandoned = store::stage_record(&fd, f.uid, &partial).unwrap();

    // Simulate abrupt termination before publication. Docker's metadata
    // scanner cannot see this directory and a fresh caller can install.
    assert!(!record(&f.config).exists());
    assert!(!contexts.join("meta").join(&abandoned).exists());
    assert!(f.install().unwrap());
    assert!(!f.install().unwrap());
    assert_eq!(fs::read(record(&f.config)).unwrap(), document(HOST));
    assert_eq!(
        fs::read(contexts.join(abandoned).join("meta.json")).unwrap(),
        partial
    );
}
