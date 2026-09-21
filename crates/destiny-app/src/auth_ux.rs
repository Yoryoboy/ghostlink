//! Application-facing authorization mode selection without transport details.

use destiny_core::{AppError, ErrorCategory};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthorizationMode {
    Auto,
    Browser,
    Headless,
}

pub trait GuiDetector {
    fn gui_available(&self) -> bool;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GuiEnvironment {
    pub os: String,
    pub ssh_connection: bool,
    pub ssh_client: bool,
    pub ssh_tty: bool,
    pub gui_override: Option<bool>,
    pub wayland_display: bool,
    pub x_display: bool,
}

impl GuiEnvironment {
    pub fn current() -> Self {
        Self {
            os: std::env::consts::OS.to_owned(),
            ssh_connection: present("SSH_CONNECTION"),
            ssh_client: present("SSH_CLIENT"),
            ssh_tty: present("SSH_TTY"),
            gui_override: std::env::var_os("DESTINY_GUI").map(|value| value != "0"),
            wayland_display: present("WAYLAND_DISPLAY"),
            x_display: present("DISPLAY"),
        }
    }

    pub fn for_platform(os: impl Into<String>) -> Self {
        Self {
            os: os.into(),
            ssh_connection: false,
            ssh_client: false,
            ssh_tty: false,
            gui_override: None,
            wayland_display: false,
            x_display: false,
        }
    }

    fn ssh_session(&self) -> bool {
        self.ssh_connection || self.ssh_client || self.ssh_tty
    }
}

fn present(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|value| !value.is_empty())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EnvironmentGuiDetector {
    environment: GuiEnvironment,
}

impl Default for EnvironmentGuiDetector {
    fn default() -> Self {
        Self {
            environment: GuiEnvironment::current(),
        }
    }
}

impl EnvironmentGuiDetector {
    pub fn new(environment: GuiEnvironment) -> Self {
        Self { environment }
    }
}

impl GuiDetector for EnvironmentGuiDetector {
    fn gui_available(&self) -> bool {
        if self.environment.ssh_session() {
            return false;
        }
        if let Some(override_value) = self.environment.gui_override {
            return override_value;
        }
        match self.environment.os.as_str() {
            "windows" | "macos" => true,
            _ => self.environment.wayland_display || self.environment.x_display,
        }
    }
}

pub fn select_mode(
    mode: AuthorizationMode,
    detector: &impl GuiDetector,
) -> Result<AuthorizationMode, AppError> {
    match mode {
        AuthorizationMode::Auto if detector.gui_available() => Ok(AuthorizationMode::Browser),
        AuthorizationMode::Auto => Ok(AuthorizationMode::Headless),
        AuthorizationMode::Browser | AuthorizationMode::Headless => Ok(mode),
    }
}

pub fn unavailable_headless_configuration() -> AppError {
    AppError::new(ErrorCategory::ConfigurationError)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Detector(bool);
    impl GuiDetector for Detector {
        fn gui_available(&self) -> bool {
            self.0
        }
    }

    #[test]
    fn auto_mode_is_injectable() {
        assert_eq!(
            select_mode(AuthorizationMode::Auto, &Detector(true)),
            Ok(AuthorizationMode::Browser)
        );
        assert_eq!(
            select_mode(AuthorizationMode::Auto, &Detector(false)),
            Ok(AuthorizationMode::Headless)
        );
    }

    #[test]
    fn explicit_modes_are_not_overridden() {
        assert_eq!(
            select_mode(AuthorizationMode::Browser, &Detector(false)),
            Ok(AuthorizationMode::Browser)
        );
        assert_eq!(
            select_mode(AuthorizationMode::Headless, &Detector(true)),
            Ok(AuthorizationMode::Headless)
        );
    }

    #[test]
    fn ssh_indicators_override_gui_on_all_platforms() {
        for os in ["linux", "macos", "windows"] {
            let mut environment = GuiEnvironment::for_platform(os);
            environment.x_display = true;
            environment.ssh_connection = true;
            assert!(!EnvironmentGuiDetector::new(environment).gui_available());
        }
    }

    #[test]
    fn non_ssh_gui_detection_requires_credible_platform_signal() {
        let mut linux = GuiEnvironment::for_platform("linux");
        assert!(!EnvironmentGuiDetector::new(linux.clone()).gui_available());
        linux.wayland_display = true;
        assert!(EnvironmentGuiDetector::new(linux).gui_available());

        for os in ["macos", "windows"] {
            assert!(EnvironmentGuiDetector::new(GuiEnvironment::for_platform(os)).gui_available());
        }
    }
}
