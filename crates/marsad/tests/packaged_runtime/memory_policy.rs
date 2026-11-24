//! Measure host task footprint around an offline, bounded guest memory workload.

use std::collections::BTreeSet;

#[path = "memory_policy/platform.rs"]
mod platform;
use platform::physical_footprint;

use super::{recovery, *};

const ACTIVE: u64 = 4 * GIB;
const IDLE_SECS: u32 = 4;
const GUEST_WORK: &str = "/bin/busybox dd if=/dev/urandom of=/work/touched bs=262144 count=8192 && exec /bin/busybox sleep 120";

#[tokio::test]
#[ignore = "requires an otherwise idle Apple silicon virtualization runner and pinned guest artifacts"]
async fn packaged_idle_stop_releases_host_memory() {
    assert!(!nix::unistd::geteuid().is_root());
    let pressure = output(Command::new("/usr/bin/memory_pressure").arg("-Q"));
    let pressure = String::from_utf8(pressure.stdout).unwrap();
    let free: u32 = pressure
        .lines()
        .find_map(|line| line.strip_prefix("System-wide memory free percentage: "))
        .unwrap()
        .trim_end_matches('%')
        .parse()
        .unwrap();
    assert!(free >= 30, "{pressure}");
    let physical = output(Command::new("/usr/sbin/sysctl").args(["-n", "hw.memsize"]));
    assert!(
        String::from_utf8(physical.stdout)
            .unwrap()
            .trim()
            .parse::<u64>()
            .unwrap()
            >= 16 * GIB,
        "at least 16 GiB of host RAM is required for the 4 GiB VM"
    );

    let scratch = tempfile::tempdir_in("/tmp").unwrap();
    let (daemon, _) = stage_bundle(scratch.path());
    let archive = recovery::offline_image(scratch.path());
    let home = scratch.path().join("home");
    fs::create_dir(&home).unwrap();
    let paths = MarsaPaths::from_home(&home).unwrap();
    fs::create_dir(paths.root()).unwrap();
    fs::set_permissions(paths.root(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut settings = Settings::default();
    settings.network.mode = marsa_core::settings::NetworkMode::Userspace;
    settings.vm.memory = Some(ACTIVE);
    settings.vm.idle_timeout = IDLE_SECS;
    settings.vm.idle_stop = true;
    settings.disk.data_size_limit = GIB;
    let mut config = fs::File::create(paths.config_file()).unwrap();
    config
        .set_permissions(fs::Permissions::from_mode(0o600))
        .unwrap();
    config
        .write_all(settings.to_toml().unwrap().as_bytes())
        .unwrap();
    drop(config);

    let previous_workers = workers();
    let log = scratch.path().join("daemon.log");
    let mut daemon = recovery::spawn(&daemon, &home, &log);
    let pid = daemon.0.id().try_into().unwrap();
    let channel = connect(&mut daemon.0, &paths.daemon_socket(), &log).await;
    let baseline = report("daemon_only", Instant::now(), pid, None, None);
    recovery::start(&channel, &paths, 1).await;
    let worker = timeout(WAIT, async {
        loop {
            let current = workers();
            match current
                .difference(&previous_workers)
                .copied()
                .collect::<Vec<_>>()
                .as_slice()
            {
                [] => tokio::time::sleep(Duration::from_millis(100)).await,
                [worker] => break *worker,
                _ => panic!("multiple new VZ workers appeared"),
            }
        }
    })
    .await
    .expect("VZ worker PID");
    let helper = network_helper(pid).expect("fixture network helper PID");
    let socket = paths.docker_socket();
    recovery::docker(
        &socket,
        &["import", archive.to_str().unwrap(), "memory-fixture"],
    )
    .await;
    let started = Instant::now();
    report(
        "before_work",
        started,
        pid,
        Some((worker, helper)),
        Some(memory(&channel).await),
    );
    recovery::docker(
        &socket,
        &[
            "run",
            "-d",
            "--name",
            "memory",
            "--network",
            "none",
            "--memory",
            "2600m",
            "--tmpfs",
            "/work:size=2200m",
            "memory-fixture",
            "/bin/busybox",
            "sh",
            "-c",
            GUEST_WORK,
        ],
    )
    .await;
    timeout(WAIT, async {
        loop {
            // Ready only after every byte has been written, while the tmpfs is held.
            if recovery::docker(
                &socket,
                &[
                    "exec",
                    "memory",
                    "/bin/busybox",
                    "sh",
                    "-c",
                    "[ -f /work/touched ] && /bin/busybox stat -c %s /work/touched || true",
                ],
            )
            .await
            .trim()
                == (2 * GIB).to_string()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    })
    .await
    .expect("2 GiB guest allocation");
    let peak = report(
        "work_held",
        started,
        pid,
        Some((worker, helper)),
        Some(memory(&channel).await),
    );
    assert!(peak.worker.unwrap() >= 2 * GIB, "{peak:?}");

    let stop_requested = Instant::now();
    recovery::docker(&socket, &["stop", "--time", "1", "memory"]).await;
    let ended = Instant::now();
    timeout(WAIT, async {
        loop {
            let status = EngineClient::new(channel.clone())
                .status(EngineStatusRequest {})
                .await
                .unwrap()
                .into_inner();
            if status.state() == EngineState::Stopped {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("idle timeout stops the VM");
    let elapsed = ended.elapsed();
    assert!(stop_requested.elapsed() >= Duration::from_secs(u64::from(IDLE_SECS)));
    eprintln!("state=Stopped after_ms={}", elapsed.as_millis());

    timeout(WAIT, async {
        while workers().contains(&worker) || network_helper(pid).is_some() {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("VZ worker and network helper exit");
    let after = report("after_stop", ended, pid, None, None);
    assert!(
        after.daemon.abs_diff(baseline.daemon) <= 32 * MIB,
        "baseline={baseline:?}, after={after:?}"
    );
    recovery::signal_running_daemon(&mut daemon).await;
    assert!(!workers().contains(&worker));
    assert!(network_helper(pid).is_none());
}

#[derive(Debug)]
struct Footprint {
    daemon: u64,
    worker: Option<u64>,
    helper: Option<u64>,
}

fn report(
    stage: &str,
    since: Instant,
    daemon: i32,
    runtime: Option<(i32, i32)>,
    guest_total: Option<u64>,
) -> Footprint {
    let footprint = Footprint {
        daemon: physical_footprint(daemon).unwrap(),
        worker: runtime.map(|(worker, _)| physical_footprint(worker).unwrap()),
        helper: runtime.map(|(_, helper)| physical_footprint(helper).unwrap()),
    };
    eprintln!(
        "{stage} t_ms={} daemon_bytes={} vz_bytes={} helper_bytes={} guest_total_bytes={}",
        since.elapsed().as_millis(),
        footprint.daemon,
        footprint.worker.unwrap_or_default(),
        footprint.helper.unwrap_or_default(),
        guest_total.unwrap_or_default()
    );
    footprint
}

fn network_helper(daemon: i32) -> Option<i32> {
    let processes = output(Command::new("/bin/ps").args(["-ax", "-o", "pid=,ppid=,comm="]));
    String::from_utf8(processes.stdout)
        .unwrap()
        .lines()
        .find_map(|line| {
            let mut fields = line.split_whitespace();
            let (pid, parent) = (fields.next()?, fields.next()?);
            let parent = parent.parse::<i32>().ok()?;
            (parent == daemon && fields.last()?.ends_with("/marsa-net-helper"))
                .then(|| pid.parse().unwrap())
        })
}

fn workers() -> BTreeSet<i32> {
    const VZ: &str = "/com.apple.Virtualization.VirtualMachine";
    let output = output(Command::new("/bin/ps").args(["-ax", "-o", "pid=,comm="]));
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .filter_map(|line| {
            let (pid, command) = line.trim().split_once(char::is_whitespace)?;
            command.trim().ends_with(VZ).then(|| pid.parse().unwrap())
        })
        .collect()
}

fn output(command: &mut Command) -> std::process::Output {
    let result = command.output().unwrap();
    assert!(result.status.success(), "{command:?} {result:?}");
    result
}
