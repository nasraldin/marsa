//! Generate Rust bindings from the checked-in protobuf contracts.

use std::env;
use std::error::Error;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

const PROTOC_VERSION: &str = "libprotoc 36.2";

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo:rerun-if-changed=proto");
    println!("cargo:rerun-if-env-changed=PROTOC");
    println!("cargo:rerun-if-env-changed=PATH");

    let protoc = env::var_os("PROTOC").map_or_else(|| PathBuf::from("protoc"), PathBuf::from);
    verify_protoc(&protoc)?;

    let mut guest = prost_build::Config::new();
    guest.protoc_executable(&protoc);
    guest.compile_protos(
        &[
            "proto/marsa/guest/v1/control.proto",
            "proto/marsa/guest/v1/bind_authorization.proto",
        ],
        &["proto"],
    )?;

    #[cfg(feature = "host-api")]
    generate_host(&protoc)?;

    Ok(())
}

fn verify_protoc(protoc: &Path) -> io::Result<()> {
    let version = Command::new(protoc)
        .arg("--version")
        .output()
        .map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("protoc 36.2 is required; see crates/marsa-proto/PROTOCOL.md: {error}"),
            )
        })?;
    if version.status.success() && String::from_utf8_lossy(&version.stdout).trim() == PROTOC_VERSION
    {
        return Ok(());
    }

    Err(io::Error::other(format!(
        "expected {PROTOC_VERSION}; see crates/marsa-proto/PROTOCOL.md"
    )))
}

#[cfg(feature = "host-api")]
fn generate_host(protoc: &Path) -> io::Result<()> {
    let mut host = prost_build::Config::new();
    host.protoc_executable(protoc);
    tonic_prost_build::configure().compile_with_config(
        host,
        &[
            "proto/marsa/v1/agents.proto",
            "proto/marsa/v1/configuration.proto",
            "proto/marsa/v1/engine.proto",
            "proto/marsa/v1/events.proto",
            "proto/marsa/v1/maintenance.proto",
        ],
        &["proto"],
    )
}
