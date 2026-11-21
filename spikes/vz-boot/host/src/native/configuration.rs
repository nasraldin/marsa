//! Build and validate identical fixture devices for the direct Rust bridge.

use std::fs::{File, symlink_metadata};
use std::io::{self, Read};
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
    let data_disk = image_block(
        &inputs.fixtures.join("data.img"),
        false,
        cache_mode(inputs.fixtures)?,
    )?;

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
        let directory =
            VZSharedDirectory::initWithURL_readOnly(VZSharedDirectory::alloc(), &directory, true);
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
    image_block(path, read_only, None)
}

fn cache_mode(fixtures: &Path) -> io::Result<Option<VZDiskImageCachingMode>> {
    let path = fixtures.join("disk-cache");
    match symlink_metadata(&path) {
        Ok(metadata) if metadata.is_file() && metadata.len() <= 32 => {}
        Ok(_) => return Err(io::Error::other("disk-cache must be a small fixture file")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut value = String::new();
    File::open(path)?.take(33).read_to_string(&mut value)?;
    if value.len() > 32 {
        return Err(io::Error::other("disk-cache must be a small fixture file"));
    }
    let mode = match value.trim() {
        "automatic" => VZDiskImageCachingMode::Automatic,
        "cached" => VZDiskImageCachingMode::Cached,
        "uncached" => VZDiskImageCachingMode::Uncached,
        _ => return Err(io::Error::other("invalid fixture disk-cache mode")),
    };
    Ok(Some(mode))
}

fn image_block(
    path: &Path,
    read_only: bool,
    cache: Option<VZDiskImageCachingMode>,
) -> io::Result<Retained<VZVirtioBlockDeviceConfiguration>> {
    let url = file_url(path)?;
    // SAFETY: These documented fallible initializers run on the VM queue with a file URL.
    // The explicit cache experiments hold synchronization mode at Full for comparability.
    unsafe {
        let allocated = VZDiskImageStorageDeviceAttachment::alloc();
        let attachment = if let Some(cache) = cache {
            VZDiskImageStorageDeviceAttachment::initWithURL_readOnly_cachingMode_synchronizationMode_error(
                allocated,
                &url,
                read_only,
                cache,
                VZDiskImageSynchronizationMode::Full,
            )
        } else {
            VZDiskImageStorageDeviceAttachment::initWithURL_readOnly_error(
                allocated, &url, read_only,
            )
        }
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
