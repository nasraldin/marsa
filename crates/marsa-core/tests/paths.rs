//! Tests the filesystem layout independently of the real user account.

use marsa_core::paths::MarsaPaths;
use std::path::Path;

#[test]
fn an_override_relocates_every_host_path() {
    let base = Path::new("/tmp/marsa-test");
    let paths = MarsaPaths::from_root(base).unwrap();

    for (actual, relative) in [
        (paths.root().to_path_buf(), ""),
        (paths.config_file(), "config.toml"),
        (paths.run_dir(), "run"),
        (paths.daemon_socket(), "run/marsad.sock"),
        (paths.docker_socket(), "run/docker.sock"),
        (paths.daemon_pid_file(), "run/marsad.pid"),
        (paths.daemon_lock_file(), "run/marsad.lock"),
        (paths.vm_dir(), "vm"),
        (paths.data_disk(), "vm/data.img"),
        (paths.console_log(), "vm/console.log"),
        (paths.logs_dir(), "logs"),
        (paths.daemon_log(), "logs/marsad.log"),
        (paths.state_dir(), "state"),
        (paths.grants_file(), "state/grants.json"),
        (paths.update_dir(), "state/update"),
        (paths.bin_dir(), "bin"),
    ] {
        assert_eq!(actual, base.join(relative));
    }
}

#[test]
fn default_root_is_inside_the_given_home() {
    let paths = MarsaPaths::from_home("/Users/example").unwrap();
    assert_eq!(paths.root().file_name().unwrap(), ".marsa");
    assert_eq!(paths.root().parent().unwrap(), Path::new("/Users/example"));
}

#[test]
fn relative_and_traversing_roots_are_rejected() {
    for root in ["project", "/tmp/../project", "/"] {
        assert!(MarsaPaths::from_root(root).is_err());
    }
}
