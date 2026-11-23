//! Shared settings, paths, and types for the Marsa workspace.

/// Constrained container creation records shared by the desktop and daemon.
pub mod container_config;
pub mod disk_schema;
pub mod docker_mounts;
pub mod error;
pub mod guest_boot;
pub mod guest_manifest;
pub mod paths;
pub mod platform;
pub mod settings;
pub mod update_manifest;
