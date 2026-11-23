//! Platform operations used by the authenticated external Docker transports.

use std::{
    fs::{DirBuilder, File, OpenOptions},
    io::{self, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::Path,
    pin::Pin,
    process::Stdio,
    task::{Context, Poll},
    time::Duration,
};

use nix::{libc, unistd::geteuid};
use tokio::{
    fs,
    io::{AsyncRead, AsyncReadExt, AsyncWrite, ReadBuf},
    process::{Child, ChildStdin, ChildStdout, Command},
    time::timeout,
};

pub(crate) async fn check_user_docker_directory(path: &Path) -> io::Result<()> {
    check_owned_directory(&fs::symlink_metadata(path).await?)
}

fn check_owned_directory(directory: &std::fs::Metadata) -> io::Result<()> {
    if !directory.is_dir() || directory.uid() != geteuid().as_raw() || directory.mode() & 0o022 != 0
    {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    Ok(())
}

pub(crate) async fn read_user_docker_file(
    path: &Path,
    limit: usize,
    private: bool,
) -> io::Result<Vec<u8>> {
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .await?;
    let metadata = file.metadata().await?;
    let forbidden_modes = if private { 0o077 } else { 0o022 };
    if !metadata.is_file()
        || metadata.uid() != geteuid().as_raw()
        || metadata.mode() & forbidden_modes != 0
    {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes).await?;
    if bytes.len() > limit {
        return Err(io::ErrorKind::InvalidData.into());
    }
    Ok(bytes)
}

pub(crate) fn lock_docker_state(directory: &Path, lock_name: &str) -> io::Result<File> {
    match DirBuilder::new().mode(0o700).create(directory) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    check_owned_directory(&std::fs::symlink_metadata(directory)?)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(directory.join(lock_name))?;
    let metadata = lock.metadata()?;
    if !metadata.is_file() || metadata.uid() != geteuid().as_raw() || metadata.mode() & 0o077 != 0 {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    lock.lock()?;
    Ok(lock)
}

pub(crate) fn replace_docker_state(path: &Path, data: &[u8]) -> io::Result<()> {
    let directory = path.parent().ok_or(io::ErrorKind::InvalidInput)?;
    let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
    temporary.write_all(data)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}

pub(crate) async fn docker_ssh_config(args: &[String]) -> io::Result<String> {
    const OUTPUT_LIMIT: u64 = 32 * 1024;
    timeout(Duration::from_secs(3), async {
        let mut child = ssh_command()
            .arg("-G")
            .args(args)
            .stdin(Stdio::null())
            .spawn()?;
        let output = child.stdout.take().ok_or(io::ErrorKind::BrokenPipe)?;
        let mut data = Vec::new();
        output.take(OUTPUT_LIMIT + 1).read_to_end(&mut data).await?;
        if data.len() as u64 > OUTPUT_LIMIT || !child.wait().await?.success() {
            return Err(io::ErrorKind::InvalidData.into());
        }
        String::from_utf8(data).map_err(io::Error::other)
    })
    .await
    .map_err(|_| io::ErrorKind::TimedOut)?
}

fn ssh_command() -> Command {
    // The OS-provided client reads the user's SSH config, agent and known hosts.
    // stderr can contain host or authentication details, so it stays out of logs.
    let mut command = Command::new("/usr/bin/ssh");
    command
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    command
}

pub(crate) fn open_docker_ssh(args: &[String]) -> io::Result<SshTransport> {
    let mut child = ssh_command().args(args).stdin(Stdio::piped()).spawn()?;
    let input = child.stdin.take().ok_or(io::ErrorKind::BrokenPipe)?;
    let output = child.stdout.take().ok_or(io::ErrorKind::BrokenPipe)?;
    Ok(SshTransport {
        _child: child,
        input: Some(input),
        output,
    })
}

pub(crate) struct SshTransport {
    _child: Child,
    input: Option<ChildStdin>,
    output: ChildStdout,
}

impl AsyncRead for SshTransport {
    fn poll_read(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        bytes: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().output).poll_read(context, bytes)
    }
}

impl AsyncWrite for SshTransport {
    fn poll_write(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        match &mut self.get_mut().input {
            Some(input) => Pin::new(input).poll_write(context, bytes),
            None => Poll::Ready(Err(io::ErrorKind::BrokenPipe.into())),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        match &mut self.get_mut().input {
            Some(input) => Pin::new(input).poll_flush(context),
            None => Poll::Ready(Ok(())),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        // Closing stdin forwards EOF while leaving the server's response readable.
        self.get_mut().input.take();
        Poll::Ready(Ok(()))
    }
}
