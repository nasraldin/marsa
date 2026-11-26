//! Hand Docker arguments to the upstream binaries, selecting amd64 for Marsa-only workloads.

mod args;
mod dockerfile;

use args::{Operation, Options};
use serde_json::Value;
use std::{
    env,
    ffi::OsString,
    io::Read,
    os::unix::{fs::FileTypeExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, ExitCode, Stdio},
    thread,
    time::{Duration, Instant},
};

const PLATFORM: &str = "linux/amd64";
const ORIGINAL_CLI: &str = "DOCKER_CLI_PLUGIN_ORIGINAL_CLI_COMMAND";

struct Docker<'a> {
    upstream: PathBuf,
    global: &'a [OsString],
    fallback_host: Option<OsString>,
}

impl Docker<'_> {
    fn program(&self, path: &Path) -> Command {
        let mut command = Command::new(path);
        // Preserve the user's helper lookup order and also make the bundled Keychain helper visible.
        if let Some(directory) = self.upstream.parent()
            && let Some(path) = env::var_os("PATH")
            && let Ok(path) = env::join_paths(env::split_paths(&path).chain([directory.to_owned()]))
        {
            command.env("PATH", path);
        }
        if let Some(host) = &self.fallback_host {
            command.env("DOCKER_HOST", host);
        }
        command.args(self.global);
        command
    }

    fn stock(&self) -> Command {
        self.program(&self.upstream)
    }

    fn plugin(&self, name: &str) -> Command {
        let path = self
            .upstream
            .with_file_name("cli-plugins")
            .join(format!("docker-{name}"));
        let mut command = self.program(&path);
        command.arg(name).env(ORIGINAL_CLI, &self.upstream);
        command
    }

    fn context(&self, format: &str) -> Option<String> {
        probe(
            self.stock()
                .args(["context", "inspect", "--format", format]),
        )
    }

    fn default_host(&self) -> Option<OsString> {
        if [
            "DOCKER_CONTEXT",
            "DOCKER_HOST",
            "DOCKER_TLS_VERIFY",
            "DOCKER_CERT_PATH",
        ]
        .iter()
        .any(|key| env::var_os(key).is_some_and(|value| !value.is_empty()))
            || self.global.iter().filter_map(|v| v.to_str()).any(|v| {
                v.starts_with("-H")
                    || v.starts_with("-c")
                    || v == "--host"
                    || v.starts_with("--host=")
                    || v == "--context"
                    || v.starts_with("--context=")
                    || v.starts_with("--tls")
            })
        {
            return None;
        }
        let own = marsa_client::docker_socket_path().ok()?;
        if !own.metadata().ok()?.file_type().is_socket() {
            return None;
        }
        let context = self.context("{{.Name}} {{.Endpoints.docker.Host}}")?;
        fallback_host(context.trim(), &own)
    }

    fn on_marsa(&self) -> bool {
        let Some(own) = marsa_client::docker_socket_path()
            .ok()
            .and_then(|p| p.canonicalize().ok())
        else {
            return false;
        };
        self.context("{{.Endpoints.docker.Host}}")
            .and_then(|endpoint| {
                Path::new(endpoint.trim().strip_prefix("unix://")?)
                    .canonicalize()
                    .ok()
            })
            == Some(own)
    }

    fn local_arch(&self, image: &str) -> Option<String> {
        probe(self.stock().args([
            "image",
            "inspect",
            "--format",
            "{{.Os}}/{{.Architecture}}",
            "--",
            image,
        ]))
    }

    fn remote_amd64(&self, image: &str) -> bool {
        let Some(raw) =
            probe(
                self.plugin("buildx")
                    .args(["imagetools", "inspect", "--raw", "--", image]),
            )
        else {
            return false;
        };
        let Ok(manifest) = serde_json::from_str::<Value>(&raw) else {
            return false;
        };
        if manifest.get("manifests").is_some() {
            return amd64_only(&manifest);
        }
        // Single-image manifests put their platform in the referenced image config.
        if manifest.get("config").is_none() {
            return false;
        }
        probe(self.plugin("buildx").args([
            "imagetools",
            "inspect",
            "--format",
            "{{.Image.OS}}/{{.Image.Architecture}}",
            "--",
            image,
        ]))
        .is_some_and(|arch| arch.trim() == PLATFORM)
    }

    fn select(&self, op: Operation, args: &[OsString], plain_build: bool) -> bool {
        if env::var_os("DOCKER_DEFAULT_PLATFORM").is_some_and(|v| !v.is_empty()) {
            return false;
        }
        let Some(options) = Options::parse(op, args) else {
            return false;
        };
        let images = match op {
            Operation::Build => {
                if env::var_os("DOCKER_BUILDKIT").is_some_and(|v| v == "0" || v == "false") {
                    return false;
                }
                // Explicit builders can point anywhere, even when the Docker engine is local.
                if options.builder.is_some()
                    || env::var_os("BUILDX_BUILDER").is_some_and(|v| !v.is_empty())
                {
                    return false;
                }
                if !plain_build {
                    let Some(info) = probe(self.plugin("buildx").arg("inspect")) else {
                        return false;
                    };
                    let Some(context) = self.context("{{.Name}}") else {
                        return false;
                    };
                    if !info
                        .lines()
                        .any(|l| l.split_whitespace().eq(["Driver:", "docker"]))
                        || !info
                            .lines()
                            .any(|l| l.split_whitespace().eq(["Endpoint:", context.trim()]))
                    {
                        return false;
                    }
                }
                let Some(images) = dockerfile::bases(&options) else {
                    return false;
                };
                images
            }
            _ => vec![options.operand.to_owned()],
        };
        images
            .iter()
            .any(|image| self.image_requires_amd64(image, op, options.pull))
    }

    fn image_requires_amd64(&self, image: &str, op: Operation, pull: Option<&str>) -> bool {
        if image.is_empty()
            || image.starts_with('-')
            || !image
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.-/:@[]".contains(&b))
        {
            return false;
        }
        if op != Operation::Pull && pull != Some("always") {
            if let Some(local) = self.local_arch(image) {
                return local.trim() == PLATFORM;
            }
            if pull == Some("never") {
                return false;
            }
        }
        self.remote_amd64(image)
    }
}

fn fallback_host(context: &str, marsa: &Path) -> Option<OsString> {
    let default = context.strip_prefix("default unix://")?;
    if !matches!(Path::new(default).try_exists(), Ok(false)) {
        return None;
    }
    let mut endpoint = OsString::from("unix://");
    endpoint.push(marsa);
    Some(endpoint)
}

fn amd64_only(manifest: &Value) -> bool {
    let Some(entries) = manifest.get("manifests").and_then(Value::as_array) else {
        return false;
    };
    let mut amd64 = false;
    for entry in entries {
        let platform = &entry["platform"];
        if platform["os"] != "linux" {
            continue;
        }
        if platform["architecture"] == "arm64" {
            return false;
        }
        amd64 |= platform["architecture"] == "amd64";
    }
    amd64
}

// Metadata probes must not consume a byte of the user's input, disclose registry errors,
// or delay execution indefinitely when a registry or credential helper is unavailable.
fn probe(command: &mut Command) -> Option<String> {
    const LIMIT: usize = 1024 * 1024;
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let output = child.stdout.take()?;
    let reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        output
            .take((LIMIT + 1) as u64)
            .read_to_end(&mut bytes)
            .ok()?;
        (bytes.len() <= LIMIT)
            .then(|| String::from_utf8(bytes).ok())
            .flatten()
    });
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(15) {
        if let Some(status) = child.try_wait().ok()? {
            if status.success() && reader.is_finished() {
                return reader.join().ok().flatten();
            }
            if !status.success() {
                return None;
            }
        }
        thread::sleep(Duration::from_millis(10));
    }
    let _ = child.kill();
    let _ = child.wait();
    None
}

fn dispatch(upstream: PathBuf, argv: &[OsString]) -> Command {
    let Some(start) = args::global_end(argv) else {
        let mut stock = Command::new(upstream);
        stock.args(argv);
        return stock;
    };
    let (globals, tail) = argv.split_at(start);
    let mut docker = Docker {
        upstream,
        global: globals,
        fallback_host: None,
    };
    let first = tail[0].to_str().unwrap_or("");
    if first != "context" {
        docker.fallback_host = docker.default_host();
    }
    let plain = matches!(first, "build" | "builder" | "image")
        && args::operation(tail).is_some_and(|(op, _)| op == Operation::Build);
    let mut command = match first {
        "compose" | "buildx" => {
            let mut command = docker.plugin(first);
            command.args(&tail[1..]);
            command
        }
        "build" | "bake" | "builder" | "image"
            if (plain || first == "bake" || first == "builder")
                && (env::var_os("DOCKER_BUILDKIT").is_none_or(|v| {
                    matches!(v.to_str(), Some("" | "1" | "true" | "TRUE" | "True"))
                }) || first == "bake") =>
        {
            let mut command = docker.plugin("buildx");
            match first {
                "build" | "bake" => {
                    command.args(tail);
                }
                _ => {
                    command.args(&tail[1..]);
                }
            }
            // The upstream "docker build" alias defaults to the current engine's builder.
            if env::var_os("BUILDX_BUILDER").is_none()
                && !tail.iter().any(|s| {
                    s == "--builder" || s.to_str().is_some_and(|s| s.starts_with("--builder="))
                })
                && let Some(name) = docker.context("{{.Name}}")
            {
                command.env("BUILDX_BUILDER", name.trim());
            }
            command
        }
        _ => {
            let mut command = docker.stock();
            command.args(tail);
            command
        }
    };
    if let Some((op, offset)) = args::operation(tail)
        && docker.on_marsa()
        && docker.select(op, &tail[offset..], plain)
    {
        command.env("DOCKER_DEFAULT_PLATFORM", PLATFORM);
    }
    command
}

fn main() -> ExitCode {
    let argv: Vec<_> = env::args_os().skip(1).collect();
    let error = env::current_exe()
        .and_then(|p| p.canonicalize())
        .map(|p| dispatch(p.with_file_name("docker-upstream"), &argv).exec());
    eprintln!(
        "Marsa could not launch Docker. | تعذّر على مارسا تشغيل دوكر. {}",
        error.unwrap_or_else(|e| e)
    );
    ExitCode::from(126)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn falls_back_only_for_a_missing_default_unix_endpoint() {
        let own = Path::new("/marsa.sock");
        assert!(fallback_host("other unix:///missing.sock", own).is_none());
        assert!(fallback_host("default tcp://127.0.0.1:2376", own).is_none());
        assert!(fallback_host("default unix:///dev/null", own).is_none());
        assert_eq!(fallback_host("default unix:///dev/null/missing", own), None);
        let missing = env::temp_dir().join(format!("marsa-absent-socket-{}", std::process::id()));
        assert_eq!(
            fallback_host(&format!("default unix://{}", missing.display()), own),
            Some("unix:///marsa.sock".into())
        );
    }

    #[test]
    fn prefers_linux_arm64_and_ignores_attestations() {
        let entries = |arches: &[(&str, &str)]| serde_json::json!({"manifests": arches.iter().map(|(os, a)| serde_json::json!({"platform":{"os":os,"architecture":a}})).collect::<Vec<_>>()});
        assert!(amd64_only(&entries(&[
            ("unknown", "unknown"),
            ("linux", "amd64")
        ])));
        assert!(!amd64_only(&entries(&[
            ("linux", "amd64"),
            ("linux", "arm64")
        ])));
        assert!(!amd64_only(&entries(&[("windows", "amd64")])));
    }
}
