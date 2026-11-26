//! Read and interpret SystemConfiguration's full proxy dictionary on macOS.

use super::*;

#[tonic::async_trait]
impl Discovery for Platform {
    async fn discover(&self, draft: &ProxySettings) -> Result<BootProxy, Status> {
        #[cfg(target_os = "macos")]
        {
            let bytes = crate::platform_command::output(
                "/usr/sbin/scutil",
                &["-d", "--proxy"],
                std::time::Duration::from_secs(2),
                64 * 1024,
            )
            .await
            .ok_or_else(|| {
                Status::unavailable(
                    "could not read macOS proxy settings | تعذرت قراءة إعدادات وكيل macOS",
                )
            })?;
            resolve_snapshot(draft, &bytes)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = draft;
            Err(unsupported())
        }
    }
}

#[cfg(any(target_os = "macos", test))]
mod macos;
#[cfg(target_os = "macos")]
use macos::resolve_snapshot;
