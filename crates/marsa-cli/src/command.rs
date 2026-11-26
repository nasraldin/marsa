//! Define the command tree once for argument parsing, help and completion scripts.

use clap::{Arg, ArgAction, Command, builder::PossibleValuesParser};
#[cfg(unix)]
use marsa_client::l10n::{Arguments, Catalog, MessageKey};
use marsa_client::l10n::{Locale, TextKey, keys};

#[cfg(unix)]
thread_local! {
    static ENGLISH_CATALOG: Option<Catalog> = Catalog::new(Locale::EnglishUs).ok().map(Catalog::with_explicit_isolates);
}

pub(super) fn label(key: TextKey) -> &'static str {
    key.text(Locale::EnglishUs)
}

#[cfg(unix)]
pub(super) fn message(key: MessageKey, values: &[(&str, &str)]) -> String {
    let mut args = Arguments::with_capacity(values.len());
    for (name, value) in values {
        args.set(*name, *value);
    }
    ENGLISH_CATALOG.with(|catalog| {
        catalog
            .as_ref()
            .and_then(|catalog| catalog.format(key, Some(&args)).ok())
            .unwrap_or_else(|| label(keys::CLI_OPERATION_FAILED).to_owned())
    })
}

#[cfg(unix)]
pub(super) fn technical(value: &str) -> String {
    value.escape_debug().to_string()
}

struct Spec {
    name: &'static str,
    about: TextKey,
    arguments: &'static [Argument],
    commands: &'static [Spec],
}

const fn leaf(name: &'static str, about: TextKey) -> Spec {
    command(name, about, &[], &[])
}

const fn command(
    name: &'static str,
    about: TextKey,
    arguments: &'static [Argument],
    commands: &'static [Spec],
) -> Spec {
    Spec {
        name,
        about,
        arguments,
        commands,
    }
}

struct Argument {
    id: &'static str,
    about: TextKey,
    form: Form,
}

enum Form {
    Flag,
    Option {
        label: &'static str,
    },
    Positional {
        label: &'static str,
        required: bool,
        values: &'static [&'static str],
    },
}

const fn flag(id: &'static str, about: TextKey) -> Argument {
    Argument {
        id,
        about,
        form: Form::Flag,
    }
}

const fn required_option(id: &'static str, label: &'static str, about: TextKey) -> Argument {
    Argument {
        id,
        about,
        form: Form::Option { label },
    }
}

const fn value(
    id: &'static str,
    label: &'static str,
    required: bool,
    values: &'static [&'static str],
    about: TextKey,
) -> Argument {
    Argument {
        id,
        about,
        form: Form::Positional {
            label,
            required,
            values,
        },
    }
}

const JSON: Argument = flag("json", keys::CLI_HELP_PRINT_A_JSON_RESPONSE);
const YES: Argument = flag("yes", keys::CLI_HELP_CONFIRM_DESTRUCTIVE_ACTIONS);
const HELP: TextKey = keys::CLI_HELP_PRINT_HELP;
const KEY: Argument = value("key", "KEY", true, &[], keys::CLI_HELP_SETTINGS_KEY);
const REQUEST: Argument = value(
    "request",
    "REQUEST",
    true,
    &[],
    keys::CLI_HELP_APPROVAL_REQUEST_ID,
);

static ROOT: Spec = command(
    "marsa",
    keys::CLI_HELP_MANAGE_MARSA,
    &[],
    &[
        leaf("status", keys::CLI_HELP_SHOW_THE_ENGINE_AND_VM_STATUS),
        leaf("start", keys::CLI_HELP_START_THE_ENGINE_VM),
        leaf("stop", keys::CLI_HELP_STOP_THE_ENGINE_VM),
        leaf("restart", keys::CLI_HELP_RESTART_THE_ENGINE_VM),
        leaf("env", keys::CLI_HELP_PRINT_THE_DOCKER_HOST_EXPORT),
        command(
            "config",
            keys::CLI_HELP_MANAGE_SETTINGS,
            &[],
            &[
                command("get", keys::CLI_HELP_READ_A_SETTING, &[KEY], &[]),
                command(
                    "set",
                    keys::CLI_HELP_SET_A_SETTING,
                    &[
                        KEY,
                        value(
                            "value",
                            "VALUE",
                            true,
                            &[],
                            keys::CLI_HELP_NEW_SETTING_VALUE,
                        ),
                    ],
                    &[],
                ),
                leaf("list", keys::CLI_HELP_LIST_SETTINGS),
            ],
        ),
        command(
            "share",
            keys::CLI_HELP_MANAGE_SHARED_FOLDERS,
            &[],
            &[
                leaf("list", keys::CLI_HELP_LIST_SHARED_FOLDERS),
                leaf("approvals", keys::CLI_HELP_LIST_PENDING_BIND_APPROVALS),
                command(
                    "resolve",
                    keys::CLI_HELP_ANSWER_A_PENDING_BIND_REQUEST,
                    &[
                        value("id", "ID", true, &[], keys::CLI_HELP_APPROVAL_IDENTIFIER),
                        value(
                            "decision",
                            "DECISION",
                            true,
                            &["once", "always", "deny"],
                            keys::CLI_HELP_SHARE_ONCE_ALWAYS_OR_DENY,
                        ),
                    ],
                    &[],
                ),
                command(
                    "add",
                    keys::CLI_HELP_ALLOW_A_FOLDER_TO_BE_SHARED,
                    &[value(
                        "path",
                        "PATH",
                        true,
                        &[],
                        keys::CLI_HELP_FOLDER_TO_SHARE,
                    )],
                    &[],
                ),
                command(
                    "remove",
                    keys::CLI_HELP_REMOVE_A_SHARED_FOLDER_FROM_THE_ALLOWLIST,
                    &[value(
                        "share",
                        "SHARE",
                        true,
                        &[],
                        keys::CLI_HELP_SHARED_FOLDER_OR_ID,
                    )],
                    &[],
                ),
            ],
        ),
        command(
            "disk",
            keys::CLI_HELP_MANAGE_VM_DISK_SPACE,
            &[],
            &[
                leaf("usage", keys::CLI_HELP_SHOW_DISK_USAGE),
                command(
                    "cleanup",
                    keys::CLI_HELP_PREVIEW_UNUSED_GUEST_STORAGE_WITH_DRY_RUN,
                    &[
                        flag("dry-run", keys::CLI_HELP_SHOW_A_READ_ONLY_PREVIEW),
                        flag(
                            "include-volumes",
                            keys::CLI_HELP_INCLUDE_UNUSED_LOCAL_VOLUMES,
                        ),
                    ],
                    &[],
                ),
                leaf(
                    "reclaim",
                    keys::CLI_HELP_RETURN_FREE_SPACE_FROM_A_RUNNING_VM_TO_MACOS,
                ),
            ],
        ),
        command(
            "network",
            keys::CLI_HELP_MANAGE_NETWORKING,
            &[],
            &[
                leaf("status", keys::CLI_HELP_SHOW_NETWORK_STATUS),
                leaf("apply", keys::CLI_HELP_APPLY_NETWORK_SETTINGS),
                command(
                    "mode",
                    keys::CLI_HELP_SHOW_OR_SAVE_THE_NETWORK_MODE_FOR_THE_NEXT_START,
                    &[value(
                        "mode",
                        "MODE",
                        false,
                        &["auto", "nat", "userspace"],
                        keys::CLI_HELP_NETWORK_MODE,
                    )],
                    &[],
                ),
            ],
        ),
        command(
            "logs",
            keys::CLI_HELP_SHOW_DAEMON_AND_GUEST_LOGS,
            &[flag("follow", keys::CLI_HELP_FOLLOW_NEW_LOG_ENTRIES)],
            &[],
        ),
        leaf("diagnose", keys::CLI_HELP_COLLECT_A_DIAGNOSTICS_BUNDLE),
        command(
            "update",
            keys::CLI_HELP_MANAGE_APP_UPDATES,
            &[],
            &[
                leaf("check", keys::CLI_HELP_CHECK_FOR_UPDATES),
                leaf("apply", keys::CLI_HELP_APPLY_AN_UPDATE),
            ],
        ),
        command(
            "mcp",
            keys::CLI_HELP_RUN_THE_MCP_SERVER_ON_STDIO,
            &[
                required_option("client-id", "ID", keys::CLI_HELP_REGISTERED_CLIENT_ID),
                required_option("project", "PATH", keys::CLI_HELP_ABSOLUTE_PROJECT_FOLDER),
            ],
            &[],
        ),
        command(
            "agents",
            keys::CLI_HELP_MANAGE_AGENT_PERMISSIONS,
            &[],
            &[
                leaf("list", keys::CLI_HELP_LIST_AGENT_PERMISSIONS),
                command(
                    "approve",
                    keys::CLI_HELP_APPROVE_AN_AGENT_REQUEST,
                    &[REQUEST],
                    &[],
                ),
                command(
                    "deny",
                    keys::CLI_HELP_DENY_AN_AGENT_REQUEST,
                    &[REQUEST],
                    &[],
                ),
                command(
                    "revoke",
                    keys::CLI_HELP_REVOKE_AN_AGENT_GRANT,
                    &[value(
                        "grant",
                        "GRANT",
                        true,
                        &[],
                        keys::CLI_HELP_AGENT_GRANT_ID,
                    )],
                    &[],
                ),
            ],
        ),
        command(
            "reset",
            keys::CLI_HELP_RESET_MARSA_SETTINGS_AND_OPTIONALLY_ERASE_VM_DATA,
            &[flag(
                "factory",
                keys::CLI_HELP_ERASE_ALL_MARSA_SETTINGS_AND_VM_DATA,
            )],
            &[],
        ),
        leaf("version", keys::CLI_HELP_SHOW_COMPONENT_VERSIONS),
        command(
            "completion",
            keys::CLI_HELP_GENERATE_SHELL_COMPLETIONS,
            &[value(
                "shell",
                "SHELL",
                true,
                &["bash", "zsh", "fish"],
                keys::CLI_HELP_TARGET_SHELL,
            )],
            &[],
        ),
    ],
);

pub(super) fn build() -> Command {
    ROOT.build("marsa")
        .subcommand_required(false)
        .arg(JSON.clap().global(true))
        .arg(YES.clap().global(true))
}

impl Spec {
    fn build(&self, path: &str) -> Command {
        Command::new(self.name)
            .about(label(self.about))
            .disable_help_subcommand(true)
            .disable_help_flag(true)
            .arg(
                Arg::new("help")
                    .short('h')
                    .long("help")
                    .action(ArgAction::Help)
                    .help(label(HELP)),
            )
            .args(self.arguments.iter().map(|arg| arg.clap()))
            .subcommands(
                self.commands
                    .iter()
                    .map(|child| child.build(&format!("{path} {}", child.name))),
            )
            .override_help(self.help(path))
            .subcommand_required(!self.commands.is_empty())
    }

    fn help(&self, path: &str) -> String {
        let usage = label(keys::CLI_HELP_USAGE);
        let options = label(keys::CLI_HELP_OPTIONS);
        let commands = label(keys::CLI_HELP_COMMANDS);
        let arguments = label(keys::CLI_HELP_ARGUMENTS);
        let options_label = label(keys::CLI_HELP_OPTIONS_PLACEHOLDER);
        let command_label = label(keys::CLI_HELP_COMMAND_PLACEHOLDER);
        let about = label(self.about);
        let mut help = format!("{about}\n\n{usage}\n  {path} [{options_label}]");
        if !self.commands.is_empty() {
            help.push_str(&format!(" <{command_label}>"));
        }
        for arg in self.arguments {
            if let Form::Option { label } = arg.form {
                help.push_str(&format!(" --{} <{label}>", arg.id));
            }
            if let Form::Positional {
                label, required, ..
            } = arg.form
            {
                let display = if required {
                    format!("<{label}>")
                } else {
                    format!("[{label}]")
                };
                help.push_str(&format!(" {display}"));
            }
        }
        help.push('\n');
        if !self.commands.is_empty() {
            help.push_str(&format!("\n{commands}\n"));
            for command in self.commands {
                let about = label(command.about);
                help.push_str(&format!("  {:<14} {about}\n", command.name));
            }
        }
        if self
            .arguments
            .iter()
            .any(|arg| matches!(arg.form, Form::Positional { .. }))
        {
            help.push_str(&format!("\n{arguments}\n"));
            for arg in self.arguments {
                if let Form::Positional {
                    label: argument_label,
                    values,
                    ..
                } = arg.form
                {
                    let about = label(arg.about);
                    help.push_str(&format!("  {argument_label:<14} {about}"));
                    if !values.is_empty() {
                        help.push_str(&format!(" ({})", values.join(", ")));
                    }
                    help.push('\n');
                }
            }
        }
        help.push_str(&format!("\n{options}\n  -h, --help     {}\n", label(HELP)));
        for arg in [JSON, YES].iter().chain(self.arguments) {
            let option = match arg.form {
                Form::Flag => format!("--{}", arg.id),
                Form::Option { label } => format!("--{} <{label}>", arg.id),
                Form::Positional { .. } => continue,
            };
            let about = label(arg.about);
            help.push_str(&format!("  {option:<18} {about}\n"));
        }
        help
    }
}

impl Argument {
    fn clap(&self) -> Arg {
        let arg = Arg::new(self.id).help(label(self.about));
        match self.form {
            Form::Flag => arg.long(self.id).action(ArgAction::SetTrue),
            Form::Option { label } => arg.long(self.id).value_name(label).required(true),
            Form::Positional {
                label,
                required,
                values,
            } => {
                let arg = arg.value_name(label).required(required);
                if values.is_empty() {
                    arg
                } else {
                    arg.value_parser(PossibleValuesParser::new(values.iter().copied()))
                }
            }
        }
    }
}

#[cfg(unix)]
pub(super) fn escape_terminal_controls(json: &str) -> String {
    // Preserve line boundaries in structured output while escaping invisible
    // controls that can reorder or obscure text in the terminal.
    let mut escaped = String::with_capacity(json.len());
    for ch in json.chars() {
        match ch {
            '\u{0000}'..='\u{0009}'
            | '\u{000b}'..='\u{001f}'
            | '\u{007f}'..='\u{009f}'
            | '\u{061c}'
            | '\u{200b}'..='\u{200f}'
            | '\u{2028}'..='\u{202e}'
            | '\u{2060}'..='\u{206f}'
            | '\u{feff}' => {
                use std::fmt::Write as _;
                let _ = write!(escaped, "\\u{:04x}", u32::from(ch));
            }
            _ => escaped.push(ch),
        }
    }
    escaped
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn technical_values_preserve_unicode_and_escape_input_controls() {
        let value = "/tmp/حاوية.web [1]\u{202e}\u{2069}\n\x1b";
        let expected = r"/tmp/حاوية.web [1]\u{202e}\u{2069}\n\u{1b}";
        assert_eq!(technical(value), expected);
    }
}
