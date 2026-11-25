# English and Arabic desktop glossary

Marsa uses Modern Standard Arabic and Western digits (0–9). Apply these
terms to its [Fluent catalogs](README.md) and the
[GPUI Component catalog extension](components/ar.yml). The project owner
reviews terminology and the four desktop variants under
[M7-14](../docs/plan/M7-gui-features.md#m7-14-arabic-review-and-four-variant-audit-gate-g5), as recorded
in [D-035](../docs/15-decisions.md#d-035-the-project-owner-reviews-arabic-terminology).

| English | Arabic | Usage |
| --- | --- | --- |
| Container | حاوية | A running or stopped container, including in lists and confirmations. |
| Image | صورة | A container image. Write **صورة الحاوية** when the context is ambiguous. |
| Tag | وسم | A Docker image tag. Names and values stay as typed. |
| Image layer | طبقة الصورة | One layer in an image or pull. |
| Engine | محرك | The Docker-compatible engine. |
| Environment | بيئة | A selected engine and its execution environment. |
| Volume | وحدة تخزين | A Docker volume. Use **مجلّد** for a directory on disk. |
| Network | شبكة | A Docker network. |
| Port | منفذ | A published or exposed port. Use Western digits. |
| Bind mount | ربط مجلّد | A directory mapped from the host. Use **نقطة الربط** for its mount point. |
| Registry | سجل الصور | An image registry. Use **سجل** or **سجلات** for application or container logs in context. |
| Repository | مستودع | A Git or image repository. Names stay as provided. |
| Stack | مكدس | Services deployed together from Compose. |
| Compose project | مشروع Compose | The named project used by Compose. Leave Compose in Latin script. |
| Build | بناء | Build an image or other artifact. |
| Pull / push | سحب / دفع | Transfer an image from or to a registry. |
| Secret | سر | A credential or value intended for scoped use. |
| Keychain | سلسلة المفاتيح | The macOS credential store. |
| Approval | موافقة | Consent to a request or operation. Use **تأكيد** for confirmation. |
| Inspect | فحص | Read resource metadata. Use **فحص الأمان** for security scanning when both are present. |
| Disk space | مساحة القرص | Host or VM data storage. |
| Cancel / copy | إلغاء / نسخ | Keep the application and component action labels consistent. |
| Patch / diff | رقعة / فروق | A patch is an applicable change. A diff is a comparison. |
| UTC | UTC | Keep the time-zone abbreviation in Latin script. |
| Byte | بايت | Use localized SI labels in decimal sizes and IEC symbols such as MiB or GiB for binary sizes. |

Keep brands, terminal commands, logs, identifiers, paths, image references
and user-supplied names in their original form. In right-to-left copy, isolate
technical values when they need a fixed left-to-right reading order. Keep the
placeholder names in both translation catalogs unchanged.
