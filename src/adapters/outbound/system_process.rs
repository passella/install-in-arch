use std::fs;
use std::io::ErrorKind;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::domain::error::DomainError;
use crate::domain::ports::ClockPort;
use crate::domain::sanitizer::sanitize_for_error;

const SAFE_SYSTEM_PATH: &str = "/usr/bin:/usr/sbin";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    pub status_code: Option<i32>,
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

pub trait PrivilegeDetectorPort {
    fn is_root(&self) -> bool;
}

pub trait CommandRunnerPort {
    fn run_capture(
        &self,
        program: &str,
        args: &[&str],
        env: &[(&str, &str)],
    ) -> Result<CommandOutput, DomainError>;

    fn run_interactive(
        &self,
        program: &str,
        args: &[&str],
        env: &[(&str, &str)],
    ) -> Result<i32, DomainError>;
}

pub struct SystemPrivilegeDetector;

impl PrivilegeDetectorPort for SystemPrivilegeDetector {
    fn is_root(&self) -> bool {
        rustix::process::geteuid().is_root()
    }
}

pub struct SystemCommandRunner;

impl SystemCommandRunner {
    fn verify_trusted_executable(program: &str, args: &[&str]) -> Result<(), DomainError> {
        let command = format!("{program} {}", args.join(" "));
        let path = Path::new(program);
        if !path.is_absolute() {
            return Err(DomainError::CommandExecutionFailed {
                command: sanitize_for_error(&command),
                reason: "refusing non-absolute executable path".to_string(),
            });
        }
        let meta = fs::metadata(path).map_err(|err| map_spawn_error(program, args, &err))?;
        if !meta.is_file() {
            return Err(DomainError::CommandExecutionFailed {
                command: sanitize_for_error(&command),
                reason: "target executable is not a regular file".to_string(),
            });
        }
        if meta.uid() != 0 {
            return Err(DomainError::CommandExecutionFailed {
                command: sanitize_for_error(&command),
                reason: format!(
                    "executable '{}' is not owned by root",
                    sanitize_for_error(program)
                ),
            });
        }
        if (meta.mode() & 0o022) != 0 {
            return Err(DomainError::CommandExecutionFailed {
                command: sanitize_for_error(&command),
                reason: format!(
                    "executable '{}' has unsafe group/world write permissions",
                    sanitize_for_error(program)
                ),
            });
        }
        Ok(())
    }
}

impl CommandRunnerPort for SystemCommandRunner {
    fn run_capture(
        &self,
        program: &str,
        args: &[&str],
        env: &[(&str, &str)],
    ) -> Result<CommandOutput, DomainError> {
        Self::verify_trusted_executable(program, args)?;
        let mut cmd = Command::new(program);
        cmd.args(args).env("PATH", SAFE_SYSTEM_PATH);
        for (k, v) in env {
            cmd.env(k, v);
        }
        let output = cmd
            .output()
            .map_err(|err| map_spawn_error(program, args, &err))?;

        Ok(CommandOutput {
            status_code: output.status.code(),
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }

    fn run_interactive(
        &self,
        program: &str,
        args: &[&str],
        env: &[(&str, &str)],
    ) -> Result<i32, DomainError> {
        Self::verify_trusted_executable(program, args)?;
        let mut cmd = Command::new(program);
        cmd.args(args)
            .env("PATH", SAFE_SYSTEM_PATH)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
        for (k, v) in env {
            cmd.env(k, v);
        }

        let status = cmd
            .status()
            .map_err(|err| map_spawn_error(program, args, &err))?;

        if status.success() {
            Ok(status.code().unwrap_or(0))
        } else {
            let code = status.code().unwrap_or(1);
            Err(DomainError::CommandExecutionFailed {
                command: sanitize_for_error(&format!("{program} {}", args.join(" "))),
                reason: format!("exited with status code {code}"),
            })
        }
    }
}

fn map_spawn_error(program: &str, args: &[&str], err: &std::io::Error) -> DomainError {
    let command = sanitize_for_error(&format!("{program} {}", args.join(" ")));
    if err.kind() == ErrorKind::NotFound {
        DomainError::ExecutableNotFound {
            command,
            executable: sanitize_for_error(program),
        }
    } else {
        DomainError::CommandExecutionFailed {
            command,
            reason: sanitize_for_error(&err.to_string()),
        }
    }
}

pub struct SystemClock;

impl ClockPort for SystemClock {
    fn unix_timestamp_secs(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::{CommandRunnerPort, SystemCommandRunner, SystemPrivilegeDetector};
    use crate::adapters::outbound::system_process::PrivilegeDetectorPort;
    use crate::domain::error::DomainError;
    use crate::domain::ports::ClockPort;
    use crate::domain::ports::TempStorageGuard;

    #[test]
    fn rejects_relative_and_missing_executable_paths_without_spawning() {
        let runner = SystemCommandRunner;
        let err = runner.run_capture("pacman", &["-V"], &[]);
        assert!(matches!(
            err,
            Err(DomainError::CommandExecutionFailed { .. })
        ));

        let missing_err =
            runner.run_capture("/nonexistent/install-in-arch-missing-pacman", &["-Qq"], &[]);
        assert!(matches!(
            missing_err,
            Err(DomainError::ExecutableNotFound { .. })
        ));

        let dir_err = runner.run_capture("/tmp", &["-V"], &[]);
        assert!(matches!(
            dir_err,
            Err(DomainError::CommandExecutionFailed { .. })
        ));

        let detector = SystemPrivilegeDetector;
        let _is_root = detector.is_root();
        let clock = super::SystemClock;
        assert!(clock.unix_timestamp_secs() > 0);
        let _: Option<Box<dyn TempStorageGuard>> = None;
    }
}
