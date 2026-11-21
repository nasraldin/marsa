//! Echo one bounded challenge on a guest-only AF_VSOCK listener.

#[cfg(target_os = "linux")]
fn main() -> std::io::Result<()> {
    use std::io::{Read, Write};
    use std::time::Duration;

    use vsock::{VMADDR_CID_ANY, VsockListener};

    let listener = VsockListener::bind_with_cid_port(VMADDR_CID_ANY, 5_000)?;
    println!("MARSA_VSOCK_READY");
    let (mut stream, _) = listener.accept()?;
    let timeout = Some(Duration::from_secs(5));
    stream.set_read_timeout(timeout)?;
    stream.set_write_timeout(timeout)?;
    let mut challenge = [0; 16];
    stream.read_exact(&mut challenge)?;
    stream.write_all(&challenge)?;
    Ok(())
}

#[cfg(not(target_os = "linux"))]
compile_error!("build the fixture with --target aarch64-unknown-linux-musl");
