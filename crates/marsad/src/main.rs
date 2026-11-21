//! Host daemon for the virtual machine and its local clients.

// Keep the architecture's dependency edges until this crate uses them directly.
use marsa_dockerproxy as _;
use marsa_net as _;
use marsa_proto as _;
use marsa_share as _;
use marsa_vm_krun as _;
use marsa_vm_vz as _;

use std::process::ExitCode;

fn main() -> ExitCode {
    // Report failure until this binary has a working entry point.
    ExitCode::FAILURE
}
