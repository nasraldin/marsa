//! Invalid specifications fail before an entitled VM can start.

#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use std::{fs, path::Path};

use marsa_vm::{DiskSpec, Hypervisor, NetworkSpec, VmError, VmSpec};
use marsa_vm_vz::VzHypervisor;

fn write_spec(directory: &Path) -> VmSpec {
    let kernel = directory.join("Image");
    let mut image = [0; 64];
    image[56..60].copy_from_slice(b"ARM\x64");
    fs::write(&kernel, image).unwrap();
    for disk in ["rootfs.img", "data.img"] {
        fs::write(directory.join(disk), [0; 512]).unwrap();
    }
    VmSpec {
        cpus: 2,
        memory_bytes: 1024 * 1024 * 1024,
        kernel,
        initramfs: None,
        cmdline: "console=hvc0 root=/dev/vda ro".into(),
        root_disk: DiskSpec {
            path: directory.join("rootfs.img"),
            read_only: true,
        },
        data_disk: DiskSpec {
            path: directory.join("data.img"),
            read_only: false,
        },
        shares: Vec::new(),
        network: NetworkSpec::Nat,
        console_log: directory.join("console.log"),
    }
}

#[tokio::test]
async fn rejects_misaligned_memory_before_creating_a_vm() {
    let scratch = tempfile::tempdir().unwrap();
    let mut spec = write_spec(scratch.path());
    spec.memory_bytes += 1;
    assert!(matches!(
        VzHypervisor.create(spec).await,
        Err(VmError::InvalidSpec {
            field: "memory_bytes",
            ..
        })
    ));
}

#[tokio::test]
async fn rejects_hardlinked_writable_data_and_root_images() {
    let scratch = tempfile::tempdir().unwrap();
    let spec = write_spec(scratch.path());
    fs::remove_file(&spec.data_disk.path).unwrap();
    fs::hard_link(&spec.root_disk.path, &spec.data_disk.path).unwrap();

    assert!(matches!(
        VzHypervisor.create(spec).await,
        Err(VmError::InvalidSpec {
            field: "data_disk",
            ..
        })
    ));
}

#[tokio::test]
async fn rejects_a_console_path_that_would_write_the_kernel() {
    let scratch = tempfile::tempdir().unwrap();
    let spec = write_spec(scratch.path());
    fs::hard_link(&spec.kernel, &spec.console_log).unwrap();
    let kernel_before = fs::read(&spec.kernel).unwrap();
    let kernel = spec.kernel.clone();

    assert!(matches!(
        VzHypervisor.create(spec).await,
        Err(VmError::InvalidSpec {
            field: "console_log",
            ..
        })
    ));
    assert_eq!(fs::read(kernel).unwrap(), kernel_before);
}

#[tokio::test]
async fn rejects_cpu_count_above_the_framework_limit() {
    let scratch = tempfile::tempdir().unwrap();
    let mut spec = write_spec(scratch.path());
    spec.cpus = u32::MAX;
    assert!(matches!(
        VzHypervisor.create(spec).await,
        Err(VmError::InvalidSpec { field: "cpus", .. })
    ));
}
