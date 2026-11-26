//! Stage the signed application, call its Unix API and inspect the sparse disk.

use super::*;

pub(super) fn stage_bundle(scratch: &Path) -> (PathBuf, PathBuf) {
    let source = PathBuf::from(env::var_os("MARSA_VZ_GUEST_DIR").expect("set MARSA_VZ_GUEST_DIR"));
    stage_bundle_from(scratch, &source)
}

pub(super) fn stage_bundle_from(scratch: &Path, source: &Path) -> (PathBuf, PathBuf) {
    stage_bundle_with_daemon(scratch, source, Path::new(env!("CARGO_BIN_EXE_marsad")))
}

pub(super) fn stage_bundle_with_daemon(
    scratch: &Path,
    source: &Path,
    binary: &Path,
) -> (PathBuf, PathBuf) {
    let helper =
        PathBuf::from(env::var_os("MARSA_VZ_NETWORK_HELPER").expect("set MARSA_VZ_NETWORK_HELPER"));
    let app = scratch.join("Marsa.app");
    let contents = app.join("Contents");
    let executables = contents.join("MacOS");
    let helpers = contents.join("Helpers");
    let guest = contents.join("Resources/guest");
    for path in [&executables, &helpers, &guest] {
        fs::create_dir_all(path).unwrap();
    }
    for name in ["Image", "rootfs.img", "guest-manifest.json"] {
        fs::copy(source.join(name), guest.join(name)).unwrap();
    }
    fs::write(contents.join("Info.plist"), r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>io.github.nasraldin.marsa.packaged-runtime-test</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleExecutable</key><string>marsad</string>
<key>CFBundleVersion</key><string>1</string>
</dict></plist>"#).unwrap();
    let daemon = executables.join("marsad");
    fs::copy(binary, &daemon).unwrap();
    let packaged_helper = helpers.join("marsa-net-helper");
    fs::copy(helper, &packaged_helper).unwrap();
    // Keep a valid build signature so the test launches the same helper identity.
    if !Command::new("/usr/bin/codesign")
        .args(["--verify", "--strict"])
        .arg(&packaged_helper)
        .output()
        .unwrap()
        .status
        .success()
    {
        checked(
            Command::new("/usr/bin/codesign")
                .args(["--force", "--sign", "-", "--timestamp=none"])
                .arg(&packaged_helper),
        );
    }
    let entitlement =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spikes/vz-boot/entitlement.plist");
    checked(
        Command::new("/usr/bin/codesign")
            .args([
                "--force",
                "--sign",
                "-",
                "--timestamp=none",
                "--entitlements",
            ])
            .arg(entitlement)
            .arg(&app),
    );
    checked(
        Command::new("/usr/bin/codesign")
            .args(["--verify", "--deep", "--strict"])
            .arg(app),
    );
    (daemon, guest)
}

pub(super) async fn connect(process: &mut Child, socket: &Path, logs: &Path) -> Channel {
    timeout(WAIT, async {
        loop {
            assert!(
                process.try_wait().unwrap().is_none(),
                "daemon exited during startup\n{}",
                fs::read_to_string(logs).unwrap()
            );
            let path = socket.to_path_buf();
            if let Ok(channel) = Endpoint::from_static("http://localhost")
                .connect_with_connector(service_fn(move |_| {
                    let path = path.clone();
                    async move { UnixStream::connect(path).await.map(TokioIo::new) }
                }))
                .await
            {
                return channel;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap()
}

pub(super) async fn active_network(events: &mut Streaming<DaemonEvent>) -> NetworkStatus {
    timeout(WAIT, async {
        while let Some(event) = events.message().await.unwrap() {
            if let Some(Body::Network(network)) = event.body
                && network.active_mode() != NetworkMode::Unspecified
            {
                return network;
            }
        }
        panic!("the packaged daemon did not publish an active network");
    })
    .await
    .unwrap()
}

pub(super) async fn memory(channel: &Channel) -> u64 {
    let mut stats = StatsClient::new(channel.clone())
        .vm_stats(VmStatsRequest {})
        .await
        .unwrap()
        .into_inner();
    let sample = timeout(WAIT, stats.message())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(sample.observed_at_unix_millis > 0);
    assert!(sample.disk_total_bytes > 0);
    assert!(sample.memory_total_bytes >= sample.memory_used_bytes);
    sample.memory_total_bytes
}

pub(super) async fn replace_settings(channel: &Channel, settings: &Settings, step: u32) -> bool {
    let mut client = SettingsClient::new(channel.clone());
    let current = client
        .get(GetSettingsRequest {})
        .await
        .unwrap()
        .into_inner();
    client
        .update(UpdateSettingsRequest {
            request_id: action(step).request_id,
            expected_revision: current.revision,
            settings_toml: settings.to_toml().unwrap(),
        })
        .await
        .unwrap()
        .into_inner()
        .restart_required
}

pub(super) fn assert_sparse_image_equal(before: &Path, after: &Path) {
    let before = fs::File::open(before).unwrap();
    let after = fs::File::open(after).unwrap();
    let size: i64 = before.metadata().unwrap().len().try_into().unwrap();
    assert_eq!(
        before.metadata().unwrap().len(),
        after.metadata().unwrap().len()
    );
    let mut offset = 0;
    let mut old = [0; 65536];
    let mut new = [0; 65536];
    // Holes read as zero. Compare their boundaries and every byte in the data extents.
    while offset < size {
        let data = lseek(&before, offset, Whence::SeekData);
        assert_eq!(data, lseek(&after, offset, Whence::SeekData));
        if data == Err(Errno::ENXIO) {
            break;
        }
        let start = data.unwrap();
        let hole = lseek(&before, start, Whence::SeekHole).unwrap();
        assert_eq!(hole, lseek(&after, start, Whence::SeekHole).unwrap());
        assert!(offset <= start && start < hole && hole <= size);
        let mut position = start;
        while position < hole {
            let count = usize::try_from((hole - position).min(old.len() as i64)).unwrap();
            let at = u64::try_from(position).unwrap();
            before.read_exact_at(&mut old[..count], at).unwrap();
            after.read_exact_at(&mut new[..count], at).unwrap();
            assert!(
                old[..count] == new[..count],
                "data disk bytes differ at offset {position}"
            );
            position += count as i64;
        }
        offset = hole;
    }
}

pub(super) fn disk_identity(path: &Path) -> (u64, u64) {
    let info = fs::symlink_metadata(path).unwrap();
    (info.dev(), info.ino())
}

pub(super) fn action(step: u32) -> EngineActionRequest {
    EngineActionRequest {
        request_id: format!("22000000-0000-4000-8000-{step:012}"),
    }
}

pub(super) fn checked(command: &mut Command) {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{command:?}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

pub(super) struct Daemon(pub(super) Child);

impl Drop for Daemon {
    fn drop(&mut self) {
        if self.0.try_wait().is_ok_and(|status| status.is_some()) {
            return;
        }
        if let Ok(pid) = self.0.id().try_into() {
            let _ = nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(pid),
                nix::sys::signal::Signal::SIGTERM,
            );
        }
        let deadline = Instant::now() + WAIT;
        while Instant::now() < deadline {
            if self.0.try_wait().is_ok_and(|status| status.is_some()) {
                return;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
