//! The Apple silicon Virtualization.framework hypervisor.

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[allow(unsafe_code)]
mod platform;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub use platform::VzHypervisor;
