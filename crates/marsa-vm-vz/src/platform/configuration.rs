//! Build and validate the minimal Virtualization.framework configuration on its queue.

use std::{fs::File, os::fd::IntoRawFd, path::Path};

use marsa_vm::{DiskSpec, VmError};
use objc2::{AnyThread, rc::Retained};
use objc2_foundation::{NSArray, NSFileHandle, NSString, NSURL};
use objc2_virtualization::{
    VZDiskImageStorageDeviceAttachment, VZFileHandleSerialPortAttachment, VZLinuxBootLoader,
    VZNATNetworkDeviceAttachment, VZVirtioBlockDeviceConfiguration,
    VZVirtioConsoleDeviceSerialPortConfiguration, VZVirtioEntropyDeviceConfiguration,
    VZVirtioNetworkDeviceConfiguration, VZVirtioSocketDeviceConfiguration, VZVirtualMachine,
    VZVirtualMachineConfiguration,
};

use super::{
    spec::Prepared,
    state::{backend, invalid_spec},
};

pub(super) fn build(
    prepared: Prepared,
) -> Result<Retained<VZVirtualMachineConfiguration>, VmError> {
    let spec = prepared.spec;
    let kernel = file_url("kernel", &spec.kernel)?;
    let initramfs = spec
        .initramfs
        .as_deref()
        .map(|path| file_url("initramfs", path))
        .transpose()?;

    // SAFETY: All framework allocations, configuration calls and drops occur on the
    // controller's serial queue. The device setters retain or copy their inputs.
    unsafe {
        let cpu_limits = VZVirtualMachineConfiguration::minimumAllowedCPUCount()
            ..=VZVirtualMachineConfiguration::maximumAllowedCPUCount();
        if !cpu_limits.contains(&(spec.cpus as usize)) {
            return Err(invalid_spec(
                "cpus",
                format!("allowed CPU count is {cpu_limits:?}"),
            ));
        }
        let memory_limits = VZVirtualMachineConfiguration::minimumAllowedMemorySize()
            ..=VZVirtualMachineConfiguration::maximumAllowedMemorySize();
        if !memory_limits.contains(&spec.memory_bytes) {
            return Err(invalid_spec(
                "memory_bytes",
                format!("allowed memory range is {memory_limits:?}"),
            ));
        }

        if !VZVirtualMachine::isSupported() {
            return Err(backend("create", "Virtualization.framework is unavailable"));
        }
        let disk_root = disk("root_disk", &spec.root_disk)?;
        let disk_data = disk("data_disk", &spec.data_disk)?;

        let loader = VZLinuxBootLoader::initWithKernelURL(VZLinuxBootLoader::alloc(), &kernel);
        loader.setInitialRamdiskURL(initramfs.as_deref());
        loader.setCommandLine(&NSString::from_str(&spec.cmdline));

        let console = VZVirtioConsoleDeviceSerialPortConfiguration::new();
        let console_out = handle(prepared.console_out);
        let console_attachment =
            VZFileHandleSerialPortAttachment::initWithFileHandleForReading_fileHandleForWriting(
                VZFileHandleSerialPortAttachment::alloc(),
                None,
                Some(&console_out),
            );
        console.setAttachment(Some(&console_attachment));
        let network = VZVirtioNetworkDeviceConfiguration::new();
        network.setAttachment(Some(&VZNATNetworkDeviceAttachment::new()));

        let config = VZVirtualMachineConfiguration::new();
        config.setCPUCount(spec.cpus as usize);
        config.setMemorySize(spec.memory_bytes);
        config.setBootLoader(Some(&loader));
        config.setStorageDevices(&NSArray::from_retained_slice(&[
            disk_root.into_super(),
            disk_data.into_super(),
        ]));
        config.setSerialPorts(&NSArray::from_retained_slice(&[console.into_super()]));
        config.setSocketDevices(&NSArray::from_retained_slice(&[
            VZVirtioSocketDeviceConfiguration::new().into_super(),
        ]));
        config.setEntropyDevices(&NSArray::from_retained_slice(&[
            VZVirtioEntropyDeviceConfiguration::new().into_super(),
        ]));
        config.setNetworkDevices(&NSArray::from_retained_slice(&[network.into_super()]));
        config.validateWithError().map_err(|error| {
            invalid_spec("configuration", error.localizedDescription().to_string())
        })?;
        Ok(config)
    }
}

fn disk(
    field: &'static str,
    spec: &DiskSpec,
) -> Result<Retained<VZVirtioBlockDeviceConfiguration>, VmError> {
    let url = file_url(field, &spec.path)?;
    // SAFETY: The checked URL is a regular raw disk and configuration stays on the queue.
    unsafe {
        let attachment = VZDiskImageStorageDeviceAttachment::initWithURL_readOnly_error(
            VZDiskImageStorageDeviceAttachment::alloc(),
            &url,
            spec.read_only,
        )
        .map_err(|error| invalid_spec(field, error.localizedDescription().to_string()))?;
        Ok(VZVirtioBlockDeviceConfiguration::initWithAttachment(
            VZVirtioBlockDeviceConfiguration::alloc(),
            &attachment,
        ))
    }
}

fn file_url(field: &'static str, path: &Path) -> Result<Retained<NSURL>, VmError> {
    let text = path
        .to_str()
        .ok_or_else(|| invalid_spec(field, "expected a UTF-8 path"))?;
    Ok(NSURL::fileURLWithPath(&NSString::from_str(text)))
}

fn handle(file: File) -> Retained<NSFileHandle> {
    NSFileHandle::initWithFileDescriptor_closeOnDealloc(
        NSFileHandle::alloc(),
        file.into_raw_fd(),
        true,
    )
}
