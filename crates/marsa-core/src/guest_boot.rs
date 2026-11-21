//! Parse only Marsa-owned parameters from the guest kernel command line.

use std::{collections::HashSet, io, path::PathBuf};

const MAX_CMDLINE: usize = 4096;
const MAX_SHARES: usize = 16;

#[derive(Debug, PartialEq, Eq)]
/// An allowlisted virtiofs tag and its destination in the guest.
pub struct Share {
    /// Tag configured on the VM device.
    pub tag: String,
    /// Absolute guest mount point.
    pub path: PathBuf,
}

#[derive(Debug, PartialEq, Eq)]
/// Guest boot options shared by PID 1 and the guest agent.
pub struct Settings {
    /// Guest hostname.
    pub hostname: String,
    /// DHCP interface, or none when the guest boots offline.
    pub interface: Option<String>,
    /// Preconfigured virtiofs mountpoints.
    pub shares: Vec<Share>,
}

impl Settings {
    /// Validate and decode Marsa-owned kernel arguments.
    pub fn parse(cmdline: &str) -> io::Result<Self> {
        if cmdline.len() > MAX_CMDLINE {
            return invalid("kernel command line exceeds the Marsa limit");
        }
        let mut hostname = None;
        let mut interface = None;
        let mut shares = Vec::new();
        let mut share_tags = HashSet::new();

        for token in cmdline.split_ascii_whitespace() {
            let Some(parameter) = token.strip_prefix("marsa.") else {
                continue;
            };
            let (name, value) = parameter
                .split_once('=')
                .ok_or_else(|| bad("missing Marsa parameter value"))?;
            match name {
                "hostname" if hostname.is_none() && identifier(value, 63) => {
                    hostname = Some(value.to_owned());
                }
                "interface" if interface.is_none() && identifier(value, 15) => {
                    interface = Some(value.to_owned());
                }
                "share" if shares.len() < MAX_SHARES => {
                    let share = parse_share(value)?;
                    if !share_tags.insert(share.tag.clone())
                        || shares.iter().any(|previous: &Share| {
                            previous.path.starts_with(&share.path)
                                || share.path.starts_with(&previous.path)
                        })
                    {
                        return invalid("duplicate or overlapping share");
                    }
                    shares.push(share);
                }
                _ => return invalid("invalid, duplicate or unknown Marsa kernel parameter"),
            }
        }

        Ok(Self {
            hostname: hostname.unwrap_or_else(|| "marsa".into()),
            interface: match interface.as_deref() {
                Some("none") => None,
                Some(_) => interface,
                None => Some("eth0".into()),
            },
            shares,
        })
    }
}

fn parse_share(value: &str) -> io::Result<Share> {
    let (tag, encoded) = value
        .split_once(':')
        .ok_or_else(|| bad("share must contain a tag and destination"))?;
    if !is_share_tag(tag) {
        return invalid("invalid virtiofs tag");
    }
    let decoded = decode_path(encoded)?;
    let parts: Vec<_> = decoded.split('/').collect();
    if parts.len() < 3
        || !parts[0].is_empty()
        || parts[1..]
            .iter()
            .any(|part| matches!(*part, "" | "." | ".."))
        || !["Users", "Volumes", "private", "tmp", "mnt"].contains(&parts[1])
        || decoded.chars().any(char::is_control)
    {
        return invalid("invalid guest share destination");
    }
    Ok(Share {
        tag: tag.to_owned(),
        path: decoded.into(),
    })
}

fn decode_path(encoded: &str) -> io::Result<String> {
    let mut bytes = encoded.bytes();
    let mut decoded = Vec::with_capacity(encoded.len());
    while let Some(byte) = bytes.next() {
        if byte != b'%' {
            decoded.push(byte);
            continue;
        }
        let mut hex = [0; 2];
        for digit in &mut hex {
            *digit = bytes
                .next()
                .and_then(|next| char::from(next).to_digit(16))
                .ok_or_else(|| bad("invalid percent encoding in share destination"))?
                as u8;
        }
        decoded.push(hex[0] * 16 + hex[1]);
    }
    String::from_utf8(decoded).map_err(|_| bad("share destination must be UTF-8"))
}

/// Return whether a virtiofs tag follows Marsa's guest boot identifier format.
pub fn is_share_tag(tag: &str) -> bool {
    identifier(tag, 36)
}

fn identifier(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

fn bad(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn invalid<T>(message: &'static str) -> io::Result<T> {
    Err(bad(message))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_defaults_and_ignores_linux_parameters() {
        assert_eq!(
            Settings::parse("console=hvc0 root=/dev/vda ro").unwrap(),
            Settings {
                hostname: "marsa".into(),
                interface: Some("eth0".into()),
                shares: vec![]
            }
        );
    }

    #[test]
    fn decodes_share_path_and_honors_offline_mode() {
        let settings = Settings::parse(
            "marsa.hostname=guest-1 marsa.interface=none marsa.share=workspace:/Users/dev/My%20App",
        )
        .unwrap();
        assert_eq!(settings.hostname, "guest-1");
        assert_eq!(settings.interface, None);
        assert_eq!(
            settings.shares[0].path,
            std::path::Path::new("/Users/dev/My App")
        );
    }

    #[test]
    fn rejects_unsafe_destinations_and_overlaps() {
        for input in [
            "marsa.share=proj:/etc/ssh",
            "marsa.share=proj:/Users/dev/%2e%2e",
            "marsa.share=proj:/Users/dev/%00",
            "marsa.share=proj:/Users/dev/a marsa.share=next:/Users/dev/a/b",
            "marsa.hostname=one marsa.hostname=two",
            "marsa.interface=-ef",
        ] {
            assert!(Settings::parse(input).is_err(), "{input}");
        }
    }
}
