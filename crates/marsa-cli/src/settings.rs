//! Read daemon settings and submit one revision-checked TOML value change.

use clap::ArgMatches;
use marsa_client::{
    DaemonEndpoint, MarsaClient,
    api::{GetSettingsRequest, UpdateSettingsRequest},
};
use serde_json::json;
use toml::{Table, Value};

use crate::{
    Failure, Outcome,
    command::Language,
    engine::{connection_failure, request_id},
};

pub(super) async fn run(args: &ArgMatches, language: Language) -> Outcome {
    apply(args, language).await.unwrap_or_else(Outcome::Failure)
}

async fn apply(args: &ArgMatches, language: Language) -> Result<Outcome, Failure> {
    let (command, args) = args.subcommand().ok_or(Failure::Usage)?;
    let name = args.try_get_one::<String>("key").ok().flatten();
    let path = name.map(|key| setting_path(key)).transpose()?;
    let client = DaemonEndpoint::discover()
        .map_err(connection_failure)?
        .connect()
        .await
        .map_err(connection_failure)?;
    let base = client
        .settings()
        .get(GetSettingsRequest {})
        .await
        .map_err(|_| Failure::Operation)?
        .into_inner();
    let table: Table = base.settings_toml.parse().map_err(|_| Failure::Operation)?;
    match (command, name, path) {
        ("list", _, _) => Ok(Outcome::Data {
            document: json!({
                "schema_version": 1,
                "kind": "settings",
                "revision": base.revision.to_string(),
                "settings": table,
                "restart_required": base.restart_required,
            }),
            text: base.settings_toml,
        }),
        ("get", Some(name), Some(path)) => {
            let value = find(&table, &path).ok_or(Failure::Usage)?;
            Ok(value_result(
                name,
                value,
                base.revision,
                base.restart_required,
                value.to_string() + "\n",
            ))
        }
        ("set", Some(name), Some(path)) => {
            let input = args.get_one::<String>("value").ok_or(Failure::Usage)?;
            let value = parse_value(input);
            save(&client, table, base.revision, name, &path, value, language).await
        }
        _ => Err(Failure::Usage),
    }
}

fn setting_path(name: &str) -> Result<Vec<&str>, Failure> {
    let parts: Vec<_> = name.split('.').collect();
    if name.len() > 128
        || parts.iter().any(|part| {
            part.is_empty()
                || !part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        })
    {
        return Err(Failure::Usage);
    }
    Ok(parts)
}

fn find<'a>(table: &'a Table, path: &[&str]) -> Option<&'a Value> {
    let (key, parents) = path.split_last()?;
    let parent = parents
        .iter()
        .try_fold(table, |table, name| table.get(*name)?.as_table())?;
    parent.get(*key)
}

fn parse_value(input: &str) -> Value {
    let mut document = format!("value = {input}").parse::<Table>().ok();
    if let Some(table) = &mut document
        && table.len() == 1
        && let Some(value) = table.remove("value")
    {
        return value;
    }
    Value::String(input.to_owned())
}

async fn save(
    client: &MarsaClient,
    mut table: Table,
    revision: u64,
    name: &str,
    path: &[&str],
    value: Value,
    language: Language,
) -> Result<Outcome, Failure> {
    let (key, parents) = path.split_last().ok_or(Failure::Usage)?;
    let parent = parents
        .iter()
        .try_fold(&mut table, |table, name| {
            table.get_mut(*name)?.as_table_mut()
        })
        .ok_or(Failure::Usage)?;
    parent.insert((*key).to_owned(), value.clone());
    let text = toml::to_string_pretty(&table).map_err(|_| Failure::Operation)?;
    let reply = client
        .settings()
        .update(UpdateSettingsRequest {
            request_id: request_id()?,
            expected_revision: revision,
            settings_toml: text,
        })
        .await;
    let result = match reply {
        Ok(reply) => reply.into_inner(),
        Err(error) => return Ok(rejection(language, error.message())),
    };
    let message = saved(language, result.restart_required);
    Ok(value_result(
        name,
        &value,
        result.revision,
        result.restart_required,
        format!("{message}\n"),
    ))
}

fn value_result(name: &str, value: &Value, revision: u64, restart: bool, text: String) -> Outcome {
    Outcome::Data {
        text,
        document: json!({
            "schema_version": 1,
            "kind": "setting",
            "key": name,
            "value": value,
            "revision": revision.to_string(),
            "restart_required": restart,
        }),
    }
}

pub(super) fn saved(language: Language, restart: bool) -> &'static str {
    if restart {
        language.text(
            "Saved. Restart the VM to apply the change.",
            "تم الحفظ. أعد تشغيل الآلة الافتراضية لتطبيق التغيير.",
        )
    } else {
        language.text("Saved.", "تم الحفظ.")
    }
}

pub(super) fn rejection(language: Language, detail: &str) -> Outcome {
    let message = language.text(
        "The Marsa daemon rejected the request.",
        "رفض خادم مرسى الطلب.",
    );
    // Leave controller output on one terminal line and bound the diagnostic.
    let detail: String = detail
        .chars()
        .take(512)
        .flat_map(char::escape_debug)
        .collect();
    Outcome::Rejected(format!("{message} {detail}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_setting_value_cannot_assign_a_sibling_key() {
        assert_eq!(parse_value("8"), Value::Integer(8));
        assert_eq!(parse_value("dark"), Value::String("dark".into()));
        assert_eq!(
            parse_value("4\nother=5"),
            Value::String("4\nother=5".into())
        );
    }
}
