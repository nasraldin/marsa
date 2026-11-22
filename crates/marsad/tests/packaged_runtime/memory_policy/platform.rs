//! Collect the macOS memory ledger for the disposable fixture tasks.

#![allow(unsafe_code)]

use std::{io, mem::MaybeUninit};

use nix::libc;

pub(super) fn physical_footprint(pid: i32) -> io::Result<u64> {
    let mut result = MaybeUninit::<libc::rusage_info_v0>::uninit();
    // SAFETY: RUSAGE_INFO_V0 writes exactly rusage_info_v0. The pointer is
    // writable and properly aligned, and the result is only read on success.
    if unsafe { libc::proc_pid_rusage(pid, libc::RUSAGE_INFO_V0, result.as_mut_ptr().cast()) } != 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: A successful call initialized the flavor's entire structure.
    Ok(unsafe { result.assume_init() }.ri_phys_footprint)
}
