//! Build and validate identical fixture devices for the direct Rust bridge.

use std::io;
use std::os::fd::{BorrowedFd, IntoRawFd};
use std::path::Path;

use objc2::AnyThread;
use objc2::rc::Retained;
use objc2_foundation::{NSArray, NSFileHandle, NSString, NSURL};
use objc2_virtualization::*;

use crate::runner::VmInputs;

pub(super) fn build(
    inputs: &VmInputs<'_>,
    network: BorrowedFd<'_>,
) -> io::Result<Retained<VZVirtualMachineConfiguration>> {
    let kernel = file_url(&inputs.fixtures.join("Image"))?;
    let initramfs = file_url(&inputs.fixtures.join("initramfs"))?;
    let directory = file_url(&inputs.fixtures.join("share"))?;
    let serial_read = file_handle(inputs.serial)?;
    let serial_write = file_handle(inputs.serial)?;
    let network = file_handle(network)?;
    let module_disk = block(&inputs.fixtures.join("modules.img"), true)?;
    let data_disk = block(&inputs.fixtures.join("data.img"), false)?;

    // SAFETY: All allocations use the matching documented initializers on the VM queue.
    // Setters receive retained/copyable objects. No VM starts until validation succeeds.
    unsafe {
        let loader = VZLinuxBootLoader::initWithKernelURL(VZLinuxBootLoader::alloc(), &kernel);
        loader.setInitialRamdiskURL(Some(&initramfs));
        loader.setCommandLine(&NSString::from_str(
            "console=hvc0 rdinit=/marsa-spike-init panic=-1",
        ));

        let console = VZVirtioConsoleDeviceSerialPortConfiguration::new();
        let serial =
            VZFileHandleSerialPortAttachment::initWithFileHandleForReading_fileHandleForWriting(
                VZFileHandleSerialPortAttachment::alloc(),
                Some(&serial_read),
                Some(&serial_write),
            );
        console.setAttachment(Some(&serial));

        let share = VZVirtioFileSystemDeviceConfiguration::initWithTag(
            VZVirtioFileSystemDeviceConfiguration::alloc(),
            &NSString::from_str("marsa-spike"),
        );
        let directory = VZSharedDirectory::initWithURL_readOnly(
            VZSharedDirectory::alloc(),
            &directory,
            !inputs.writable_share,
        );
        let directory =
            VZSingleDirectoryShare::initWithDirectory(VZSingleDirectoryShare::alloc(), &directory);
        share.setShare(Some(&directory));

        let nat = VZVirtioNetworkDeviceConfiguration::new();
        nat.setAttachment(Some(&VZNATNetworkDeviceAttachment::new()));
        let file_handle = VZVirtioNetworkDeviceConfiguration::new();
        let attachment = VZFileHandleNetworkDeviceAttachment::initWithFileHandle(
            VZFileHandleNetworkDeviceAttachment::alloc(),
            &network,
        );
        file_handle.setAttachment(Some(&attachment));

        let config = VZVirtualMachineConfiguration::new();
        config.setCPUCount(2);
        config.setMemorySize(1024 * 1024 * 1024);
        config.setBootLoader(Some(&loader));
        config.setSerialPorts(&NSArray::from_retained_slice(&[console.into_super()]));
        config.setStorageDevices(&NSArray::from_retained_slice(&[
            module_disk.into_super(),
            data_disk.into_super(),
        ]));
        config.setDirectorySharingDevices(&NSArray::from_retained_slice(&[share.into_super()]));
        config.setSocketDevices(&NSArray::from_retained_slice(&[
            VZVirtioSocketDeviceConfiguration::new().into_super(),
        ]));
        config.setNetworkDevices(&NSArray::from_retained_slice(&[
            nat.into_super(),
            file_handle.into_super(),
        ]));
        config.setMemoryBalloonDevices(&NSArray::from_retained_slice(&[
            VZVirtioTraditionalMemoryBalloonDeviceConfiguration::new().into_super(),
        ]));
        config.setEntropyDevices(&NSArray::from_retained_slice(&[
            VZVirtioEntropyDeviceConfiguration::new().into_super(),
        ]));
        config
            .validateWithError()
            .map_err(|error| io::Error::other(error.localizedDescription().to_string()))?;
        Ok(config)
    }
}

fn block(path: &Path, read_only: bool) -> io::Result<Retained<VZVirtioBlockDeviceConfiguration>> {
    let url = file_url(path)?;
    // SAFETY: This file URL is passed to the documented fallible initializer on the VM queue.
    unsafe {
        let attachment = VZDiskImageStorageDeviceAttachment::initWithURL_readOnly_error(
            VZDiskImageStorageDeviceAttachment::alloc(),
            &url,
            read_only,
        )
        .map_err(|error| io::Error::other(error.localizedDescription().to_string()))?;
        Ok(VZVirtioBlockDeviceConfiguration::initWithAttachment(
            VZVirtioBlockDeviceConfiguration::alloc(),
            &attachment,
        ))
    }
}

fn file_handle(fd: BorrowedFd<'_>) -> io::Result<Retained<NSFileHandle>> {
    let owned = fd.try_clone_to_owned()?;
    Ok(NSFileHandle::initWithFileDescriptor_closeOnDealloc(
        NSFileHandle::alloc(),
        owned.into_raw_fd(),
        true,
    ))
}

fn file_url(path: &Path) -> io::Result<Retained<NSURL>> {
    let path = path
        .to_str()
        .ok_or_else(|| io::Error::other("fixture paths must be UTF-8"))?;
    Ok(NSURL::fileURLWithPath(&NSString::from_str(path)))
}
