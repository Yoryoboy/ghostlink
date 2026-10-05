//! Consent and platform-neutral URI handler CLI operations.

use std::io::{BufRead, Read, Write};

use destiny_app::uri_handler::{
    AutoCallbackNotice, AutoCallbackResolution, HandlerChanges, HandlerRegistration,
    HandlerWarningKind, InstallConsent, UnsupportedRegistrar, UriHandlerRegistrar,
    resolve_auto_callback,
};
use destiny_app::{AppError, ErrorCategory};
use serde_json::json;

use crate::command::{CliError, HandlerCommand};
use crate::output::{OutputMode, sanitize};
use crate::runner::ProcessOutput;

/// A bounded consent prompt with injectable input and diagnostic output.
pub struct TerminalConsent<R, W> {
    pub reader: R,
    pub writer: W,
}

impl<R: BufRead, W: Write> InstallConsent for TerminalConsent<R, W> {
    fn confirm(&mut self, plan: &HandlerChanges) -> Result<bool, AppError> {
        let error = |_| AppError::new(ErrorCategory::UnsupportedOperation);
        writeln!(
            self.writer,
            "The ghostlink:// handler is not set up for this executable. Planned changes:"
        )
        .map_err(error)?;
        for (index, change) in plan.changes.iter().enumerate() {
            writeln!(self.writer, "{}. {}", index + 1, sanitize(change)).map_err(error)?;
        }
        write!(self.writer, "Install the handler now? [y/N] ").map_err(error)?;
        self.writer.flush().map_err(error)?;
        // Limit bytes before decoding or trimming: oversized whitespace must not
        // turn an incomplete "yes" prefix into consent. Do not drain unbounded input.
        let mut line = Vec::new();
        self.reader
            .by_ref()
            .take(257)
            .read_until(b'\n', &mut line)
            .map_err(error)?;
        if line.len() > 256 {
            return Ok(false);
        }
        let Ok(line) = std::str::from_utf8(&line) else {
            return Ok(false);
        };
        Ok(matches!(
            line.trim().to_ascii_lowercase().as_str(),
            "y" | "yes"
        ))
    }
}

/// Resolve auto without exposing a prompt unless both terminals permit it.
pub fn resolve_callback(
    registrar: &mut dyn UriHandlerRegistrar,
    local_browser: bool,
    stdin_terminal: bool,
    stdout_terminal: bool,
    mode: OutputMode,
    consent: &mut dyn InstallConsent,
) -> AutoCallbackResolution {
    let consent = if stdin_terminal && stdout_terminal && mode != OutputMode::Json {
        Some(consent)
    } else {
        None
    };
    resolve_auto_callback(registrar, local_browser, consent)
}

const PASTE_NOTICE: &str =
    "Using terminal paste; run `ghostlink handler install` to set up the handler.";

/// Render one policy notice as a sanitized diagnostic line.
pub fn render_notice(notice: &AutoCallbackNotice) -> String {
    use AutoCallbackNotice::*;
    match notice {
        NoLocalBrowser => "Using terminal paste; no local browser is available.".into(),
        StatusUnavailable(category) => format!(
            "Handler status unavailable ({}). {PASTE_NOTICE}",
            crate::command::category_outcome(*category).code
        ),
        NotInteractive(_) | Declined => PASTE_NOTICE.into(),
        PromptFailed(category) => format!(
            "Handler prompt failed ({}). {PASTE_NOTICE}",
            crate::command::category_outcome(*category).code
        ),
        Installed(changes) => format!(
            "Handler installed: {}",
            changes
                .changes
                .iter()
                .map(|s| sanitize(s))
                .collect::<Vec<_>>()
                .join("; ")
        ),
        InstallFailed(category) => format!(
            "Handler install failed ({}). {PASTE_NOTICE}",
            crate::command::category_outcome(*category).code
        ),
        InstallNotEffective => format!("Handler install was not effective. {PASTE_NOTICE}"),
        Warnings(warnings) => warnings
            .iter()
            .map(|warning| {
                format!(
                    "{}: {}",
                    sanitize(&warning.summary),
                    sanitize(&warning.remedy)
                )
            })
            .collect::<Vec<_>>()
            .join("; "),
    }
}

/// Choose the concrete platform adapter only at the CLI composition root.
pub(crate) fn production_registrar() -> Box<dyn UriHandlerRegistrar> {
    #[cfg(target_os = "linux")]
    if let Ok(registrar) =
        destiny_adapters::uri_handler::LinuxUriHandlerRegistrar::for_current_executable()
    {
        return Box::new(registrar);
    }
    Box::new(UnsupportedRegistrar)
}

/// Dispatch an explicit, non-interactive handler operation.
pub fn run_handler(
    registrar: &mut dyn UriHandlerRegistrar,
    command: HandlerCommand,
    mode: OutputMode,
) -> ProcessOutput {
    let rendered = match command {
        HandlerCommand::Status => registrar.status().map(|status| {
            let (registration, handler) = match &status.registration {
                HandlerRegistration::Current => ("current", None),
                HandlerRegistration::Missing => ("missing", None),
                HandlerRegistration::Elsewhere { handler } => ("elsewhere", Some(handler)),
            };
            if mode == OutputMode::Json {
                json!({
                    "kind": "uri_handler_status",
                    "registration": registration,
                    "handler": handler,
                    "warnings": status.warnings.iter().map(|warning| json!({
                        "kind": match warning.kind {
                            HandlerWarningKind::ConflictingAssociation => "conflicting_association",
                            HandlerWarningKind::RememberedChoice => "remembered_choice",
                        },
                        "summary": warning.summary,
                        "remedy": warning.remedy,
                    })).collect::<Vec<_>>()
                })
                .to_string()
            } else {
                let mut lines = vec![match handler {
                    Some(handler) => format!("elsewhere: {}", sanitize(handler)),
                    None => registration.into(),
                }];
                lines.extend(status.warnings.iter().map(|warning| {
                    format!(
                        "{}: {}",
                        sanitize(&warning.summary),
                        sanitize(&warning.remedy)
                    )
                }));
                lines.join("\n")
            }
        }),
        HandlerCommand::Install | HandlerCommand::Uninstall => {
            let (result, kind) = match command {
                HandlerCommand::Install => (registrar.install(), "uri_handler_install"),
                _ => (registrar.uninstall(), "uri_handler_uninstall"),
            };
            result.map(|changes| {
                if mode == OutputMode::Json {
                    json!({"kind": kind, "changes": changes.changes}).to_string()
                } else if changes.changes.is_empty() {
                    match command {
                        HandlerCommand::Uninstall => "Nothing was installed.".into(),
                        _ => "No changes needed.".into(),
                    }
                } else {
                    changes
                        .changes
                        .iter()
                        .map(|s| sanitize(s))
                        .collect::<Vec<_>>()
                        .join("\n")
                }
            })
        }
    };
    match rendered {
        Ok(body) => ProcessOutput::success(body),
        Err(error) => ProcessOutput::failure(mode, CliError::from(error)),
    }
}

#[cfg(test)]
#[path = "handler_tests.rs"]
mod tests;
