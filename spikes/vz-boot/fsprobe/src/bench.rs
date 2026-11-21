//! Run identical workloads and monotonic timing on both filesystems.

use std::fs::{self, File};
use std::io::{self, Read};
use std::path::Path;
use std::time::Instant;

const FILES: usize = 2048;
const SMALL_BYTES: usize = 4096;
const LARGE_BYTES: u64 = 128 * 1024 * 1024;
const WALK_FILES: usize = 128 * 128;
const REPETITIONS: usize = 5;

pub(super) fn run(root: &Path) -> io::Result<()> {
    let started = Instant::now();
    let mut entries = 0;
    for directory in fs::read_dir(root.join("walk"))? {
        let directory = directory?;
        for entry in fs::read_dir(directory.path())? {
            let entry = entry?;
            if !fs::symlink_metadata(entry.path())?.is_file() {
                return Err(io::Error::other("metadata walk found a non-file entry"));
            }
            entries += 1;
        }
    }
    if entries != WALK_FILES {
        return Err(io::Error::other("unexpected metadata walk count"));
    }
    println!(
        "VF_WALK count={entries} elapsed_ns={}",
        started.elapsed().as_nanos()
    );

    small_reads(root)?;
    large_reads(root)
}

fn small_reads(root: &Path) -> io::Result<()> {
    let paths: Vec<_> = (0..FILES)
        .map(|index| root.join(format!("small/{:04}.dat", index * 4051 % FILES)))
        .collect();
    let mut observations = Vec::with_capacity(FILES * REPETITIONS);
    let mut buffer = [0; SMALL_BYTES];
    for repetition in 0..=REPETITIONS {
        for path in &paths {
            let start = Instant::now();
            File::open(path)?.read_exact(&mut buffer)?;
            let elapsed = start.elapsed().as_nanos();
            if repetition != 0 {
                observations.push(elapsed);
            }
        }
    }
    observations.sort_unstable();
    let count = observations.len();
    println!(
        "VF_SMALL bytes={SMALL_BYTES} count={count} p50_ns={} p95_ns={}",
        observations[(count - 1) / 2],
        observations[(count - 1) * 95 / 100],
    );
    Ok(())
}

fn large_reads(root: &Path) -> io::Result<()> {
    let path = root.join("large.dat");
    let mut buffer = vec![0u8; 1024 * 1024];
    for repetition in 0..=REPETITIONS {
        let start = Instant::now();
        let mut file = File::open(&path)?;
        let mut bytes = 0;
        loop {
            let count = file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            bytes += count as u64;
        }
        let elapsed = start.elapsed().as_nanos();
        if bytes != LARGE_BYTES {
            return Err(io::Error::other("unexpected large read count"));
        }
        if repetition != 0 {
            println!("VF_LARGE run={repetition} bytes={bytes} elapsed_ns={elapsed}");
        }
    }
    Ok(())
}
