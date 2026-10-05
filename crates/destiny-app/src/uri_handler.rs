//! Operating-system registration of the `ghostlink://` callback handler.
//!
//! The login can receive the provider's custom-scheme callback through a desktop
//! handler instead of a terminal paste, but only when the operating system
//! routes the scheme to this executable. This module owns the platform-neutral
//! port for inspecting and changing that registration, and the policy that the
//! default `auto` callback strategy follows. Platform adapters implement
//! [`UriHandlerRegistrar`]; only a composition root chooses one.
//!
//! The policy never changes the system without consent: a missing or foreign
//! registration is installed only after the person confirmed the exact planned
//! changes, and without an interactive consent surface it always falls back to
//! the terminal paste.

use destiny_core::{AppError, ErrorCategory};

/// Where the operating system currently routes the callback scheme.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HandlerRegistration {
    /// The scheme opens this executable's callback handler.
    Current,
    /// No handler is associated with the scheme.
    Missing,
    /// Another application, or this handler for a different executable.
    Elsewhere {
        /// A human-readable description of the current handler.
        handler: String,
    },
}

/// What kind of known platform pitfall a warning reports.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HandlerWarningKind {
    /// The association database names different handlers in different places.
    ConflictingAssociation,
    /// A desktop service remembers another application for the scheme and may
    /// launch it regardless of the association.
    RememberedChoice,
}

/// A known platform pitfall that can route the callback away from this handler.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HandlerWarning {
    pub kind: HandlerWarningKind,
    /// What was found, in one sentence.
    pub summary: String,
    /// How the person can fix it, in one sentence.
    pub remedy: String,
}

/// The registration plus every detected pitfall.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HandlerStatus {
    pub registration: HandlerRegistration,
    pub warnings: Vec<HandlerWarning>,
}

/// An ordered, human-readable list of exact system changes.
///
/// A plan lists the changes an install would make; a report lists the changes
/// an install or uninstall made. Each entry names one file or one command.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct HandlerChanges {
    pub changes: Vec<String>,
}

/// Inspects and changes the callback-scheme registration for this executable.
pub trait UriHandlerRegistrar {
    /// Reports the current registration without changing anything.
    fn status(&mut self) -> Result<HandlerStatus, AppError>;

    /// Lists the exact changes [`UriHandlerRegistrar::install`] would make,
    /// without changing anything.
    fn install_plan(&mut self) -> Result<HandlerChanges, AppError>;

    /// Registers this executable as the handler, recording the previous handler
    /// for [`UriHandlerRegistrar::uninstall`] unless a record already exists.
    fn install(&mut self) -> Result<HandlerChanges, AppError>;

    /// Removes this handler and restores the recorded previous handler.
    fn uninstall(&mut self) -> Result<HandlerChanges, AppError>;
}

/// A platform without a handler registrar.
///
/// Every operation is an explicit unsupported operation, so the `auto` policy
/// falls back to the terminal paste.
#[derive(Clone, Copy, Debug, Default)]
pub struct UnsupportedRegistrar;

impl UriHandlerRegistrar for UnsupportedRegistrar {
    fn status(&mut self) -> Result<HandlerStatus, AppError> {
        Err(AppError::new(ErrorCategory::UnsupportedOperation))
    }

    fn install_plan(&mut self) -> Result<HandlerChanges, AppError> {
        Err(AppError::new(ErrorCategory::UnsupportedOperation))
    }

    fn install(&mut self) -> Result<HandlerChanges, AppError> {
        Err(AppError::new(ErrorCategory::UnsupportedOperation))
    }

    fn uninstall(&mut self) -> Result<HandlerChanges, AppError> {
        Err(AppError::new(ErrorCategory::UnsupportedOperation))
    }
}

/// Asks the person whether the planned changes may be applied.
pub trait InstallConsent {
    /// Presents `plan` and returns whether the person confirmed it.
    fn confirm(&mut self, plan: &HandlerChanges) -> Result<bool, AppError>;
}

/// The callback strategy the `auto` policy selected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CallbackChoice {
    Handler,
    Paste,
}

/// Something the person should be told about how `auto` decided.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AutoCallbackNotice {
    /// No local browser can reach this machine's handler (for example over SSH).
    NoLocalBrowser,
    /// The registration could not be inspected or planned.
    StatusUnavailable(ErrorCategory),
    /// The handler is not usable and there is no one to ask.
    NotInteractive(HandlerRegistration),
    /// The consent prompt itself failed.
    PromptFailed(ErrorCategory),
    /// The person declined the planned changes.
    Declined,
    /// The handler was installed with these changes.
    Installed(HandlerChanges),
    /// Installing failed; nothing is claimed about partial changes beyond the
    /// category.
    InstallFailed(ErrorCategory),
    /// The install finished but the registration still does not point here.
    InstallNotEffective,
    /// Known pitfalls that may still route the callback elsewhere.
    Warnings(Vec<HandlerWarning>),
}

/// The selected strategy and the notices explaining it, in order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AutoCallbackResolution {
    pub choice: CallbackChoice,
    pub notices: Vec<AutoCallbackNotice>,
}

impl AutoCallbackResolution {
    fn paste(notices: Vec<AutoCallbackNotice>) -> Self {
        Self {
            choice: CallbackChoice::Paste,
            notices,
        }
    }

    fn handler(mut notices: Vec<AutoCallbackNotice>, warnings: Vec<HandlerWarning>) -> Self {
        if !warnings.is_empty() {
            notices.push(AutoCallbackNotice::Warnings(warnings));
        }
        Self {
            choice: CallbackChoice::Handler,
            notices,
        }
    }
}

/// Resolves the default `auto` callback strategy.
///
/// - Without a local browser the handler cannot receive the redirect, so the
///   paste is used and the registration is not inspected.
/// - A handler that already points at this executable is used.
/// - Otherwise the exact install plan is offered through `consent`. Without a
///   consent surface (`None`: not a terminal, or machine output) nothing is
///   asked and the paste is used. A declined, failed, or ineffective install
///   also falls back to the paste.
///
/// Every fallback is reported as a notice; no failure here aborts the login,
/// because the paste works everywhere.
pub fn resolve_auto_callback(
    registrar: &mut dyn UriHandlerRegistrar,
    local_browser: bool,
    consent: Option<&mut dyn InstallConsent>,
) -> AutoCallbackResolution {
    use AutoCallbackNotice as Notice;

    if !local_browser {
        return AutoCallbackResolution::paste(vec![Notice::NoLocalBrowser]);
    }
    let status = match registrar.status() {
        Ok(status) => status,
        Err(error) => {
            return AutoCallbackResolution::paste(vec![Notice::StatusUnavailable(
                error.category(),
            )]);
        }
    };
    if status.registration == HandlerRegistration::Current {
        return AutoCallbackResolution::handler(Vec::new(), status.warnings);
    }
    let Some(consent) = consent else {
        return AutoCallbackResolution::paste(vec![Notice::NotInteractive(status.registration)]);
    };
    let plan = match registrar.install_plan() {
        Ok(plan) => plan,
        Err(error) => {
            return AutoCallbackResolution::paste(vec![Notice::StatusUnavailable(
                error.category(),
            )]);
        }
    };
    match consent.confirm(&plan) {
        Ok(true) => {}
        Ok(false) => return AutoCallbackResolution::paste(vec![Notice::Declined]),
        Err(error) => {
            return AutoCallbackResolution::paste(vec![Notice::PromptFailed(error.category())]);
        }
    }
    let installed = match registrar.install() {
        Ok(installed) => installed,
        Err(error) => {
            return AutoCallbackResolution::paste(vec![Notice::InstallFailed(error.category())]);
        }
    };
    let notices = vec![Notice::Installed(installed)];
    match registrar.status() {
        Ok(after) if after.registration == HandlerRegistration::Current => {
            AutoCallbackResolution::handler(notices, after.warnings)
        }
        _ => {
            let mut notices = notices;
            notices.push(Notice::InstallNotEffective);
            AutoCallbackResolution::paste(notices)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct FakeRegistrar {
        statuses: Vec<Result<HandlerStatus, AppError>>,
        plan: Option<Result<HandlerChanges, AppError>>,
        install: Option<Result<HandlerChanges, AppError>>,
        calls: Vec<&'static str>,
    }

    impl FakeRegistrar {
        fn with_statuses(statuses: Vec<Result<HandlerStatus, AppError>>) -> Self {
            Self {
                statuses,
                plan: Some(Ok(changes(&["write entry"]))),
                install: Some(Ok(changes(&["wrote entry"]))),
                ..Self::default()
            }
        }
    }

    impl UriHandlerRegistrar for FakeRegistrar {
        fn status(&mut self) -> Result<HandlerStatus, AppError> {
            self.calls.push("status");
            self.statuses.remove(0)
        }

        fn install_plan(&mut self) -> Result<HandlerChanges, AppError> {
            self.calls.push("plan");
            self.plan.take().expect("plan requested once")
        }

        fn install(&mut self) -> Result<HandlerChanges, AppError> {
            self.calls.push("install");
            self.install.take().expect("install requested once")
        }

        fn uninstall(&mut self) -> Result<HandlerChanges, AppError> {
            self.calls.push("uninstall");
            Ok(HandlerChanges::default())
        }
    }

    struct Consent {
        answer: Result<bool, AppError>,
        seen: Option<HandlerChanges>,
    }

    impl Consent {
        fn answering(answer: Result<bool, AppError>) -> Self {
            Self { answer, seen: None }
        }
    }

    impl InstallConsent for Consent {
        fn confirm(&mut self, plan: &HandlerChanges) -> Result<bool, AppError> {
            self.seen = Some(plan.clone());
            self.answer.clone()
        }
    }

    fn changes(lines: &[&str]) -> HandlerChanges {
        HandlerChanges {
            changes: lines.iter().map(|line| (*line).to_owned()).collect(),
        }
    }

    fn status(registration: HandlerRegistration) -> Result<HandlerStatus, AppError> {
        Ok(HandlerStatus {
            registration,
            warnings: Vec::new(),
        })
    }

    fn elsewhere() -> HandlerRegistration {
        HandlerRegistration::Elsewhere {
            handler: "google-chrome.desktop".to_owned(),
        }
    }

    fn warning() -> HandlerWarning {
        HandlerWarning {
            kind: HandlerWarningKind::RememberedChoice,
            summary: "portal remembers a browser".to_owned(),
            remedy: "repoint it".to_owned(),
        }
    }

    #[test]
    fn a_handler_pointing_here_is_used_without_asking() {
        let mut registrar =
            FakeRegistrar::with_statuses(vec![status(HandlerRegistration::Current)]);
        let mut consent = Consent::answering(Ok(true));

        let resolution = resolve_auto_callback(&mut registrar, true, Some(&mut consent));

        assert_eq!(resolution.choice, CallbackChoice::Handler);
        assert!(resolution.notices.is_empty());
        assert_eq!(consent.seen, None);
        assert_eq!(registrar.calls, ["status"]);
    }

    #[test]
    fn warnings_on_a_current_handler_are_reported_but_the_handler_is_used() {
        let mut registrar = FakeRegistrar::with_statuses(vec![Ok(HandlerStatus {
            registration: HandlerRegistration::Current,
            warnings: vec![warning()],
        })]);

        let resolution = resolve_auto_callback(&mut registrar, true, None);

        assert_eq!(resolution.choice, CallbackChoice::Handler);
        assert_eq!(
            resolution.notices,
            [AutoCallbackNotice::Warnings(vec![warning()])]
        );
    }

    #[test]
    fn without_a_local_browser_the_paste_is_used_without_inspecting() {
        let mut registrar = FakeRegistrar::default();

        let resolution = resolve_auto_callback(&mut registrar, false, None);

        assert_eq!(resolution.choice, CallbackChoice::Paste);
        assert_eq!(resolution.notices, [AutoCallbackNotice::NoLocalBrowser]);
        assert!(registrar.calls.is_empty());
    }

    #[test]
    fn an_uninspectable_registration_falls_back_to_paste() {
        let mut registrar = FakeRegistrar::with_statuses(vec![Err(AppError::new(
            ErrorCategory::UnsupportedOperation,
        ))]);

        let resolution = resolve_auto_callback(&mut registrar, true, None);

        assert_eq!(resolution.choice, CallbackChoice::Paste);
        assert_eq!(
            resolution.notices,
            [AutoCallbackNotice::StatusUnavailable(
                ErrorCategory::UnsupportedOperation
            )]
        );
    }

    #[test]
    fn without_consent_a_missing_or_foreign_handler_is_never_installed() {
        for registration in [HandlerRegistration::Missing, elsewhere()] {
            let mut registrar = FakeRegistrar::with_statuses(vec![status(registration.clone())]);

            let resolution = resolve_auto_callback(&mut registrar, true, None);

            assert_eq!(resolution.choice, CallbackChoice::Paste);
            assert_eq!(
                resolution.notices,
                [AutoCallbackNotice::NotInteractive(registration)]
            );
            assert_eq!(registrar.calls, ["status"]);
        }
    }

    #[test]
    fn a_declined_plan_changes_nothing_and_falls_back_to_paste() {
        let mut registrar = FakeRegistrar::with_statuses(vec![status(elsewhere())]);
        let mut consent = Consent::answering(Ok(false));

        let resolution = resolve_auto_callback(&mut registrar, true, Some(&mut consent));

        assert_eq!(resolution.choice, CallbackChoice::Paste);
        assert_eq!(resolution.notices, [AutoCallbackNotice::Declined]);
        assert_eq!(consent.seen, Some(changes(&["write entry"])));
        assert_eq!(registrar.calls, ["status", "plan"]);
    }

    #[test]
    fn a_failed_prompt_changes_nothing_and_falls_back_to_paste() {
        let mut registrar =
            FakeRegistrar::with_statuses(vec![status(HandlerRegistration::Missing)]);
        let mut consent = Consent::answering(Err(AppError::new(ErrorCategory::LocalStorage)));

        let resolution = resolve_auto_callback(&mut registrar, true, Some(&mut consent));

        assert_eq!(resolution.choice, CallbackChoice::Paste);
        assert_eq!(
            resolution.notices,
            [AutoCallbackNotice::PromptFailed(
                ErrorCategory::LocalStorage
            )]
        );
        assert_eq!(registrar.calls, ["status", "plan"]);
    }

    #[test]
    fn a_confirmed_install_that_takes_effect_uses_the_handler() {
        let mut registrar = FakeRegistrar::with_statuses(vec![
            status(HandlerRegistration::Missing),
            Ok(HandlerStatus {
                registration: HandlerRegistration::Current,
                warnings: vec![warning()],
            }),
        ]);
        let mut consent = Consent::answering(Ok(true));

        let resolution = resolve_auto_callback(&mut registrar, true, Some(&mut consent));

        assert_eq!(resolution.choice, CallbackChoice::Handler);
        assert_eq!(
            resolution.notices,
            [
                AutoCallbackNotice::Installed(changes(&["wrote entry"])),
                AutoCallbackNotice::Warnings(vec![warning()]),
            ]
        );
        assert_eq!(registrar.calls, ["status", "plan", "install", "status"]);
    }

    #[test]
    fn a_failed_install_falls_back_to_paste() {
        let mut registrar = FakeRegistrar::with_statuses(vec![status(elsewhere())]);
        registrar.install = Some(Err(AppError::new(ErrorCategory::LocalStorage)));
        let mut consent = Consent::answering(Ok(true));

        let resolution = resolve_auto_callback(&mut registrar, true, Some(&mut consent));

        assert_eq!(resolution.choice, CallbackChoice::Paste);
        assert_eq!(
            resolution.notices,
            [AutoCallbackNotice::InstallFailed(
                ErrorCategory::LocalStorage
            )]
        );
    }

    #[test]
    fn an_install_that_does_not_take_effect_falls_back_to_paste() {
        let mut registrar =
            FakeRegistrar::with_statuses(vec![status(elsewhere()), status(elsewhere())]);
        let mut consent = Consent::answering(Ok(true));

        let resolution = resolve_auto_callback(&mut registrar, true, Some(&mut consent));

        assert_eq!(resolution.choice, CallbackChoice::Paste);
        assert_eq!(
            resolution.notices,
            [
                AutoCallbackNotice::Installed(changes(&["wrote entry"])),
                AutoCallbackNotice::InstallNotEffective,
            ]
        );
    }

    #[test]
    fn an_unplannable_install_falls_back_to_paste_without_asking() {
        let mut registrar =
            FakeRegistrar::with_statuses(vec![status(HandlerRegistration::Missing)]);
        registrar.plan = Some(Err(AppError::new(ErrorCategory::ConfigurationError)));
        let mut consent = Consent::answering(Ok(true));

        let resolution = resolve_auto_callback(&mut registrar, true, Some(&mut consent));

        assert_eq!(resolution.choice, CallbackChoice::Paste);
        assert_eq!(consent.seen, None);
        assert_eq!(
            resolution.notices,
            [AutoCallbackNotice::StatusUnavailable(
                ErrorCategory::ConfigurationError
            )]
        );
    }

    #[test]
    fn the_unsupported_registrar_resolves_to_paste() {
        let resolution = resolve_auto_callback(&mut UnsupportedRegistrar, true, None);

        assert_eq!(resolution.choice, CallbackChoice::Paste);
        assert_eq!(
            resolution.notices,
            [AutoCallbackNotice::StatusUnavailable(
                ErrorCategory::UnsupportedOperation
            )]
        );
    }
}
