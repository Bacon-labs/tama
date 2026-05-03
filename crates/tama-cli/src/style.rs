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

pub fn resolve(choice: ColorChoice, json: bool, stream: Stream) -> bool {
    if json {
        return false;
    }
    match choice {
        ColorChoice::Always => true,
        ColorChoice::Never => false,
        ColorChoice::Auto => auto_enabled(stream),
    }
}

/// Synchronize the CLI color choice with the environment variables that
/// `anstream`'s auto-detection consults, so `anstream::{println,eprintln}!`
/// keep our painted ANSI sequences when the user explicitly forced color on or
/// strips them when the user forced color off, even when stdout is piped.
pub fn apply_env(choice: ColorChoice, json: bool) {
    if json || matches!(choice, ColorChoice::Never) {
        std::env::set_var("NO_COLOR", "1");
        std::env::remove_var("CLICOLOR_FORCE");
    } else if matches!(choice, ColorChoice::Always) {
        std::env::set_var("CLICOLOR_FORCE", "1");
        std::env::remove_var("NO_COLOR");
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
    pub run: Style,
    pub skip: Style,
    pub fail: Style,
    pub warn: Style,
    pub info: Style,
    pub error_prefix: Style,
    pub warning_prefix: Style,
    pub header: Style,
    pub dim: Style,
    pub path: Style,
    pub count: Style,
    pub severity_error: Style,
    pub severity_warning: Style,
    pub severity_info: Style,
}

impl Palette {
    pub fn new(enabled: bool) -> Self {
        if !enabled {
            return Self::plain();
        }
        let bold = Effects::BOLD;
        let dim = Effects::DIMMED;
        Self {
            ok: Style::new().fg_color(Some(AnsiColor::Green.into())).effects(bold),
            run: Style::new().fg_color(Some(AnsiColor::Cyan.into())).effects(bold),
            skip: Style::new().effects(dim),
            fail: Style::new().fg_color(Some(AnsiColor::Red.into())).effects(bold),
            warn: Style::new().fg_color(Some(AnsiColor::Yellow.into())).effects(bold),
            info: Style::new().fg_color(Some(AnsiColor::Cyan.into())).effects(bold),
            error_prefix: Style::new().fg_color(Some(AnsiColor::Red.into())).effects(bold),
            warning_prefix: Style::new().fg_color(Some(AnsiColor::Yellow.into())).effects(bold),
            header: Style::new().effects(bold),
            dim: Style::new().effects(dim),
            path: Style::new().fg_color(Some(AnsiColor::Cyan.into())),
            count: Style::new().effects(bold),
            severity_error: Style::new().fg_color(Some(AnsiColor::Red.into())).effects(bold),
            severity_warning: Style::new().fg_color(Some(AnsiColor::Yellow.into())).effects(bold),
            severity_info: Style::new().fg_color(Some(AnsiColor::Green.into())).effects(bold),
        }
    }

    pub fn plain() -> Self {
        let s = Style::new();
        Self {
            ok: s,
            run: s,
            skip: s,
            fail: s,
            warn: s,
            info: s,
            error_prefix: s,
            warning_prefix: s,
            header: s,
            dim: s,
            path: s,
            count: s,
            severity_error: s,
            severity_warning: s,
            severity_info: s,
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
        write!(f, "{}{}{}", self.style.render(), self.value, self.style.render_reset())
    }
}
