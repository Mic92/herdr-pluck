mod detect;
mod error;
mod osc52;
mod runner;
mod system;
mod tool;

pub use error::ClipboardError;

use detect::select_tool;
use osc52::write_osc52;
use runner::ClipboardCommandRunner;
use std::io::Write;
use system::SystemCommandRunner;
use tool::ClipboardEnvironment;

/// Copy mechanism, selected via `HERDR_PLUCK_CLIPBOARD`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ClipboardBackend {
    /// System clipboard commands, falling back to OSC 52.
    #[default]
    Auto,
    System,
    Osc52,
}

impl ClipboardBackend {
    fn parse(name: &str) -> Option<Self> {
        match name {
            "auto" => Some(Self::Auto),
            "system" => Some(Self::System),
            "osc52" => Some(Self::Osc52),
            _ => None,
        }
    }

    /// Resolves the backend: HERDR_PLUCK_CLIPBOARD wins over the config file
    /// value, which wins over the default.
    pub fn resolve(configured: Option<&str>) -> Self {
        if let Ok(value) = std::env::var("HERDR_PLUCK_CLIPBOARD") {
            if let Some(backend) = Self::parse(&value) {
                return backend;
            }
        }
        configured.and_then(Self::parse).unwrap_or_default()
    }

    pub fn from_env() -> Self {
        Self::resolve(None)
    }
}

/// Successful clipboard copy metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopySuccess {
    pub tool: String,
}

/// Clipboard abstraction used by picker code and tests.
pub trait Clipboard {
    fn copy(&self, text: &str) -> Result<CopySuccess, ClipboardError>;
}

/// System clipboard implementation using available platform command-line tools.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClipboard {
    backend: ClipboardBackend,
}

impl SystemClipboard {
    pub fn new(backend: ClipboardBackend) -> Self {
        Self { backend }
    }
}

impl Clipboard for SystemClipboard {
    fn copy(&self, text: &str) -> Result<CopySuccess, ClipboardError> {
        copy_with_backend(
            text,
            &SystemCommandRunner,
            ClipboardEnvironment::current(),
            self.backend,
            &mut std::io::stdout(),
        )
    }
}

/// Copies text to the system clipboard with the default fallback adapter.
pub fn copy_to_system_clipboard(text: &str) -> Result<CopySuccess, ClipboardError> {
    SystemClipboard::new(ClipboardBackend::from_env()).copy(text)
}

fn copy_with_backend(
    text: &str,
    runner: &impl ClipboardCommandRunner,
    env: ClipboardEnvironment,
    backend: ClipboardBackend,
    osc_output: &mut impl Write,
) -> Result<CopySuccess, ClipboardError> {
    match backend {
        ClipboardBackend::System => copy_with_runner(text, runner, env),
        ClipboardBackend::Osc52 => copy_with_osc52(text, osc_output),
        // Without a Wayland or X11 session the tools would fail or hit the
        // wrong display, so go straight to OSC 52.
        ClipboardBackend::Auto
            if env.os != tool::ClipboardOs::Macos && !env.wayland && !env.x11 =>
        {
            copy_with_osc52(text, osc_output)
        }
        ClipboardBackend::Auto => match copy_with_runner(text, runner, env) {
            Err(ClipboardError::NoToolFound { .. }) => copy_with_osc52(text, osc_output),
            other => other,
        },
    }
}

fn copy_with_osc52(text: &str, output: &mut impl Write) -> Result<CopySuccess, ClipboardError> {
    write_osc52(output, text)?;
    Ok(CopySuccess {
        tool: "osc52".to_string(),
    })
}

fn copy_with_runner(
    text: &str,
    runner: &impl ClipboardCommandRunner,
    env: ClipboardEnvironment,
) -> Result<CopySuccess, ClipboardError> {
    let (selected, candidates) = select_tool(runner, env);
    let Some(tool) = selected else {
        return Err(ClipboardError::no_tool_found(&candidates));
    };

    runner.run_with_stdin(tool, text)?;
    Ok(CopySuccess {
        tool: tool.name.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clipboard::tool::{ClipboardOs, ClipboardTool};
    use std::cell::RefCell;
    use std::collections::HashSet;

    #[derive(Default)]
    struct FakeRunner {
        available: HashSet<&'static str>,
        runs: RefCell<Vec<(&'static str, Vec<&'static str>, String)>>,
        failure: Option<ClipboardError>,
    }

    impl ClipboardCommandRunner for FakeRunner {
        fn command_exists(&self, command: &str) -> bool {
            self.available.contains(command)
        }

        fn run_with_stdin(&self, tool: ClipboardTool, stdin: &str) -> Result<(), ClipboardError> {
            self.runs
                .borrow_mut()
                .push((tool.name, tool.args.to_vec(), stdin.to_string()));
            if let Some(error) = &self.failure {
                Err(error.clone())
            } else {
                Ok(())
            }
        }
    }

    fn env(os: ClipboardOs, wayland: bool, x11: bool) -> ClipboardEnvironment {
        ClipboardEnvironment { os, wayland, x11 }
    }

    #[test]
    fn copies_with_pbcopy_on_macos() {
        let runner = FakeRunner {
            available: HashSet::from(["pbcopy"]),
            ..FakeRunner::default()
        };

        let success = copy_with_runner(
            "https://example.com",
            &runner,
            env(ClipboardOs::Macos, false, false),
        )
        .unwrap();

        assert_eq!(success.tool, "pbcopy");
        assert_eq!(
            runner.runs.borrow().as_slice(),
            &[("pbcopy", Vec::new(), "https://example.com".to_string())]
        );
    }

    #[test]
    fn copies_with_wayland_tool_when_available() {
        let runner = FakeRunner {
            available: HashSet::from(["wl-copy", "xclip"]),
            ..FakeRunner::default()
        };

        let success =
            copy_with_runner("token", &runner, env(ClipboardOs::Other, true, true)).unwrap();

        assert_eq!(success.tool, "wl-copy");
        assert_eq!(runner.runs.borrow()[0].0, "wl-copy");
    }

    #[test]
    fn copies_with_xclip_arguments_on_x11() {
        let runner = FakeRunner {
            available: HashSet::from(["xclip"]),
            ..FakeRunner::default()
        };

        let success =
            copy_with_runner("/tmp/file", &runner, env(ClipboardOs::Other, false, true)).unwrap();

        assert_eq!(success.tool, "xclip");
        assert_eq!(
            runner.runs.borrow().as_slice(),
            &[(
                "xclip",
                vec!["-selection", "clipboard"],
                "/tmp/file".to_string()
            )]
        );
    }

    #[test]
    fn copies_with_xsel_arguments_when_xclip_missing() {
        let runner = FakeRunner {
            available: HashSet::from(["xsel"]),
            ..FakeRunner::default()
        };

        let success =
            copy_with_runner("abcdef1", &runner, env(ClipboardOs::Other, false, true)).unwrap();

        assert_eq!(success.tool, "xsel");
        assert_eq!(
            runner.runs.borrow().as_slice(),
            &[(
                "xsel",
                vec!["--clipboard", "--input"],
                "abcdef1".to_string()
            )]
        );
    }

    #[test]
    fn reports_no_supported_tool_with_tried_list() {
        let runner = FakeRunner::default();

        let error =
            copy_with_runner("unused", &runner, env(ClipboardOs::Other, false, false)).unwrap_err();

        assert_eq!(
            error,
            ClipboardError::NoToolFound {
                tried: "pbcopy, wl-copy, xclip, xsel".to_string()
            }
        );
        assert!(runner.runs.borrow().is_empty());
    }

    #[test]
    fn auto_backend_uses_osc52_on_linux_without_display() {
        let runner = FakeRunner {
            available: HashSet::from(["wl-copy", "xclip"]),
            ..FakeRunner::default()
        };
        let mut osc_output = Vec::new();

        let success = copy_with_backend(
            "hello",
            &runner,
            env(ClipboardOs::Other, false, false),
            ClipboardBackend::Auto,
            &mut osc_output,
        )
        .unwrap();

        assert_eq!(success.tool, "osc52");
        assert!(runner.runs.borrow().is_empty());
    }

    #[test]
    fn auto_backend_falls_back_to_osc52_when_no_tool_exists() {
        let runner = FakeRunner::default();
        let mut osc_output = Vec::new();

        let success = copy_with_backend(
            "hello",
            &runner,
            env(ClipboardOs::Other, false, true),
            ClipboardBackend::Auto,
            &mut osc_output,
        )
        .unwrap();

        assert_eq!(success.tool, "osc52");
        assert_eq!(osc_output, b"\x1b]52;c;aGVsbG8=\x07");
        assert!(runner.runs.borrow().is_empty());
    }

    #[test]
    fn auto_backend_prefers_system_tool_when_available() {
        let runner = FakeRunner {
            available: HashSet::from(["wl-copy"]),
            ..FakeRunner::default()
        };
        let mut osc_output = Vec::new();

        let success = copy_with_backend(
            "token",
            &runner,
            env(ClipboardOs::Other, true, false),
            ClipboardBackend::Auto,
            &mut osc_output,
        )
        .unwrap();

        assert_eq!(success.tool, "wl-copy");
        assert!(osc_output.is_empty());
    }

    #[test]
    fn osc52_backend_skips_system_tools_entirely() {
        let runner = FakeRunner {
            available: HashSet::from(["wl-copy"]),
            ..FakeRunner::default()
        };
        let mut osc_output = Vec::new();

        let success = copy_with_backend(
            "hi",
            &runner,
            env(ClipboardOs::Other, true, false),
            ClipboardBackend::Osc52,
            &mut osc_output,
        )
        .unwrap();

        assert_eq!(success.tool, "osc52");
        assert_eq!(osc_output, b"\x1b]52;c;aGk=\x07");
        assert!(runner.runs.borrow().is_empty());
    }

    #[test]
    fn surfaces_command_execution_failure() {
        let runner = FakeRunner {
            available: HashSet::from(["wl-copy"]),
            failure: Some(ClipboardError::CommandFailed {
                tool: "wl-copy".to_string(),
                status: "exit status: 1".to_string(),
            }),
            ..FakeRunner::default()
        };

        let error =
            copy_with_runner("token", &runner, env(ClipboardOs::Other, true, false)).unwrap_err();

        assert_eq!(
            error,
            ClipboardError::CommandFailed {
                tool: "wl-copy".to_string(),
                status: "exit status: 1".to_string(),
            }
        );
        assert_eq!(runner.runs.borrow()[0].2, "token");
    }
}
