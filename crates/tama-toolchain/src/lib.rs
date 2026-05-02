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
    #[error("invalid expected {tool} version `{version}`")]
    InvalidExpectedVersion { tool: String, version: String },
    #[error("{0}")]
    ToolVersionMismatch(String),
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
    detect_tool_at(name, None)
}

pub fn detect_tool_at(name: &str, cwd: Option<&Utf8Path>) -> Result<Tool> {
    let path = which::which(name).map_err(|_| Error::MissingTool(name.to_string()))?;
    let path = Utf8PathBuf::from_path_buf(path)
        .map_err(|path| tama_common::Error::NonUtf8Path(path.display().to_string()))?;
    let version = tool_version_at(name, &path, cwd).ok();
    Ok(Tool {
        name: name.to_string(),
        path,
        version,
    })
}

pub fn detect_required_tools() -> DoctorReport {
    detect_required_tools_at(None)
}

pub fn detect_required_tools_at(cwd: Option<&Utf8Path>) -> DoctorReport {
    let mut report = DoctorReport::default();
    for name in ["lean", "lake", "forge", "solc", "git", "tar"] {
        match detect_tool_at(name, cwd) {
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
    tool_version_at(name, path, None)
}

pub fn tool_version_at(name: &str, path: &Utf8Path, cwd: Option<&Utf8Path>) -> Result<String> {
    let mut command = Command::new(path);
    command.arg("--version");
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let output = command.output().map_err(|source| Error::Process {
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

pub fn resolve_solc(expected: &str, root: &Utf8Path) -> Result<Tool> {
    let expected_version = parse_expected_version("solc", expected)?;
    let path = resolve_solc_path(root, expected)?;
    let version_text = tool_version("solc", &path)?;
    let found = parse_solc_version(&version_text)?;
    if found != expected_version {
        return Err(Error::ToolVersionMismatch(format!(
            "solc at {path} has version {found}, expected {expected_version}"
        )));
    }
    Ok(Tool {
        name: "solc".to_string(),
        path,
        version: Some(version_text),
    })
}

pub fn parse_forge_version(output: &str) -> Result<Version> {
    let re = Regex::new(r"forge Version:\s*([0-9]+\.[0-9]+\.[0-9]+)").expect("valid regex");
    parse_version_with(output, "forge", &re)
}

pub fn parse_expected_version(tool: &str, version: &str) -> Result<Version> {
    Version::parse(version.trim_start_matches('v')).map_err(|_| Error::InvalidExpectedVersion {
        tool: tool.to_string(),
        version: version.to_string(),
    })
}

fn resolve_solc_path(root: &Utf8Path, expected: &str) -> Result<Utf8PathBuf> {
    if let Ok(path) = std::env::var("TAMA_SOLC") {
        return Ok(Utf8PathBuf::from(path));
    }
    let project_managed = root.join(".tama/bin/solc");
    if project_managed.is_file() {
        return Ok(project_managed);
    }
    if let Ok(home) = std::env::var("HOME") {
        let home_managed = Utf8PathBuf::from(home)
            .join(".tama")
            .join("solc")
            .join(expected.trim_start_matches('v'))
            .join("solc");
        if home_managed.is_file() {
            return Ok(home_managed);
        }
    }
    let path = which::which("solc").map_err(|_| Error::MissingTool("solc".to_string()))?;
    Utf8PathBuf::from_path_buf(path)
        .map_err(|path| tama_common::Error::NonUtf8Path(path.display().to_string()).into())
}

pub fn parse_lean_version(output: &str) -> Result<Version> {
    let re = Regex::new(r"Lean \(version\s*([0-9]+\.[0-9]+\.[0-9]+)").expect("valid regex");
    parse_version_with(output, "lean", &re)
}

pub fn parse_lake_lean_version(output: &str) -> Result<Version> {
    let re = Regex::new(r"Lean version\s*([0-9]+\.[0-9]+\.[0-9]+)").expect("valid regex");
    parse_version_with(output, "lake", &re)
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
    use std::ffi::{OsStr, OsString};
    use std::sync::{Mutex, OnceLock};

    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    struct EnvVarGuard {
        key: &'static str,
        old: Option<OsString>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: impl AsRef<OsStr>) -> Self {
            let old = std::env::var_os(key);
            std::env::set_var(key, value);
            Self { key, old }
        }

        fn unset(key: &'static str) -> Self {
            let old = std::env::var_os(key);
            std::env::remove_var(key);
            Self { key, old }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            if let Some(value) = &self.old {
                std::env::set_var(self.key, value);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }

    #[test]
    fn parses_solc_version() {
        let version = parse_solc_version("Version: 0.8.33+commit.64118f21").unwrap();
        assert_eq!(version, Version::new(0, 8, 33));
    }

    #[test]
    fn parses_expected_solc_version_with_optional_v_prefix() {
        assert_eq!(
            parse_expected_version("solc", "v0.8.33").unwrap(),
            Version::new(0, 8, 33)
        );
    }

    #[test]
    fn resolve_solc_reports_missing_tool() {
        let _guard = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().join("project")).unwrap();
        let bin = Utf8PathBuf::from_path_buf(dir.path().join("bin")).unwrap();
        let home = Utf8PathBuf::from_path_buf(dir.path().join("home")).unwrap();
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        let _solc_guard = EnvVarGuard::unset("TAMA_SOLC");
        let _path_guard = EnvVarGuard::set("PATH", bin.as_os_str());
        let _home_guard = EnvVarGuard::set("HOME", home.as_os_str());

        let err = resolve_solc("0.8.33", &root).unwrap_err();

        assert!(matches!(err, Error::MissingTool(name) if name == "solc"));
    }

    #[cfg(unix)]
    #[test]
    fn resolve_solc_reports_wrong_version() {
        use std::os::unix::fs::PermissionsExt;

        let _guard = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().join("project")).unwrap();
        let solc = Utf8PathBuf::from_path_buf(dir.path().join("solc")).unwrap();
        std::fs::create_dir_all(&root).unwrap();
        tama_common::write_string(
            &solc,
            "#!/bin/sh\nprintf '%s\\n' 'Version: 0.8.32+commit.test'\n",
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&solc).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&solc, permissions).unwrap();
        let _solc_guard = EnvVarGuard::set("TAMA_SOLC", solc.as_os_str());

        let err = resolve_solc("0.8.33", &root).unwrap_err();

        assert!(matches!(
            err,
            Error::ToolVersionMismatch(message)
                if message.contains("0.8.32") && message.contains("0.8.33")
        ));
    }

    #[test]
    fn parses_forge_version() {
        let version = parse_forge_version("forge Version: 1.6.0-v1.7.0").unwrap();
        assert_eq!(version, Version::new(1, 6, 0));
    }

    #[test]
    fn parses_lake_embedded_lean_version() {
        let version =
            parse_lake_lean_version("Lake version 5.0.0-src+abc123 (Lean version 4.22.0)").unwrap();
        assert_eq!(version, Version::new(4, 22, 0));
    }

    #[cfg(unix)]
    #[test]
    fn passthrough_preserves_args_and_exit_status() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let script = root.join("fake-forge");
        tama_common::write_string(
            &script,
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > args.txt\nexit 7\n",
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&script, permissions).unwrap();

        let args = vec![
            "test".to_string(),
            "--match-test".to_string(),
            "CounterTest".to_string(),
            "-vvv".to_string(),
        ];
        let status = run_passthrough(script.as_str(), &args, &root).unwrap();

        assert_eq!(status.code(), Some(7));
        assert_eq!(
            std::fs::read_to_string(root.join("args.txt")).unwrap(),
            "test\n--match-test\nCounterTest\n-vvv\n"
        );
    }

    #[test]
    fn parse_failure_is_typed() {
        assert!(matches!(
            parse_lean_version("not lean"),
            Err(Error::VersionParse { .. })
        ));
    }
}
