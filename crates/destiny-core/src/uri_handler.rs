//! Port for the operating-system registration of the `ghostlink://` callback
//! handler.
//!
//! The login can receive the provider's custom-scheme callback through a desktop
//! handler instead of a terminal paste, but only when the operating system
//! routes the scheme to this executable. Platform adapters implement
//! [`UriHandlerRegistrar`]; only a composition root chooses one.

use crate::error::{AppError, ErrorCategory};

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
