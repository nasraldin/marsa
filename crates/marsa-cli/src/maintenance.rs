//! Explicit local diagnostics and confirmed reset requests.

use std::io::{self, BufRead, Write};

use clap::ArgMatches;
use marsa_client::{
    ClientError, MarsaClient,
    api::{
        CollectDiagnosticsRequest, ConfirmFactoryReset, PrepareFactoryReset, PrepareSettingsReset,
        ResetRequest, ResetSettings, reset_request::Action, reset_response::Result as ResetResult,
    },
};
use serde_json::json;

use crate::{Failure, Outcome, command::Language, engine};

pub(super) async fn run(
    name: &str,
    args: &ArgMatches,
    language: Language,
    yes: bool,
) -> Option<Outcome> {
    let outcome = match name {
        "diagnose" => diagnose(language).await,
        "reset" => reset(args.get_flag("factory"), language, yes).await,
        _ => return None,
    };
    Some(outcome.unwrap_or_else(Outcome::Failure))
}

async fn diagnose(language: Language) -> Result<Outcome, Failure> {
    let client = engine::connect().await?;
    let path = client
        .diagnostics()
        .collect(CollectDiagnosticsRequest {
            request_id: engine::request_id()?,
            include_agent_audit: false,
        })
        .await
        .map_err(ClientError::from)
        .map_err(rpc_failure)?
        .into_inner()
        .bundle_path;
    if path.is_empty() {
        return Err(Failure::Operation);
    }
    let label = language.text("Diagnostics archive created at", "أُنشئ أرشيف التشخيص في");
    Ok(Outcome::Data {
        document: json!({"schema_version": 1, "kind": "diagnose", "bundle_path": path}),
        text: format!("{label} {path}\n"),
    })
}

async fn reset(factory: bool, language: Language, yes: bool) -> Result<Outcome, Failure> {
    if !yes {
        tokio::task::spawn_blocking(move || {
            confirm(factory, language, io::stdin().lock(), io::stderr().lock())
        })
        .await
        .map_err(|_| Failure::Operation)??;
    }
    let client = engine::connect().await?;
    let prepare = if factory {
        Action::PrepareFactory(PrepareFactoryReset {})
    } else {
        Action::PrepareSettings(PrepareSettingsReset {})
    };
    let response = reset_call(&client, prepare).await?;
    let confirmation_token = match (factory, response) {
        (true, ResetResult::Challenge(challenge)) => challenge.confirmation_token,
        (false, ResetResult::SettingsChallenge(challenge)) => challenge.confirmation_token,
        _ => return Err(Failure::Operation),
    };
    if confirmation_token.is_empty() {
        return Err(Failure::Operation);
    }
    let action = if factory {
        Action::ConfirmFactory(ConfirmFactoryReset { confirmation_token })
    } else {
        Action::Settings(ResetSettings { confirmation_token })
    };
    complete(&client, action).await?;
    let (scope, message) = if factory {
        (
            "factory",
            language.text("Factory reset completed.", "اكتملت إعادة ضبط المصنع."),
        )
    } else {
        (
            "settings",
            language.text("Marsa settings reset.", "أُعيد ضبط إعدادات مرسى."),
        )
    };
    Ok(Outcome::Data {
        document: json!({"schema_version": 1, "kind": "reset", "scope": scope}),
        text: format!("{message}\n"),
    })
}

async fn complete(client: &MarsaClient, action: Action) -> Result<(), Failure> {
    match reset_call(client, action).await? {
        ResetResult::Completed(_) => Ok(()),
        ResetResult::Challenge(_) | ResetResult::SettingsChallenge(_) => Err(Failure::Operation),
    }
}

async fn reset_call(client: &MarsaClient, action: Action) -> Result<ResetResult, Failure> {
    client
        .engine()
        .reset(ResetRequest {
            request_id: engine::request_id()?,
            action: Some(action),
        })
        .await
        .map_err(ClientError::from)
        .map_err(rpc_failure)?
        .into_inner()
        .result
        .ok_or(Failure::Operation)
}

fn rpc_failure(error: ClientError) -> Failure {
    if error.is_unimplemented() {
        Failure::Unavailable
    } else {
        Failure::Operation
    }
}

fn confirm(
    factory: bool,
    language: Language,
    input: impl BufRead,
    mut output: impl Write,
) -> Result<(), Failure> {
    let (phrase, prompt) = if factory {
        (
            "FACTORY RESET",
            language.text(
                "This erases Marsa settings and VM data. Type FACTORY RESET to continue.",
                "سيؤدي هذا إلى مسح إعدادات مرسى وبيانات الآلة الافتراضية. اكتب FACTORY RESET للمتابعة.",
            ),
        )
    } else {
        (
            "RESET SETTINGS",
            language.text(
                "This resets Marsa settings. Type RESET SETTINGS to continue.",
                "سيؤدي هذا إلى إعادة ضبط إعدادات مرسى. اكتب RESET SETTINGS للمتابعة.",
            ),
        )
    };
    writeln!(output, "{prompt}").map_err(|_| Failure::Operation)?;
    output.flush().map_err(|_| Failure::Operation)?;
    let mut line = String::new();
    input
        .take(64)
        .read_line(&mut line)
        .map_err(|_| Failure::ConfirmationDeclined)?;
    if line
        .strip_suffix("\r\n")
        .or_else(|| line.strip_suffix('\n'))
        == Some(phrase)
    {
        Ok(())
    } else {
        Err(Failure::ConfirmationDeclined)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factory_reset_requires_the_exact_complete_phrase() {
        for input in [
            &b""[..],
            &b"factory reset\n"[..],
            &b"RESET SETTINGS\n"[..],
            &b"FACTORY RESET"[..],
            &b"FACTORY RESET extra\n"[..],
        ] {
            assert!(matches!(
                confirm(true, Language::English, input, Vec::new()),
                Err(Failure::ConfirmationDeclined)
            ));
        }
        assert!(confirm(true, Language::English, &b"FACTORY RESET\n"[..], Vec::new()).is_ok());
    }

    #[test]
    fn settings_prompt_is_localized_and_accepts_its_own_phrase() {
        let mut prompt = Vec::new();
        assert!(
            confirm(
                false,
                Language::Arabic,
                &b"RESET SETTINGS\r\n"[..],
                &mut prompt
            )
            .is_ok()
        );
        let prompt = String::from_utf8(prompt).unwrap();
        assert!(prompt.contains("إعادة ضبط إعدادات مرسى"));
        assert!(prompt.contains("RESET SETTINGS"));
    }
}
