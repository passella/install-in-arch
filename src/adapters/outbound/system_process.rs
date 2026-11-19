use std::fs;
use std::io::ErrorKind;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::domain::error::DomainError;
use crate::domain::ports::ClockPort;

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

impl SystemPrivilegeDetector {
    pub fn parse_effective_uid_from_proc_status(status_content: &str) -> Option<u32> {
        for line in status_content.lines() {
            if let Some(rest) = line.strip_prefix("Uid:") {
                let mut parts = rest.split_whitespace();
                let _real = parts.next()?;
                let effective = parts.next()?;
                return effective.parse::<u32>().ok();
            }
        }
        None
    }
}

impl PrivilegeDetectorPort for SystemPrivilegeDetector {
    fn is_root(&self) -> bool {
        if let Ok(content) = fs::read_to_string("/proc/self/status")
            && let Some(euid) = Self::parse_effective_uid_from_proc_status(&content)
        {
            return euid == 0;
        }

        Command::new("id")
            .arg("-u")
            .output()
            .ok()
            .filter(|out| out.status.success())
            .and_then(|out| String::from_utf8(out.stdout).ok())
            .is_some_and(|s| s.trim() == "0")
    }
}

pub struct SystemCommandRunner;

impl CommandRunnerPort for SystemCommandRunner {
    fn run_capture(
        &self,
        program: &str,
        args: &[&str],
        env: &[(&str, &str)],
    ) -> Result<CommandOutput, DomainError> {
        let mut cmd = Command::new(program);
        cmd.args(args);
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
        let mut cmd = Command::new(program);
        cmd.args(args)
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
                command: format!("{program} {}", args.join(" ")),
                reason: format!("exited with status code {code}"),
            })
        }
    }
}

fn map_spawn_error(program: &str, args: &[&str], err: &std::io::Error) -> DomainError {
    let command = format!("{program} {}", args.join(" "));
    let reason = if err.kind() == ErrorKind::NotFound {
        format!("executable '{program}' not found in PATH")
    } else {
        err.to_string()
    };
    DomainError::CommandExecutionFailed { command, reason }
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
    use super::SystemPrivilegeDetector;

    #[test]
    fn parses_effective_uid_from_proc_status() {
        let root_status = "Name:\ttest\nUid:\t1000\t0\t0\t0\n";
        assert_eq!(
            SystemPrivilegeDetector::parse_effective_uid_from_proc_status(root_status),
            Some(0)
        );

        let user_status = "Name:\ttest\nUid:\t1000\t1000\t1000\t1000\n";
        assert_eq!(
            SystemPrivilegeDetector::parse_effective_uid_from_proc_status(user_status),
            Some(1000)
        );

        assert_eq!(
            SystemPrivilegeDetector::parse_effective_uid_from_proc_status("NoUidHere\n"),
            None
        );
    }
}
