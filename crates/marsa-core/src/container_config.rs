//! Bounded, portable values for desktop-created Docker containers.

use std::{collections::HashSet, fmt, path::Path};

use serde::{Deserialize, Serialize};

/// Maximum configuration sets per settings document.
pub const MAX_CONFIG_SETS: usize = 32;

/// Maximum rows in each environment, label and port section.
pub const MAX_CONFIG_ROWS: usize = 32;

/// Maximum mount rows in a container configuration.
pub const MAX_CONFIG_MOUNTS: usize = 8;

/// A reusable name and description for container creation options.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ContainerConfigSet {
    /// Display name, unique within the owner's settings.
    pub name: String,
    /// Optional description displayed in the settings list.
    #[serde(default)]
    pub description: String,
    /// Docker settings copied into each create draft that uses this set.
    #[serde(default)]
    pub options: ContainerCreateOptions,
}

impl ContainerConfigSet {
    /// Check the metadata and every container option.
    pub fn validate(&self) -> Result<(), ContainerConfigError> {
        if !display_text(&self.name, 80)
            || self.name.trim().is_empty()
            || self.name != self.name.trim()
            || !display_text(&self.description, 512)
        {
            return Err(ContainerConfigError::Details);
        }
        self.options.validate()
    }
}

/// The constrained Docker options accepted by the desktop creation form.
#[derive(Debug, Default, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ContainerCreateOptions {
    /// Engine default, bridge, or an isolated network namespace.
    pub network: ConfigNetwork,
    /// Container restart policy applied by Docker.
    pub restart: ConfigRestartPolicy,
    /// Explicit environment assignments.
    pub env: Vec<ConfigKeyValue>,
    /// Explicit Docker labels, outside reserved namespaces.
    pub labels: Vec<ConfigKeyValue>,
    /// Published ports bound to loopback on the selected engine.
    pub ports: Vec<ConfigPort>,
    /// Named volumes and host bind mounts.
    pub mounts: Vec<ConfigMount>,
}

impl ContainerCreateOptions {
    /// Validate options before they can be persisted or sent to an engine.
    pub fn validate(&self) -> Result<(), ContainerConfigError> {
        if self.env.len() > MAX_CONFIG_ROWS
            || self.labels.len() > MAX_CONFIG_ROWS
            || self.ports.len() > MAX_CONFIG_ROWS
            || self.mounts.len() > MAX_CONFIG_MOUNTS
        {
            return Err(ContainerConfigError::Capacity);
        }
        validate_pairs(&self.env, env_key, ContainerConfigError::Environment)?;
        validate_pairs(&self.labels, label_key, ContainerConfigError::Labels)?;

        let mut published = HashSet::new();
        if (self.network == ConfigNetwork::Disabled && !self.ports.is_empty())
            || self.ports.iter().any(|port| {
                port.host == 0
                    || port.container == 0
                    || !published.insert((port.host, port.protocol))
            })
        {
            return Err(ContainerConfigError::Ports);
        }

        let mut targets = HashSet::new();
        if self.mounts.iter().any(|mount| {
            !container_path(&mount.target)
                || !targets.insert(&mount.target)
                || match mount.kind {
                    ConfigMountKind::Host => !host_path(&mount.source),
                    ConfigMountKind::Volume => !volume_name(&mount.source),
                }
        }) {
            return Err(ContainerConfigError::Mounts);
        }
        Ok(())
    }

    /// Whether a create request would mount anything from the engine host.
    pub fn has_host_mounts(&self) -> bool {
        self.mounts
            .iter()
            .any(|mount| mount.kind == ConfigMountKind::Host)
    }
}

/// Supported container network settings. Shared host namespaces are outside this contract.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigNetwork {
    /// Leave this choice to Docker.
    #[default]
    Default,
    /// Attach Docker's standard bridge.
    Bridge,
    /// Isolate the container from networking.
    Disabled,
}

/// Supported Docker restart modes.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigRestartPolicy {
    /// Keep the engine's default behavior.
    #[default]
    Never,
    /// Retry when the container exits unsuccessfully.
    OnFailure,
    /// Automatically restart whenever Docker allows.
    Always,
    /// Restart except after an explicit owner stop.
    UnlessStopped,
}

/// One environment assignment or Docker label.
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigKeyValue {
    /// Environment variable name or label key.
    pub key: String,
    /// Explicit value, including an empty value.
    pub value: String,
}

impl fmt::Debug for ConfigKeyValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConfigKeyValue")
            .field("key", &self.key)
            .finish_non_exhaustive()
    }
}

/// The Docker transport exposed by a published port.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigPortProtocol {
    /// Stream transport.
    #[default]
    Tcp,
    /// Datagram transport.
    Udp,
}

/// Map an engine loopback port to a single container port.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigPort {
    /// Loopback port on the selected engine.
    pub host: u16,
    /// Destination port inside the container.
    pub container: u16,
    /// Docker port protocol.
    pub protocol: ConfigPortProtocol,
}

/// Identifies how Docker resolves a mount source.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigMountKind {
    /// A Docker-managed named volume.
    #[default]
    Volume,
    /// An explicitly reviewed path on the engine host.
    Host,
}

/// Mount a Docker volume or explicit engine host path at a container path.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigMount {
    /// How Docker resolves the mount source.
    pub kind: ConfigMountKind,
    /// Volume name or absolute host path.
    pub source: String,
    /// Absolute path inside the container.
    pub target: String,
    /// Whether Docker may write to the mounted source.
    pub read_only: bool,
}

/// Location of invalid reusable or ad hoc container configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ContainerConfigError {
    /// The set name or description is empty, oversized or malformed.
    #[error("invalid container configuration name or description")]
    Details,
    /// A section contains too many entries.
    #[error("too many container configuration entries")]
    Capacity,
    /// Invalid or repeated environment assignments.
    #[error("invalid or duplicate environment assignment")]
    Environment,
    /// Invalid, reserved or repeated Docker labels.
    #[error("invalid, reserved or duplicate Docker label")]
    Labels,
    /// Invalid or repeated publications, or a publication without networking.
    #[error("invalid or duplicate published port")]
    Ports,
    /// Invalid source or destination, a sensitive source, or repeated destinations.
    #[error("invalid, protected or duplicate container mount")]
    Mounts,
}

fn validate_pairs(
    pairs: &[ConfigKeyValue],
    valid_name: fn(&str) -> bool,
    error: ContainerConfigError,
) -> Result<(), ContainerConfigError> {
    let mut keys = HashSet::new();
    if pairs.iter().any(|pair| {
        !valid_name(&pair.key)
            || pair.value.len() > 4096
            || pair.value.contains(['\0', '\r', '\n'])
            || !keys.insert(&pair.key)
    }) {
        return Err(error);
    }
    Ok(())
}

fn env_key(key: &str) -> bool {
    let mut bytes = key.bytes();
    key.len() <= 128
        && bytes
            .next()
            .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn label_key(key: &str) -> bool {
    volume_name(key)
        && ![
            "com.docker.",
            "io.docker.",
            "org.dockerproject.",
            "dev.marsa.",
        ]
        .into_iter()
        .any(|prefix| key.starts_with(prefix))
}

fn volume_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    name.len() <= 128
        && bytes
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
}

fn display_text(value: &str, limit: usize) -> bool {
    value.len() <= limit
        && !value.chars().any(|ch| {
            ch.is_control() || matches!(ch, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
}

fn container_path(path: &str) -> bool {
    display_text(path, 512)
        && path
            .strip_prefix('/')
            .is_some_and(|tail| tail.split('/').all(|part| !matches!(part, "" | "." | "..")))
}

fn host_path(path: &str) -> bool {
    if !container_path(path) {
        return false;
    }
    let name = path.to_ascii_lowercase();
    !name.ends_with(".sock")
        && ![
            "/dev",
            "/etc",
            "/proc",
            "/sys",
            "/run",
            "/var/run",
            "/var/lib/docker",
            "/var/lib/containerd",
            "/var/lib/containers",
            "/private/var/run",
            "/root",
            "/system",
            "/library/keychains",
            "/.vol",
        ]
        .into_iter()
        .any(|root| Path::new(&name).starts_with(root))
        && !name
            .split('/')
            .any(|part| matches!(part, ".ssh" | ".marsa"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_configurations_cannot_set_reserved_labels_or_mount_engine_storage() {
        let mut options = ContainerCreateOptions {
            labels: vec![ConfigKeyValue {
                key: "com.docker.compose.project".into(),
                value: "example".into(),
            }],
            ..Default::default()
        };
        assert_eq!(options.validate(), Err(ContainerConfigError::Labels));
        options.labels.clear();
        for source in [
            "/var/run/docker.sock",
            "/var/lib/docker/volumes",
            "/Users/example/.marsa",
            "/Users/example/.MARSA",
            "/Users/example/../example/data",
            "/Users/example/control.SOCK",
        ] {
            options.mounts = vec![ConfigMount {
                kind: ConfigMountKind::Host,
                source: source.into(),
                target: "/data".into(),
                read_only: true,
            }];
            assert_eq!(
                options.validate(),
                Err(ContainerConfigError::Mounts),
                "{source}"
            );
        }
        options.mounts[0].source = "/Users/example/project/data".into();
        assert!(options.validate().is_ok());
    }

    #[test]
    fn publishes_only_unique_engine_ports_with_networking() {
        let tcp = ConfigPort {
            host: 8080,
            container: 80,
            protocol: ConfigPortProtocol::Tcp,
        };
        let mut udp = tcp.clone();
        udp.protocol = ConfigPortProtocol::Udp;
        let mut options = ContainerCreateOptions {
            ports: vec![tcp.clone(), udp],
            ..Default::default()
        };
        assert!(options.validate().is_ok());
        options.ports.push(tcp);
        assert_eq!(options.validate(), Err(ContainerConfigError::Ports));
        options.ports.pop();
        options.network = ConfigNetwork::Disabled;
        assert_eq!(options.validate(), Err(ContainerConfigError::Ports));
    }
}
