//! Locate Docker's command and its image without inspecting the container's arguments.

use std::ffi::OsString;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Operation {
    Run,
    Pull,
    Build,
}

pub(super) fn global_end(args: &[OsString]) -> Option<usize> {
    let mut i = 0;
    while let Some(arg) = args.get(i) {
        let arg = arg.to_str()?;
        if !arg.starts_with('-') {
            return Some(i);
        }
        let name = arg.split('=').next()?;
        if matches!(
            name,
            "--context"
                | "--host"
                | "--config"
                | "--log-level"
                | "--tlscacert"
                | "--tlscert"
                | "--tlskey"
                | "-H"
                | "-c"
                | "-l"
        ) {
            i += usize::from(!arg.contains('='));
        } else if !(matches!(name, "-D" | "--debug" | "--tls" | "--tlsverify")
            || ["-H", "-c", "-l"]
                .iter()
                .any(|prefix| arg.starts_with(prefix) && arg.len() > 2))
        {
            return None;
        }
        i += 1;
    }
    None
}

pub(super) fn operation(args: &[OsString]) -> Option<(Operation, usize)> {
    let first = args.first()?.to_str()?;
    let second = args.get(1).and_then(|a| a.to_str());
    match (first, second) {
        ("run", _) => Some((Operation::Run, 1)),
        ("pull", _) => Some((Operation::Pull, 1)),
        ("build", _) => Some((Operation::Build, 1)),
        ("container", Some("run")) => Some((Operation::Run, 2)),
        ("image", Some("pull")) => Some((Operation::Pull, 2)),
        ("image" | "builder" | "buildx", Some("build")) | ("buildx", Some("b")) => {
            Some((Operation::Build, 2))
        }
        _ => None,
    }
}

// Value-taking long options in the pinned CLI and Buildx. Unknown options are left to Docker.
const RUN_VALUES: &str =
    "add-host annotation attach blkio-weight blkio-weight-device cap-add cap-drop cgroup-parent
cgroupns cidfile cpu-period cpu-quota cpu-rt-period cpu-rt-runtime cpu-shares cpus
cpuset-cpus cpuset-mems detach-keys device device-cgroup-rule device-read-bps
device-read-iops device-write-bps device-write-iops dns dns-option dns-search domainname
entrypoint env env-file expose gpus group-add health-cmd health-interval health-retries
health-start-interval health-start-period health-timeout hostname ip ip6 ipc isolation
label label-file link link-local-ip log-driver log-opt mac-address memory
memory-reservation memory-swap memory-swappiness mount name network network-alias
oom-score-adj pid pids-limit platform publish pull restart runtime security-opt shm-size
stop-signal stop-timeout storage-opt sysctl tmpfs ulimit user userns uts volume
volume-driver volumes-from workdir";

const BUILD_VALUES: &str =
    "add-host allow annotation attest build-arg build-context builder cache-from cache-to call
cgroup-parent file iidfile label metadata-file network no-cache-filter output platform
policy progress provenance resource sbom secret shm-size ssh tag target ulimit";

#[derive(Default)]
pub(super) struct Options<'a> {
    pub(super) operand: &'a str,
    pub(super) file: Option<&'a str>,
    pub(super) target: Option<&'a str>,
    pub(super) build_args: Vec<&'a str>,
    pub(super) pull: Option<&'a str>,
    pub(super) builder: Option<&'a str>,
}

impl<'a> Options<'a> {
    pub(super) fn parse(op: Operation, args: &'a [OsString]) -> Option<Self> {
        let mut result = Self::default();
        let mut iter = args.iter();
        let mut flags = true;
        while let Some(arg) = iter.next() {
            let arg = arg.to_str()?;
            if flags && arg == "--" {
                flags = false;
                continue;
            }
            if !flags || !arg.starts_with('-') || arg == "-" {
                if !result.operand.is_empty() {
                    return None;
                }
                result.operand = arg;
                if op == Operation::Run {
                    break;
                }
                continue;
            }
            let (name, inline) = arg
                .split_once('=')
                .map_or((arg, None), |(k, v)| (k, Some(v)));
            if matches!(
                name,
                "--platform"
                    | "--help"
                    | "--all-tags"
                    | "--build-context"
                    | "--call"
                    | "--check"
                    | "--invoke"
            ) {
                return None;
            }
            if name.starts_with("--") {
                let boolean = match op {
                    Operation::Run => matches!(
                        name,
                        "--detach"
                            | "--init"
                            | "--interactive"
                            | "--no-healthcheck"
                            | "--oom-kill-disable"
                            | "--privileged"
                            | "--publish-all"
                            | "--quiet"
                            | "--read-only"
                            | "--rm"
                            | "--sig-proxy"
                            | "--tty"
                            | "--use-api-socket"
                    ),
                    Operation::Pull => matches!(name, "--quiet" | "--disable-content-trust"),
                    Operation::Build => matches!(
                        name,
                        "--pull"
                            | "--push"
                            | "--load"
                            | "--no-cache"
                            | "--quiet"
                            | "--debug"
                            | "--rm"
                            | "--force-rm"
                            | "--compress"
                            | "--squash"
                            | "--disable-content-trust"
                    ),
                };
                if boolean {
                    if name == "--pull" && inline != Some("false") {
                        result.pull = Some("always");
                    }
                    continue;
                }
                let values = match op {
                    Operation::Run => RUN_VALUES,
                    Operation::Build => BUILD_VALUES,
                    Operation::Pull => return None,
                };
                if !values
                    .split_ascii_whitespace()
                    .any(|option| Some(option) == name.strip_prefix("--"))
                {
                    return None;
                }
                let value = inline.or_else(|| iter.next()?.to_str())?;
                match name {
                    "--file" => result.file = Some(value),
                    "--builder" => result.builder = Some(value),
                    "--target" => result.target = Some(value),
                    "--build-arg" => result.build_args.push(value),
                    "--pull" => result.pull = Some(value),
                    _ => {}
                }
            } else {
                // pflag allows boolean clusters and attached string values, e.g. -itv/path:/path.
                for (offset, c) in arg[1..].char_indices() {
                    if matches!(
                        (op, c),
                        (Operation::Run, 'd' | 'i' | 't' | 'P' | 'q')
                            | (Operation::Pull, 'q')
                            | (Operation::Build, 'D' | 'q')
                    ) {
                        continue;
                    }
                    if !matches!(
                        (op, c),
                        (
                            Operation::Run,
                            'a' | 'c' | 'e' | 'h' | 'l' | 'm' | 'p' | 'u' | 'v' | 'w'
                        ) | (Operation::Build, 'f' | 'o' | 't' | 'm' | 'c')
                    ) {
                        return None;
                    }
                    let attached = arg.get(offset + 2..)?.trim_start_matches('=');
                    let value = if attached.is_empty() {
                        iter.next()?.to_str()?
                    } else {
                        attached
                    };
                    if op == Operation::Build && c == 'f' {
                        result.file = Some(value);
                    }
                    break;
                }
            }
        }
        (!result.operand.is_empty()).then_some(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(words: &[&str]) -> Vec<OsString> {
        words.iter().map(OsString::from).collect()
    }

    #[test]
    fn image_options_end_before_container_command() {
        let input = args(&[
            "-itv/data:/data",
            "--env",
            "A=B",
            "repo",
            "--platform",
            "linux/arm64",
        ]);
        assert_eq!(
            Options::parse(Operation::Run, &input).unwrap().operand,
            "repo"
        );
        assert!(
            Options::parse(Operation::Run, &args(&["--platform=linux/arm64", "repo"])).is_none()
        );
        assert!(Options::parse(Operation::Run, &args(&["--unknown", "image", "secret"])).is_none());
        assert_eq!(
            Options::parse(Operation::Run, &args(&["-h", "my-host", "repo"]))
                .unwrap()
                .operand,
            "repo"
        );
        assert_eq!(
            global_end(&args(&["--config", "/dir", "-Hunix:///sock", "run"])),
            Some(3)
        );
    }
}
