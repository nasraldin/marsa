//! Resolve deploy secrets after Compose has normalized the user-selected model.

use std::{
    collections::BTreeMap, future::Future, io, os::unix::fs::PermissionsExt as _, path::Path,
    process::Stdio, time::Duration,
};

use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    process::Command,
    time::timeout,
};

use super::{ComposeError, ComposeStream, relay::Relay};
use crate::docker::{DeploySecretStore, deploy_secrets::valid_name};

const PROVIDER: &str = "MARSA_KEYCHAIN";
const LIMIT: usize = 32;
const REVISION_PREFIX: &str = "io.marsa.deploy-secret.";

const MODEL_LIMIT: u64 = 1024 * 1024;
const PREPARE_LIMIT: Duration = Duration::from_secs(30);

pub(super) async fn start(
    binary: &Path,
    file: &Path,
    name: &str,
    socket: &str,
    relay: Option<Relay>,
) -> Result<ComposeStream, ComposeError> {
    launch(binary, file, name, socket, relay, |names| async move {
        let store = DeploySecretStore::for_canonical_project(file, name).await?;
        Ok(store.resolve_all(&names).await?)
    })
    .await
}

async fn launch<F>(
    binary: &Path,
    file: &Path,
    name: &str,
    socket: &str,
    relay: Option<Relay>,
    provider: impl FnOnce(Vec<String>) -> F,
) -> Result<ComposeStream, ComposeError>
where
    F: Future<Output = Result<Vec<(String, String)>, ComposeError>>,
{
    let directory = file.parent().ok_or(ComposeError::ProtectedModel)?;
    let sandbox = tempfile::Builder::new()
        .prefix("marsa-compose-keychain.")
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in("/tmp")
        .map_err(ComposeError::Io)?;
    let command = || {
        let mut command = Command::new(binary);
        command
            .args([
                "--ansi",
                "never",
                "--env-file",
                "/dev/null",
                "--project-name",
                name,
            ])
            .arg("--project-directory")
            .arg(directory)
            .current_dir(sandbox.path())
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", sandbox.path())
            .env("DOCKER_CONFIG", sandbox.path())
            .env("DOCKER_HOST", format!("unix://{socket}"))
            .env("OTEL_SDK_DISABLED", "true")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .kill_on_drop(true);
        command
    };
    let model = timeout(PREPARE_LIMIT, async {
        let mut process = command()
            .arg("--file")
            .arg(file)
            .args(["config", "--format", "json"])
            .stdout(Stdio::piped())
            .spawn()
            .map_err(ComposeError::Io)?;
        let mut output = Vec::new();
        process
            .stdout
            .take()
            .ok_or(ComposeError::Output)?
            .take(MODEL_LIMIT + 1)
            .read_to_end(&mut output)
            .await
            .map_err(ComposeError::Io)?;
        if output.len() as u64 > MODEL_LIMIT
            || !process.wait().await.map_err(ComposeError::Io)?.success()
        {
            return Err(ComposeError::ProtectedModel);
        }
        serde_json::from_slice(&output).map_err(|_| ComposeError::ProtectedModel)
    })
    .await
    .map_err(|_| ComposeError::ProtectedModel)??;
    let mut plan = Plan::from_json(model)?;
    let names: Vec<_> = plan.consumers.keys().cloned().collect();
    let secrets = provider(names.clone()).await?;
    if names.len() != secrets.len() {
        return Err(ComposeError::ProtectedModel);
    }

    // Only the verified Compose process sees these environment values. The
    // frozen model cannot name the fresh keys outside its runtime secret map.
    // Host providers, build, registry helpers and further dotenv loads are off.
    let mut deploy = command();
    let normalized = plan.model.to_string();
    for (name, (revision, secret)) in names.iter().zip(secrets) {
        let variable = format!(
            "MARSA_PRIVATE_{}",
            crate::request_id()
                .map_err(ComposeError::Io)?
                .replace('-', "")
        );
        if normalized.contains(&variable) {
            return Err(ComposeError::ProtectedModel);
        }
        plan.bind(name, &variable, &revision)?;
        deploy.env(variable, secret);
    }
    let model = serde_json::to_vec(&plan.model).map_err(|_| ComposeError::ProtectedModel)?;
    let mut process = deploy
        .args([
            "--file",
            "-",
            "up",
            "--detach",
            "--no-build",
            "--pull",
            "never",
        ])
        .stdin(Stdio::piped())
        .spawn()
        .map_err(ComposeError::Io)?;
    let mut input = process.stdin.take().ok_or(ComposeError::Output)?;
    timeout(PREPARE_LIMIT, input.write_all(&model))
        .await
        .map_err(|_| ComposeError::Io(io::ErrorKind::TimedOut.into()))?
        .map_err(ComposeError::Io)?;
    drop(input);

    // Engine error strings are untrusted and could reflect a copied archive.
    // Keep the protected operation's output limited to its exit status.
    Ok(ComposeStream {
        child: process,
        _relay: relay,
        _private_directory: Some(sandbox),
        stdout: None,
        stderr: None,
        out_buffer: Vec::new(),
        err_buffer: Vec::new(),
        done: false,
    })
}

struct Plan {
    model: Value,
    consumers: BTreeMap<String, Vec<String>>,
}

impl Plan {
    fn from_json(mut model: Value) -> Result<Self, ComposeError> {
        let fields = model.as_object().ok_or(ComposeError::ProtectedModel)?;
        if fields.keys().any(|key| {
            !matches!(
                key.as_str(),
                "name" | "services" | "secrets" | "configs" | "volumes" | "networks"
            )
        }) {
            return Err(ComposeError::ProtectedModel);
        }
        let definitions = model
            .get("secrets")
            .and_then(Value::as_object)
            .ok_or(ComposeError::ProtectedModel)?;
        let mut consumers = BTreeMap::new();
        for (name, definition) in definitions {
            let Some(source) = definition.get("environment") else {
                continue;
            };
            if source.as_str() != Some(PROVIDER)
                || !valid_name(name)
                || ["file", "content", "driver", "template_driver"]
                    .iter()
                    .any(|key| definition.get(*key).is_some())
                || definition.get("external").and_then(Value::as_bool) == Some(true)
                || consumers.len() >= LIMIT
            {
                return Err(ComposeError::ProtectedModel);
            }
            consumers.insert(name.clone(), Vec::new());
        }
        let services = model
            .get_mut("services")
            .and_then(Value::as_object_mut)
            .ok_or(ComposeError::ProtectedModel)?;
        for (service, config) in services {
            let fields = config.as_object_mut().ok_or(ComposeError::ProtectedModel)?;
            if ["provider", "extends", "env_file", "label_file"]
                .iter()
                .any(|key| fields.contains_key(*key))
            {
                return Err(ComposeError::ProtectedModel);
            }
            // The canonical model already selected the profiles for the approved project.
            fields.remove("profiles");
            let Some(secrets) = fields.get("secrets").and_then(Value::as_array) else {
                continue;
            };
            for secret in secrets {
                let source = secret
                    .get("source")
                    .and_then(Value::as_str)
                    .ok_or(ComposeError::ProtectedModel)?;
                let Some(entries) = consumers.get_mut(source) else {
                    continue;
                };
                let expected = format!("/run/secrets/{source}");
                if secret.get("target").and_then(Value::as_str) != Some(expected.as_str())
                    || fields.get("read_only").and_then(Value::as_bool) == Some(true)
                    || fields.contains_key("volumes_from")
                    || fields.contains_key("tmpfs")
                    || fields
                        .get("volumes")
                        .and_then(Value::as_array)
                        .is_some_and(|mounts| {
                            mounts.iter().any(|mount| {
                                mount.get("type").and_then(Value::as_str) != Some("volume")
                                    || mount
                                        .get("target")
                                        .and_then(Value::as_str)
                                        .is_none_or(overlaps_secrets)
                            })
                        })
                    || entries.len() >= LIMIT
                {
                    return Err(ComposeError::ProtectedModel);
                }
                entries.push(service.clone());
            }
        }
        if consumers.is_empty() || consumers.values().any(Vec::is_empty) {
            return Err(ComposeError::ProtectedModel);
        }
        Ok(Self { model, consumers })
    }

    fn bind(&mut self, name: &str, variable: &str, revision: &str) -> Result<(), ComposeError> {
        let consumers = self
            .consumers
            .get(name)
            .ok_or(ComposeError::ProtectedModel)?;
        let definition = self.model["secrets"][name]
            .as_object_mut()
            .ok_or(ComposeError::ProtectedModel)?;
        definition.insert("environment".to_owned(), json!(variable));
        for service in consumers {
            let config = self.model["services"][service]
                .as_object_mut()
                .ok_or(ComposeError::ProtectedModel)?;
            let labels = config
                .entry("labels")
                .or_insert_with(|| json!({}))
                .as_object_mut()
                .ok_or(ComposeError::ProtectedModel)?;
            labels.insert(format!("{REVISION_PREFIX}{name}"), json!(revision));
        }
        Ok(())
    }
}

fn overlaps_secrets(path: &str) -> bool {
    use std::path::{Component, Path};

    let path = Path::new(path);
    let secrets = Path::new("/run/secrets");
    !path.is_absolute()
        || path
            .components()
            .any(|component| component == Component::ParentDir)
        || path.starts_with(secrets)
        || secrets.starts_with(path)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn model() -> serde_json::Value {
        json!({
            "name": "demo",
            "secrets": {"database-password": {"environment": "MARSA_KEYCHAIN"}},
            "services": {"app": {
                "image": "app:latest",
                "command": ["echo", "$$literal"],
                "secrets": [{"source": "database-password", "target": "/run/secrets/database-password"}]
            }}
        })
    }

    #[test]
    fn binds_only_the_normalized_secret_source_and_the_granted_service() {
        let mut plan = Plan::from_json(model()).unwrap();
        plan.bind("database-password", "MARSA_PRIVATE_A01", "revision-1")
            .unwrap();
        assert_eq!(plan.model["services"]["app"]["command"][1], "$$literal");
        assert_eq!(
            plan.model["secrets"]["database-password"]["environment"],
            "MARSA_PRIVATE_A01"
        );
        assert!(plan.model["services"]["app"]["environment"].is_null());
        assert!(!plan.model.to_string().contains("secret contents"));
    }

    #[test]
    fn rejects_host_launcher_and_secret_path_overrides() {
        let mut config = model();
        config["services"]["app"]["provider"] = json!({"type": "unknown"});
        assert!(Plan::from_json(config).is_err());
        let mut config = model();
        config["services"]["app"]["secrets"][0]["target"] = json!("/other/path");
        assert!(Plan::from_json(config).is_err());
        let mut config = model();
        config["services"]["app"]["volumes"] =
            json!([{"type": "bind", "source": "/tmp", "target": "/run"}]);
        assert!(Plan::from_json(config).is_err());
        for field in ["include", "jobs", "models", "x-embedded"] {
            let mut config = model();
            config[field] = json!({});
            assert!(Plan::from_json(config).is_err(), "{field}");
        }
    }
    #[tokio::test]
    #[ignore = "requires the pinned Compose and Docker CLI and a disposable Unix-socket engine project"]
    async fn copies_the_secret_only_into_the_granted_container() {
        use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
        use sha2::{Digest, Sha256};
        use std::{env, fs, path::PathBuf};

        let compose =
            PathBuf::from(env::var_os("MARSA_TEST_COMPOSE").expect("set MARSA_TEST_COMPOSE"));
        let docker =
            PathBuf::from(env::var_os("MARSA_TEST_DOCKER").expect("set MARSA_TEST_DOCKER"));
        let socket = env::var("MARSA_TEST_DOCKER_SOCKET").expect("set MARSA_TEST_DOCKER_SOCKET");
        let image = env::var("MARSA_TEST_SECRET_IMAGE").expect("set MARSA_TEST_SECRET_IMAGE");
        let project = format!(
            "marsasecret{}",
            crate::request_id().unwrap().replace('-', "")
        );
        let granted = format!("{project}-granted");
        let ordinary = format!("{project}-ordinary");
        let scratch = tempfile::tempdir().unwrap();
        let file = scratch.path().join("compose.yaml");
        let spec = json!({
            "name": project,
            "services": {
                "granted": {
                    "image": image,
                    "container_name": granted,
                    "command": ["sh", "-c", "echo ready; sleep 60"],
                    "environment": {"INTERPOLATION_CANARY": "${MARSA_KEYCHAIN:-unset}"},
                    "secrets": ["credential"],
                },
                "ordinary": {"image": image, "container_name": ordinary, "command": ["sleep", "60"]}
            },
            "secrets": {"credential": {"environment": "MARSA_KEYCHAIN"}}
        });
        let source = serde_json::to_vec(&spec).unwrap();
        fs::write(&file, &source).unwrap();
        let mut token = [0; 32];
        getrandom::fill(&mut token).unwrap();
        let marker = URL_SAFE_NO_PAD.encode(token);
        let secret = format!("{marker} $value{}", char::from(10));
        let expected = Sha256::digest(secret.as_bytes());
        let copies = secret.clone();
        let mut stream = launch(
            &compose,
            &file,
            &project,
            &socket,
            None,
            |names| async move {
                assert_eq!(names, ["credential"]);
                Ok(vec![("fixture-revision".to_owned(), copies)])
            },
        )
        .await
        .unwrap();

        struct Cleanup<'a> {
            docker: &'a Path,
            socket: &'a str,
            granted: &'a str,
            ordinary: &'a str,
            network: String,
        }
        impl Drop for Cleanup<'_> {
            fn drop(&mut self) {
                let base = || {
                    let mut command = std::process::Command::new(self.docker);
                    command.args(["--host", &format!("unix://{}", self.socket)]);
                    command
                };
                let _ = base()
                    .args(["rm", "-f", self.granted, self.ordinary])
                    .output();
                let _ = base().args(["network", "rm", &self.network]).output();
            }
        }
        let _cleanup = Cleanup {
            docker: &docker,
            socket: &socket,
            granted: &granted,
            ordinary: &ordinary,
            network: format!("{project}_default"),
        };
        assert!(
            timeout(Duration::from_secs(30), stream.next())
                .await
                .unwrap()
                .unwrap()
                .is_none()
        );
        let invoke = |args: Vec<String>| {
            std::process::Command::new(&docker)
                .args(["--host", &format!("unix://{socket}")])
                .args(args)
                .output()
                .unwrap()
        };
        for args in [
            vec!["inspect".into(), granted.clone(), ordinary.clone()],
            vec!["logs".into(), granted.clone()],
        ] {
            let output = invoke(args);
            assert!(output.status.success());
            for bytes in [&output.stdout, &output.stderr] {
                assert!(
                    !bytes
                        .windows(marker.len())
                        .any(|value| value == marker.as_bytes())
                );
            }
        }
        let configured = invoke(vec![
            "inspect".into(),
            "--format".into(),
            "{{json .Config.Env}}".into(),
            granted.clone(),
        ]);
        assert!(configured.status.success());
        let configured: Vec<String> = serde_json::from_slice(&configured.stdout).unwrap();
        assert!(
            configured
                .iter()
                .any(|entry| entry == "INTERPOLATION_CANARY=unset")
        );
        assert!(
            !configured
                .iter()
                .any(|entry| entry.starts_with("MARSA_PRIVATE_"))
        );
        let read = invoke(vec![
            "exec".into(),
            granted.clone(),
            "cat".into(),
            "/run/secrets/credential".into(),
        ]);
        assert!(read.status.success());
        assert!(Sha256::digest(read.stdout) == expected);
        let absent = invoke(vec![
            "exec".into(),
            ordinary.clone(),
            "test".into(),
            "!".into(),
            "-e".into(),
            "/run/secrets/credential".into(),
        ]);
        assert!(absent.status.success());
        let identity = |name: &str| {
            let output = invoke(vec![
                "inspect".into(),
                "--format".into(),
                "{{.Id}}".into(),
                name.into(),
            ]);
            assert!(output.status.success());
            output.stdout
        };
        let original = (identity(&granted), identity(&ordinary));
        let mut rotated = launch(
            &compose,
            &file,
            &project,
            &socket,
            None,
            |names| async move {
                assert_eq!(names, ["credential"]);
                Ok(vec![("fixture-revision-2".to_owned(), secret)])
            },
        )
        .await
        .unwrap();
        assert!(
            timeout(Duration::from_secs(30), rotated.next())
                .await
                .unwrap()
                .unwrap()
                .is_none()
        );
        assert_ne!(identity(&granted), original.0);
        assert_eq!(identity(&ordinary), original.1);
        assert_eq!(fs::read(file).unwrap(), source);
    }
}
