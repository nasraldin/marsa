//! Define the command tree once for argument parsing, help and completion scripts.

use clap::{Arg, ArgAction, Command, builder::PossibleValuesParser};
use marsa_client::l10n::Locale;

#[derive(Clone, Copy)]
pub(super) enum Language {
    English,
    Arabic,
}

impl Language {
    pub(super) fn current() -> Self {
        let locale = ["LC_ALL", "LC_MESSAGES", "LANG"]
            .into_iter()
            .filter_map(|name| std::env::var(name).ok())
            .find(|value| !value.is_empty());
        if locale
            .as_ref()
            .is_some_and(|locale| locale.split(['_', '-', '.', '@']).next() == Some("ar"))
        {
            Self::Arabic
        } else {
            Self::English
        }
    }

    pub(super) fn locale(self) -> Locale {
        match self {
            Self::English => Locale::EnglishUs,
            Self::Arabic => Locale::Arabic,
        }
    }

    pub(super) fn text(self, english: &'static str, arabic: &'static str) -> &'static str {
        match self {
            Self::English => english,
            Self::Arabic => arabic,
        }
    }
}

type Text = (&'static str, &'static str);

struct Spec {
    name: &'static str,
    about: Text,
    arguments: &'static [Argument],
    commands: &'static [Spec],
}

const fn leaf(name: &'static str, about: Text) -> Spec {
    command(name, about, &[], &[])
}

const fn command(
    name: &'static str,
    about: Text,
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
    about: Text,
    form: Form,
}

enum Form {
    Flag,
    Positional {
        label: &'static str,
        required: bool,
        values: &'static [&'static str],
    },
}

const fn flag(id: &'static str, about: Text) -> Argument {
    Argument {
        id,
        about,
        form: Form::Flag,
    }
}

const fn value(
    id: &'static str,
    label: &'static str,
    required: bool,
    values: &'static [&'static str],
    about: Text,
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

const JSON: Argument = flag(
    "json",
    ("Print a JSON response", "اطبع الاستجابة بصيغة JSON"),
);
const YES: Argument = flag(
    "yes",
    (
        "Confirm destructive actions",
        "أكّد الإجراءات التي تحذف البيانات",
    ),
);
const HELP: Text = ("Print help", "اعرض المساعدة");
const KEY: Argument = value("key", "KEY", true, &[], ("Settings key", "مفتاح الإعداد"));
const REQUEST: Argument = value(
    "request",
    "REQUEST",
    true,
    &[],
    ("Approval request ID", "معرّف طلب الموافقة"),
);

static ROOT: Spec = command(
    "marsa",
    ("Manage Marsa", "أدر تطبيق مرسى"),
    &[],
    &[
        leaf(
            "status",
            (
                "Show the engine and VM status",
                "اعرض حالة المحرك والآلة الافتراضية",
            ),
        ),
        leaf(
            "start",
            ("Start the engine VM", "شغّل الآلة الافتراضية للمحرك"),
        ),
        leaf("stop", ("Stop the engine VM", "أوقف الآلة الافتراضية للمحرك")),
        leaf(
            "restart",
            ("Restart the engine VM", "أعد تشغيل الآلة الافتراضية للمحرك"),
        ),
        leaf(
            "env",
            ("Print the DOCKER_HOST export", "اطبع أمر تعيين DOCKER_HOST"),
        ),
        command(
            "config",
            ("Manage settings", "أدر الإعدادات"),
            &[],
            &[
                command("get", ("Read a setting", "اقرأ إعدادًا"), &[KEY], &[]),
                command(
                    "set",
                    ("Set a setting", "غيّر إعدادًا"),
                    &[
                        KEY,
                        value(
                            "value",
                            "VALUE",
                            true,
                            &[],
                            ("New setting value", "القيمة الجديدة للإعداد"),
                        ),
                    ],
                    &[],
                ),
                leaf("list", ("List settings", "اسرد الإعدادات")),
            ],
        ),
        command(
            "share",
            ("Manage shared folders", "أدر المجلدات المشتركة"),
            &[],
            &[
                leaf("list", ("List shared folders", "اسرد المجلدات المشتركة")),
                leaf(
                    "approvals",
                    (
                        "List pending bind approvals",
                        "اعرض طلبات وصل المجلدات المعلّقة",
                    ),
                ),
                command(
                    "resolve",
                    ("Answer a pending bind request", "أجب عن طلب وصل معلّق"),
                    &[
                        value(
                            "id",
                            "ID",
                            true,
                            &[],
                            ("Approval identifier", "معرّف طلب الموافقة"),
                        ),
                        value(
                            "decision",
                            "DECISION",
                            true,
                            &["once", "always", "deny"],
                            (
                                "Share once, always, or deny",
                                "شارك مرة واحدة أو دائمًا، أو ارفض",
                            ),
                        ),
                    ],
                    &[],
                ),
                command(
                    "add",
                    ("Allow a folder to be shared", "اسمح بمشاركة مجلد"),
                    &[value(
                        "path",
                        "PATH",
                        true,
                        &[],
                        ("Folder to share", "المجلد المراد مشاركته"),
                    )],
                    &[],
                ),
                command(
                    "remove",
                    (
                        "Remove a shared folder from the allowlist",
                        "أزل مجلدًا من قائمة المشاركة",
                    ),
                    &[value(
                        "share",
                        "SHARE",
                        true,
                        &[],
                        ("Shared folder or ID", "المجلد المشترك أو معرّفه"),
                    )],
                    &[],
                ),
            ],
        ),
        command(
            "disk",
            ("Manage VM disk space", "أدر مساحة قرص الآلة الافتراضية"),
            &[],
            &[
                leaf("usage", ("Show disk usage", "اعرض استخدام القرص")),
                command(
                    "cleanup",
                    (
                        "Remove unused container data",
                        "احذف بيانات الحاويات غير المستخدمة",
                    ),
                    &[flag(
                        "dry-run",
                        ("Preview the cleanup", "اعرض معاينة التنظيف"),
                    )],
                    &[],
                ),
                leaf(
                    "reclaim",
                    (
                        "Return free space from a running VM to macOS",
                        "أعد المساحة الحرة من آلة افتراضية قيد التشغيل إلى macOS",
                    ),
                ),
            ],
        ),
        command(
            "network",
            ("Manage networking", "أدر الشبكة"),
            &[],
            &[
                leaf("status", ("Show network status", "اعرض حالة الشبكة")),
                command(
                    "mode",
                    (
                        "Show or save the network mode for the next start",
                        "اعرض وضع الشبكة أو احفظه لبدء التشغيل التالي",
                    ),
                    &[value(
                        "mode",
                        "MODE",
                        false,
                        &["auto", "nat", "userspace"],
                        ("Network mode", "وضع الشبكة"),
                    )],
                    &[],
                ),
            ],
        ),
        command(
            "logs",
            (
                "Show daemon and guest logs",
                "اعرض سجلات الخادم والنظام الضيف",
            ),
            &[flag(
                "follow",
                ("Follow new log entries", "تابع إدخالات السجل الجديدة"),
            )],
            &[],
        ),
        leaf(
            "diagnose",
            ("Collect a diagnostics bundle", "اجمع حزمة تشخيص"),
        ),
        command(
            "update",
            ("Manage app updates", "أدر تحديثات التطبيق"),
            &[],
            &[
                leaf("check", ("Check for updates", "تحقّق من التحديثات")),
                leaf("apply", ("Apply an update", "طبّق تحديثًا")),
            ],
        ),
        leaf(
            "mcp",
            (
                "Run the MCP server on stdio",
                "شغّل خادم MCP عبر الإدخال والإخراج القياسيين",
            ),
        ),
        command(
            "agents",
            ("Manage agent permissions", "أدر أذونات الوكلاء"),
            &[],
            &[
                leaf("list", ("List agent permissions", "اسرد أذونات الوكلاء")),
                command(
                    "approve",
                    ("Approve an agent request", "وافق على طلب وكيل"),
                    &[REQUEST],
                    &[],
                ),
                command(
                    "deny",
                    ("Deny an agent request", "ارفض طلب وكيل"),
                    &[REQUEST],
                    &[],
                ),
                command(
                    "revoke",
                    ("Revoke an agent grant", "ألغِ إذن وكيل"),
                    &[value(
                        "grant",
                        "GRANT",
                        true,
                        &[],
                        ("Agent grant ID", "معرّف إذن الوكيل"),
                    )],
                    &[],
                ),
            ],
        ),
        command(
            "reset",
            (
                "Reset Marsa settings and optionally erase VM data",
                "أعد ضبط إعدادات مرسى ويمكنك مسح بيانات الآلة الافتراضية",
            ),
            &[flag(
                "factory",
                (
                    "Erase all Marsa settings and VM data",
                    "امسح جميع إعدادات مرسى وبيانات الآلة الافتراضية",
                ),
            )],
            &[],
        ),
        leaf(
            "version",
            ("Show component versions", "اعرض إصدارات المكوّنات"),
        ),
        command(
            "completion",
            ("Generate shell completions", "أنشئ إكمالات الصدفة"),
            &[value(
                "shell",
                "SHELL",
                true,
                &["bash", "zsh", "fish"],
                ("Target shell", "الصدفة المستهدفة"),
            )],
            &[],
        ),
    ],
);

pub(super) fn build(language: Language) -> Command {
    ROOT.build("marsa", language)
        .arg(JSON.clap(language).global(true))
        .arg(YES.clap(language).global(true))
}

impl Spec {
    fn build(&self, path: &str, language: Language) -> Command {
        Command::new(self.name)
            .about(language.text(self.about.0, self.about.1))
            .disable_help_subcommand(true)
            .disable_help_flag(true)
            .arg(
                Arg::new("help")
                    .short('h')
                    .long("help")
                    .action(ArgAction::Help)
                    .help(language.text(HELP.0, HELP.1)),
            )
            .args(self.arguments.iter().map(|arg| arg.clap(language)))
            .subcommands(
                self.commands
                    .iter()
                    .map(|child| child.build(&format!("{path} {}", child.name), language)),
            )
            .override_help(self.help(path, language))
            .subcommand_required(!self.commands.is_empty())
    }

    fn help(&self, path: &str, language: Language) -> String {
        let usage = language.text("Usage", "الاستخدام");
        let options = language.text("Options", "الخيارات");
        let commands = language.text("Commands", "الأوامر");
        let arguments = language.text("Arguments", "الوسائط");
        let options_label = language.text("OPTIONS", "الخيارات");
        let command_label = language.text("COMMAND", "الأمر");
        let about = language.text(self.about.0, self.about.1);
        let mut help = format!("{about}\n\n{usage}\n  {path} [{options_label}]");
        if !self.commands.is_empty() {
            help.push_str(&format!(" <{command_label}>"));
        }
        for arg in self.arguments {
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
                let about = language.text(command.about.0, command.about.1);
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
                if let Form::Positional { label, values, .. } = arg.form {
                    let about = language.text(arg.about.0, arg.about.1);
                    help.push_str(&format!("  {label:<14} {about}"));
                    if !values.is_empty() {
                        help.push_str(&format!(" ({})", values.join(", ")));
                    }
                    help.push('\n');
                }
            }
        }
        help.push_str(&format!(
            "\n{options}\n  -h, --help     {}\n",
            language.text(HELP.0, HELP.1)
        ));
        for arg in [JSON, YES].iter().chain(self.arguments) {
            if matches!(arg.form, Form::Flag) {
                let about = language.text(arg.about.0, arg.about.1);
                help.push_str(&format!("  --{:<12} {about}\n", arg.id));
            }
        }
        help
    }
}

impl Argument {
    fn clap(&self, language: Language) -> Arg {
        let arg = Arg::new(self.id).help(language.text(self.about.0, self.about.1));
        match self.form {
            Form::Flag => arg.long(self.id).action(ArgAction::SetTrue),
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
