//! Share the disk format and early-boot diagnostic between the guest and the host.

use std::str::FromStr;

/// Data-disk layout supported by the packaged guest.
pub const VERSION: u32 = 1;

const PREFIX: &str = "MARSA_BOOT_ERROR disk-schema ";

/// A valid data version that this guest cannot open.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("data disk schema {found} requires support newer than schema {supported}")]
pub struct TooNew {
    /// The schema recorded on disk.
    pub found: u32,
    /// The newest schema understood by the booted guest.
    pub supported: u32,
}

impl TooNew {
    /// Format the bounded serial diagnostic emitted by guest PID 1.
    pub fn console_line(self) -> String {
        let Self { found, supported } = self;
        format!("{PREFIX}{found} {supported}")
    }

    /// Recover a strictly formatted diagnostic from the current launch console.
    pub fn from_console_line(line: &str) -> Option<Self> {
        let (found, supported) = line.strip_prefix(PREFIX)?.split_once(' ')?;
        let parse = |text: &str| {
            if text.is_empty() || text.len() > 10 || !text.bytes().all(|byte| byte.is_ascii_digit())
            {
                return None;
            }
            u32::from_str(text).ok()
        };
        let found = parse(found)?;
        let supported = parse(supported)?;
        (supported > 0 && found > supported).then_some(Self { found, supported })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_exact_schema_diagnostics_and_no_extra_fields() {
        let error = TooNew {
            found: 2,
            supported: VERSION,
        };
        assert_eq!(
            TooNew::from_console_line(&error.console_line()),
            Some(error)
        );
        for line in [
            "unexpected",
            "MARSA_BOOT_ERROR disk-schema +2 1",
            "MARSA_BOOT_ERROR disk-schema 1 2",
            "MARSA_BOOT_ERROR disk-schema 2 1 trailing",
            "MARSA_BOOT_ERROR disk-schema 4294967296 1",
        ] {
            assert!(TooNew::from_console_line(line).is_none(), "{line}");
        }
    }
}
