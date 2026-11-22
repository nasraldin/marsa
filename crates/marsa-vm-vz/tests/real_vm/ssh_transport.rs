//! Exercise SSH framing and socket-peer isolation through the real VZ transport.

use super::{LIMIT, spec};
use marsa_vm::{Hypervisor, VmEvent, VmExitCause, VmState};
use marsa_vm_vz::VzHypervisor;
use std::{
    env, fs, io,
    os::unix::fs::FileTypeExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::UnixStream,
    time::{sleep, timeout},
};

use tokio_stream::StreamExt;

const PORT: u32 = 5050;
const MAX_FRAME: usize = 32 * 1024;
const CHALLENGE: &[u8] = b"marsa-vz-ssh-agent-fixture";

#[tokio::test]
#[ignore = "requires an entitled Apple silicon test and the prepared SSH transport initramfs"]
async fn host_initiated_signing_stays_with_the_approved_socket_peer() {
    let scratch = tempfile::tempdir_in("/tmp").unwrap();
    let guest = PathBuf::from(env::var_os("MARSA_VZ_GUEST_DIR").expect("set MARSA_VZ_GUEST_DIR"));
    let initramfs = env::var_os("MARSA_VZ_SSH_INITRAMFS").expect("set MARSA_VZ_SSH_INITRAMFS");
    let mut boot = spec(&guest, scratch.path(), "console=hvc0 rdinit=/init panic=-1");
    boot.initramfs = Some(initramfs.into());
    let console = boot.console_log.clone();
    let mut vm = VzHypervisor.create(boot).await.unwrap();
    let agent = Agent::spawn(scratch.path()).await;
    let mut transitions = vm.events();

    timeout(LIMIT, vm.start()).await.unwrap().unwrap();
    let mut vsock = timeout(LIMIT, async {
        loop {
            if let Ok(stream) = vm.connect_vsock(PORT).await {
                return stream;
            }
            sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("guest transport did not listen\n{}", transcript(&console)));
    // The guest authenticates the transport peer before accepting fixture grants.
    assert_eq!(
        timeout(LIMIT, vsock.read_u8()).await.unwrap().unwrap(),
        b'H'
    );
    vsock.write_all(b"G").await.unwrap();

    let mut upstream = UnixStream::connect(&agent.socket).await.unwrap();
    timeout(LIMIT, async {
        for expected in [11, 19, 13] {
            let request = read_frame(&mut vsock).await.unwrap();
            assert_eq!(request[0], expected);
            if permits(&request) {
                write_frame(&mut upstream, &request).await.unwrap();
                let reply = read_frame(&mut upstream).await.unwrap();
                assert_eq!(reply[0], if expected == 11 { 12 } else { 14 });
                write_frame(&mut vsock, &reply).await.unwrap();
            } else {
                assert_eq!(request, [19]); // remove all identities
                write_frame(&mut vsock, &[5]).await.unwrap();
            }
        }
    })
    .await
    .unwrap();
    wait_line(&console, "ssh-fixture: approved opener verified signature").await;
    drop(vsock);
    wait_line(
        &console,
        "ssh-fixture: revocation cut active and fresh streams",
    )
    .await;
    let events = transcript(&console);
    for expected in [
        "ssh-fixture: unsolicited guest-to-host connection denied",
        "ssh-fixture: denied opener from other cgroup",
        "ssh-fixture: approved peer pidfd signaled exit",
    ] {
        assert!(events.contains(expected), "{events}");
    }

    // The forbidden change must not have reached even this disposable agent.
    write_frame(&mut upstream, &[11]).await.unwrap();
    assert_eq!(
        read_frame(&mut upstream).await.unwrap()[..5],
        [12, 0, 0, 0, 1]
    );
    let exit = timeout(LIMIT, async {
        while let Some(event) = transitions.next().await {
            if let VmEvent::UnexpectedExit { cause, .. } = event {
                return Some(cause);
            }
        }
        None
    })
    .await
    .unwrap_or_else(|_| panic!("fixture did not power off\n{}", transcript(&console)));
    assert_eq!(exit, Some(VmExitCause::GuestPowerOff));
    assert_eq!(vm.state(), VmState::Failed);
}

fn permits(frame: &[u8]) -> bool {
    match frame {
        [11] => true, // request identities
        [13, payload @ ..] => {
            let Some((key, remaining)) = ssh_string(payload) else {
                return false;
            };
            let Some((data, flags)) = ssh_string(remaining) else {
                return false;
            };
            let Some((kind, rest)) = ssh_string(key) else {
                return false;
            };
            matches!(ssh_string(rest), Some((public, [])) if public.len() == 32)
                && kind == b"ssh-ed25519"
                && data == CHALLENGE
                && flags == [0; 4]
        }
        _ => false,
    }
}

fn ssh_string(input: &[u8]) -> Option<(&[u8], &[u8])> {
    let (length, remainder) = input.split_first_chunk::<4>()?;
    remainder.split_at_checked(u32::from_be_bytes(*length) as usize)
}

async fn read_frame(stream: &mut (impl AsyncRead + Unpin)) -> io::Result<Vec<u8>> {
    let length = stream.read_u32().await? as usize;
    if !(1..=MAX_FRAME).contains(&length) {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let mut data = vec![0; length];
    stream.read_exact(&mut data).await?;
    Ok(data)
}

async fn write_frame(stream: &mut (impl AsyncWrite + Unpin), data: &[u8]) -> io::Result<()> {
    stream.write_u32(data.len() as u32).await?;
    stream.write_all(data).await
}

async fn wait_line(console: &Path, expected: &str) {
    timeout(LIMIT, async {
        while !transcript(console).contains(expected) {
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "missing fixture milestone {expected}\n{}",
            transcript(console)
        )
    });
}

fn transcript(console: &Path) -> String {
    fs::read_to_string(console).unwrap_or_default()
}

struct Agent {
    socket: PathBuf,
    child: Child,
}

impl Agent {
    async fn spawn(home: &Path) -> Self {
        let key = home.join("disposable-key");
        let socket = home.join("agent.sock");
        let run = |binary: &str| {
            let mut command = Command::new(binary);
            command
                .env_clear()
                .env("HOME", home)
                .env("SSH_AUTH_SOCK", &socket)
                .env("SSH_ASKPASS_REQUIRE", "never")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            command
        };
        assert!(
            run("/usr/bin/ssh-keygen")
                .args(["-q", "-t", "ed25519", "-N", ""])
                .arg("-f")
                .arg(&key)
                .status()
                .unwrap()
                .success()
        );
        let child = run("/usr/bin/ssh-agent")
            .args(["-D", "-a"])
            .arg(&socket)
            .spawn()
            .unwrap();
        let agent = Self {
            socket: socket.clone(),
            child,
        };
        timeout(LIMIT, async {
            while !fs::symlink_metadata(&socket).is_ok_and(|value| value.file_type().is_socket()) {
                sleep(Duration::from_millis(15)).await;
            }
        })
        .await
        .unwrap();
        assert!(
            run("/usr/bin/ssh-add")
                .arg(&key)
                .status()
                .unwrap()
                .success()
        );
        agent
    }
}

impl Drop for Agent {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[tokio::test]
async fn bounds_agent_frames_and_denies_mutations_and_invalid_list_requests() {
    let mut oversized = &[0x00, 0x00, 0x80, 0x01][..];
    let error = read_frame(&mut oversized).await.unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);

    assert!(permits(&[11]));
    for rejected in [&[11, 0][..], &[17], &[19], &[22], &[27], &[13, 0, 0, 0, 1]] {
        assert!(!permits(rejected));
    }
}
