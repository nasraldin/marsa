//! Resolve only the registry credential needed for an explicit image push.

use std::{collections::BTreeMap, io, path::PathBuf, process::Stdio, time::Duration};

use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE},
};
use hyper::header::HeaderValue;
use nix::unistd::geteuid;
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    process::Command,
    time::timeout,
};

use super::DockerError;

pub(super) const DOCKER_HUB: &str = "https://index.docker.io/v1/";
const CONFIG_LIMIT: usize = 1024 * 1024;
const CREDENTIAL_LIMIT: usize = 8 * 1024;
const CREDENTIAL_DEADLINE: Duration = Duration::from_secs(15);

#[derive(Default, Deserialize)]
struct DockerConfig {
    #[serde(default)]
    auths: BTreeMap<String, LocalAuth>,
    #[serde(default, rename = "credsStore")]
    credentials_store: String,
    #[serde(default, rename = "credHelpers")]
    helpers: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct LocalAuth {
    #[serde(default)]
    auth: String,
    #[serde(default)]
    username: String,
    #[serde(default)]
    password: String,
    #[serde(default)]
    identitytoken: String,
    #[serde(default)]
    registrytoken: String,
}

#[derive(Default, Serialize)]
struct RegistryAuth<'a> {
    #[serde(skip_serializing_if = "String::is_empty")]
    username: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    password: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    identitytoken: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    registrytoken: String,
    #[serde(skip_serializing_if = "str::is_empty")]
    serveraddress: &'a str,
}

impl LocalAuth {
    fn for_registry(mut self, registry: &str) -> Result<RegistryAuth<'_>, DockerError> {
        if !self.auth.is_empty() {
            let decoded = STANDARD
                .decode(&self.auth)
                .or_else(|_| STANDARD_NO_PAD.decode(&self.auth))
                .map_err(|_| DockerError::RegistryAuthentication)?;
            let decoded =
                String::from_utf8(decoded).map_err(|_| DockerError::RegistryAuthentication)?;
            let (username, password) = decoded
                .split_once(':')
                .filter(|(username, _)| !username.is_empty())
                .ok_or(DockerError::RegistryAuthentication)?;
            self.username = username.to_owned();
            self.password = password.to_owned();
        }
        Ok(RegistryAuth {
            username: self.username,
            password: self.password,
            identitytoken: self.identitytoken,
            registrytoken: self.registrytoken,
            serveraddress: registry,
        })
    }
}

pub(super) async fn registry_header(registry: &str) -> Result<HeaderValue, DockerError> {
    if geteuid().is_root() {
        return Err(crate::ClientError::Root.into());
    }
    let auth = timeout(CREDENTIAL_DEADLINE, resolve(registry))
        .await
        .map_err(|_| DockerError::RegistryAuthentication)??;
    auth_header(&auth)
}

fn auth_header(auth: &RegistryAuth<'_>) -> Result<HeaderValue, DockerError> {
    let json = serde_json::to_vec(auth).map_err(|_| DockerError::RegistryAuthentication)?;
    let token = URL_SAFE.encode(json);
    if token.len() > CREDENTIAL_LIMIT {
        return Err(DockerError::RegistryAuthentication);
    }
    let mut value =
        HeaderValue::from_str(&token).map_err(|_| DockerError::RegistryAuthentication)?;
    value.set_sensitive(true);
    Ok(value)
}

async fn resolve(registry: &str) -> Result<RegistryAuth<'_>, DockerError> {
    let path =
        if let Some(config) = std::env::var_os("DOCKER_CONFIG").filter(|path| !path.is_empty()) {
            PathBuf::from(config)
        } else {
            std::env::home_dir()
                .ok_or(DockerError::RegistryAuthentication)?
                .join(".docker")
        };
    if !path.is_absolute() {
        return Err(DockerError::RegistryAuthentication);
    }

    let mut config = load_config(path.join("config.json")).await?;
    let helper = config
        .helpers
        .get(registry)
        .filter(|name| !name.is_empty())
        .map_or(config.credentials_store.as_str(), String::as_str);
    if !helper.is_empty() {
        // A Docker config can name arbitrary programs. Only the packaged helper
        // with a verified app-bundle signature can read credentials for Marsa.
        if helper != "osxkeychain" {
            return Err(DockerError::RegistryAuthentication);
        }
        let path = crate::platform::bundled_registry_helper()
            .await
            .map_err(|_| DockerError::RegistryAuthentication)?;
        return read_keychain(path, registry).await;
    }

    if let Some(auth) = config.auths.remove(registry) {
        return auth.for_registry(registry);
    }
    if config.auths.is_empty() && config.helpers.is_empty() {
        // The Docker CLI defaults to the native credential store when the config
        // contains no credentials. Development builds may have no bundled helper.
        if let Ok(path) = crate::platform::bundled_registry_helper().await {
            return read_keychain(path, registry).await;
        }
    }
    Ok(RegistryAuth::default())
}

async fn load_config(path: PathBuf) -> Result<DockerConfig, DockerError> {
    let data = match crate::platform::read_user_docker_file(&path, CONFIG_LIMIT, false).await {
        Ok(data) => data,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(DockerConfig::default()),
        Err(_) => return Err(DockerError::RegistryAuthentication),
    };
    serde_json::from_slice(&data).map_err(|_| DockerError::RegistryAuthentication)
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Credentials {
    #[serde(rename = "ServerURL")]
    server_url: String,
    username: String,
    secret: String,
}

async fn read_keychain(path: PathBuf, registry: &str) -> Result<RegistryAuth<'_>, DockerError> {
    let failure = |_| DockerError::RegistryAuthentication;
    let mut helper = Command::new(path)
        .arg("get")
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(failure)?;
    let mut stdin = helper
        .stdin
        .take()
        .ok_or(DockerError::RegistryAuthentication)?;
    stdin
        .write_all(registry.as_bytes())
        .await
        .map_err(failure)?;
    drop(stdin);

    let mut data = Vec::new();
    helper
        .stdout
        .take()
        .ok_or(DockerError::RegistryAuthentication)?
        .take(CREDENTIAL_LIMIT as u64 + 1)
        .read_to_end(&mut data)
        .await
        .map_err(failure)?;
    if data.len() > CREDENTIAL_LIMIT {
        return Err(DockerError::RegistryAuthentication);
    }
    if !helper.wait().await.map_err(failure)?.success() {
        if data.trim_ascii() == b"credentials not found in native keychain" {
            return Ok(RegistryAuth::default());
        }
        return Err(DockerError::RegistryAuthentication);
    }

    let credentials: Credentials =
        serde_json::from_slice(&data).map_err(|_| DockerError::RegistryAuthentication)?;
    if credentials.server_url != registry || credentials.username.is_empty() {
        return Err(DockerError::RegistryAuthentication);
    }
    let mut result = RegistryAuth {
        serveraddress: registry,
        ..RegistryAuth::default()
    };
    if credentials.username == "<token>" {
        result.identitytoken = credentials.secret;
    } else {
        result.username = credentials.username;
        result.password = credentials.secret;
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_auth_preserves_password_colons_and_only_targets_the_selected_registry() {
        let auth: LocalAuth = serde_json::from_str(
            r#"{"auth":"dXNlcjpwYXNzOndvcmQ=","serveraddress":"untrusted.example"}"#,
        )
        .unwrap();
        let header = auth_header(&auth.for_registry("registry.example").unwrap()).unwrap();
        let decoded = URL_SAFE.decode(header.as_bytes()).unwrap();
        let actual: serde_json::Value = serde_json::from_slice(&decoded).unwrap();
        assert_eq!(
            actual,
            serde_json::json!({
                "username": "user",
                "password": "pass:word",
                "serveraddress": "registry.example",
            })
        );
        assert!(header.is_sensitive());
    }
}
