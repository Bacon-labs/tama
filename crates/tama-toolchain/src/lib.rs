use std::process::{Command, ExitStatus, Stdio};

use camino::{Utf8Path, Utf8PathBuf};
use regex::Regex;
use semver::Version;
use serde::{Deserialize, Serialize};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Common(#[from] tama_common::Error),
    #[error("required tool `{0}` was not found on PATH")]
    MissingTool(String),
    #[error("could not parse {tool} version from `{output}`")]
    VersionParse { tool: String, output: String },
    #[error("failed to run {program}: {source}")]
    Process {
        program: String,
        #[source]
        source: std::io::Error,
    },
    #[error("{0}")]
    Failure(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tool {
    pub name: String,
    pub path: Utf8PathBuf,
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolStatus {
    Ok(Tool),
    Missing {
        name: String,
        remediation: String,
    },
    Incompatible {
        name: String,
        found: String,
        expected: String,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DoctorReport {
    pub tools: Vec<ToolStatus>,
    pub lock_current: Option<bool>,
    pub notes: Vec<String>,
}

pub fn detect_tool(name: &str) -> Result<Tool> {
    let path = which::which(name).map_err(|_| Error::MissingTool(name.to_string()))?;
    let path = Utf8PathBuf::from_path_buf(path)
        .map_err(|path| tama_common::Error::NonUtf8Path(path.display().to_string()))?;
    let version = tool_version(name, &path).ok();
    Ok(Tool {
        name: name.to_string(),
        path,
        version,
    })
}

pub fn detect_required_tools() -> DoctorReport {
    let mut report = DoctorReport::default();
    for name in ["lean", "lake", "forge", "solc", "git", "tar"] {
        match detect_tool(name) {
            Ok(tool) => report.tools.push(ToolStatus::Ok(tool)),
            Err(_) => report.tools.push(ToolStatus::Missing {
                name: name.to_string(),
                remediation: remediation(name),
            }),
        }
    }
    report
}

pub fn tool_version(name: &str, path: &Utf8Path) -> Result<String> {
    let output = Command::new(path)
        .arg("--version")
        .output()
        .map_err(|source| Error::Process {
            program: name.to_string(),
            source,
        })?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let text = if stdout.trim().is_empty() {
        stderr
    } else {
        stdout
    };
    Ok(text.trim().to_string())
}

pub fn parse_solc_version(output: &str) -> Result<Version> {
    let re = Regex::new(r"Version:\s*([0-9]+\.[0-9]+\.[0-9]+)").expect("valid regex");
    parse_version_with(output, "solc", &re)
}

pub fn parse_forge_version(output: &str) -> Result<Version> {
    let re = Regex::new(r"forge Version:\s*([0-9]+\.[0-9]+\.[0-9]+)").expect("valid regex");
    parse_version_with(output, "forge", &re)
}

pub fn parse_lean_version(output: &str) -> Result<Version> {
    let re = Regex::new(r"Lean \(version\s*([0-9]+\.[0-9]+\.[0-9]+)").expect("valid regex");
    parse_version_with(output, "lean", &re)
}

fn parse_version_with(output: &str, tool: &str, re: &Regex) -> Result<Version> {
    let raw = re
        .captures(output)
        .and_then(|captures| captures.get(1))
        .ok_or_else(|| Error::VersionParse {
            tool: tool.to_string(),
            output: output.to_string(),
        })?
        .as_str();
    Version::parse(raw).map_err(|_| Error::VersionParse {
        tool: tool.to_string(),
        output: output.to_string(),
    })
}

pub fn run_capture(program: &Utf8Path, args: &[String], cwd: &Utf8Path) -> Result<String> {
    let output = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(|source| Error::Process {
            program: program.to_string(),
            source,
        })?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        let failure = tama_common::ExternalFailure {
            program: program.to_string(),
            args: args.to_vec(),
            status: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        };
        Err(Error::Failure(failure.message()))
    }
}

pub fn run_passthrough(program: &str, args: &[String], cwd: &Utf8Path) -> Result<ExitStatus> {
    Command::new(program)
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|source| Error::Process {
            program: program.to_string(),
            source,
        })
}

fn remediation(name: &str) -> String {
    match name {
        "solc" => "install solc 0.8.33 or set TAMA_SOLC to a matching binary".to_string(),
        "forge" => "install Foundry from https://getfoundry.sh/".to_string(),
        "lean" | "lake" => "install elan and the project lean-toolchain".to_string(),
        _ => format!("install `{name}` and ensure it is on PATH"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_solc_version() {
        let version = parse_solc_version("Version: 0.8.33+commit.64118f21").unwrap();
        assert_eq!(version, Version::new(0, 8, 33));
    }

    #[test]
    fn parses_forge_version() {
        let version = parse_forge_version("forge Version: 1.6.0-v1.7.0").unwrap();
        assert_eq!(version, Version::new(1, 6, 0));
    }

    #[test]
    fn parse_failure_is_typed() {
        assert!(matches!(
            parse_lean_version("not lean"),
            Err(Error::VersionParse { .. })
        ));
    }
}
