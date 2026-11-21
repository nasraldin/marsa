//! Build the greeting from release pins and the timings emitted by PID 1.

use std::{
    fs::File,
    io::{self, Read},
};

use marsa_proto::guest::{
    PROTOCOL_VERSION,
    v1::{BootTiming, ComponentVersion, Hello},
};
use serde::Deserialize;

const MAX_TIMING_BYTES: u64 = 16 * 1024;
const PINS: &str = include_str!("../../../guest/versions.toml");

#[derive(Deserialize)]
struct Version {
    version: String,
}

#[derive(Deserialize)]
struct Inputs {
    kernel: Version,
    alpine: Version,
    docker: Version,
}

pub(crate) fn hello() -> io::Result<Hello> {
    let inputs: Inputs = toml::from_str(PINS).map_err(io::Error::other)?;
    let package = env!("CARGO_PKG_VERSION").to_owned();
    let components = [
        ("linux", inputs.kernel.version),
        ("alpine", inputs.alpine.version),
        ("docker", inputs.docker.version),
        ("marsa-agent", package.clone()),
    ]
    .into_iter()
    .map(|(name, version)| ComponentVersion {
        name: name.to_owned(),
        version,
    })
    .collect();

    let mut raw = String::new();
    File::open("/run/marsa/boot-timings")?
        .take(MAX_TIMING_BYTES + 1)
        .read_to_string(&mut raw)?;
    if raw.len() as u64 > MAX_TIMING_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "boot timing record exceeds its limit",
        ));
    }
    let boot_timings = raw.lines().map(timing).collect::<io::Result<_>>()?;
    Ok(Hello {
        protocol: Some(PROTOCOL_VERSION),
        image_version: package,
        components,
        boot_timings,
    })
}

fn timing(raw: &str) -> io::Result<BootTiming> {
    let malformed = || io::Error::new(io::ErrorKind::InvalidData, "invalid PID 1 boot timing");
    let (stage, number) = raw.split_once('=').ok_or_else(malformed)?;
    let elapsed_millis = number.parse().map_err(|_| malformed())?;
    Ok(BootTiming {
        stage: stage.to_owned(),
        elapsed_millis,
    })
}
