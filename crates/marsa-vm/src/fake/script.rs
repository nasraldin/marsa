//! Replay bytes to test both sides of guest vsock without depending on a protocol crate.

use std::{io, time::Duration};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt, DuplexStream},
    time::sleep,
};

/// One action of a simulated guest on a vsock connection.
#[derive(Debug, Clone)]
pub enum AgentStep {
    /// Write these bytes to the host.
    Send(Vec<u8>),
    /// Read and match these exact bytes from the host.
    Expect(Vec<u8>),
    /// Wait before performing the next step.
    Wait(Duration),
}

pub(super) async fn serve(mut guest: DuplexStream, steps: Vec<AgentStep>) {
    // A script mismatch or closed peer terminates the connection, like a failed guest.
    let _ = replay(&mut guest, steps).await;
}

async fn replay(guest: &mut DuplexStream, steps: Vec<AgentStep>) -> io::Result<()> {
    for step in steps {
        match step {
            AgentStep::Send(bytes) => guest.write_all(&bytes).await?,
            AgentStep::Wait(delay) => sleep(delay).await,
            AgentStep::Expect(expected) => {
                let mut received = [0; 4096];
                for bytes in expected.chunks(received.len()) {
                    let actual = &mut received[..bytes.len()];
                    guest.read_exact(actual).await?;
                    if bytes != actual {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "guest script received unexpected host bytes",
                        ));
                    }
                }
            }
        }
    }
    guest.shutdown().await
}
