use std::fmt;
use std::io::{self, IsTerminal};

use anstyle::{AnsiColor, Effects, Style};
use clap::ValueEnum;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "lower")]
pub enum ColorChoice {
    #[default]
    Auto,
    Always,
    Never,
}

#[derive(Debug, Clone, Copy)]
pub enum Stream {
    Stdout,
    Stderr,
}

pub fn resolve(choice: ColorChoice, stream: Stream) -> bool {
    match choice {
        ColorChoice::Always => true,
        ColorChoice::Never => false,
        ColorChoice::Auto => auto_enabled(stream),
    }
}

/// Synchronize the CLI color choice with the environment variables that
/// `anstream`'s auto-detection consults, so painted ANSI sequences survive
/// piping when the user passed `--color=always`.
pub fn apply_env(choice: ColorChoice) {
    match choice {
        ColorChoice::Never => {
            std::env::set_var("NO_COLOR", "1");
            std::env::remove_var("CLICOLOR_FORCE");
        }
        ColorChoice::Always => {
            std::env::set_var("CLICOLOR_FORCE", "1");
            std::env::remove_var("NO_COLOR");
        }
        ColorChoice::Auto => {}
    }
}

fn auto_enabled(stream: Stream) -> bool {
    if env_truthy("CLICOLOR_FORCE") {
        return true;
    }
    if std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty()) {
        return false;
    }
    if std::env::var_os("CLICOLOR").is_some_and(|v| v == "0") {
        return false;
    }
    match stream {
        Stream::Stdout => io::stdout().is_terminal(),
        Stream::Stderr => io::stderr().is_terminal(),
    }
}

fn env_truthy(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|v| !v.is_empty() && v != "0")
}

#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub ok: Style,
    pub error_prefix: Style,
    pub header: Style,
    pub count: Style,
}

impl Palette {
    pub fn new(enabled: bool) -> Self {
        if !enabled {
            return Self::plain();
        }
        let bold = Effects::BOLD;
        Self {
            ok: Style::new().fg_color(Some(AnsiColor::Green.into())).effects(bold),
            error_prefix: Style::new().fg_color(Some(AnsiColor::Red.into())).effects(bold),
            header: Style::new().effects(bold),
            count: Style::new().effects(bold),
        }
    }

    pub fn plain() -> Self {
        let s = Style::new();
        Self {
            ok: s,
            error_prefix: s,
            header: s,
            count: s,
        }
    }
}

pub fn paint<T: fmt::Display>(style: Style, value: T) -> Painted<T> {
    Painted { style, value }
}

pub struct Painted<T> {
    style: Style,
    value: T,
}

impl<T: fmt::Display> fmt::Display for Painted<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}{}{}",
            self.style.render(),
            self.value,
            self.style.render_reset()
        )
    }
}
