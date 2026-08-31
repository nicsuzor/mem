//! Safe, bounded command execution with enforced timeouts.
//!
//! Prevents external child processes and shell invocations from hanging indefinitely.
//! Every command execution runs under an enforced timeout budget (default: 30s).
//! On timeout, the child process is terminated immediately and a typed `CommandError::Timeout`
//! is surfaced to the caller.

use std::ffi::{OsStr, OsString};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// Default timeout for external command execution (30 seconds).
pub const DEFAULT_COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

/// Typed error variants for bounded command execution.
#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    /// The command exceeded its execution deadline and was killed.
    #[error("command '{program}' timed out after {timeout:?}")]
    Timeout {
        program: String,
        timeout: Duration,
    },
    /// An I/O error occurred while spawning or interacting with the process.
    #[error("failed to execute command '{program}': {source}")]
    Io {
        program: String,
        #[source]
        source: io::Error,
    },
}

/// A builder for executing external commands under an enforced timeout.
#[derive(Debug, Clone)]
pub struct BoundedCommand {
    program: String,
    args: Vec<OsString>,
    current_dir: Option<PathBuf>,
    envs: Vec<(OsString, Option<OsString>)>,
    timeout: Duration,
}

impl BoundedCommand {
    /// Create a new `BoundedCommand` for the given executable program.
    pub fn new(program: impl AsRef<OsStr>) -> Self {
        let program_str = program.as_ref().to_string_lossy().to_string();
        Self {
            program: program_str,
            args: Vec::new(),
            current_dir: None,
            envs: Vec::new(),
            timeout: DEFAULT_COMMAND_TIMEOUT,
        }
    }

    /// Append an argument to the command.
    pub fn arg<S: AsRef<OsStr>>(&mut self, arg: S) -> &mut Self {
        self.args.push(arg.as_ref().to_os_string());
        self
    }

    /// Append multiple arguments to the command.
    pub fn args<I, S>(&mut self, args: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        for arg in args {
            self.arg(arg);
        }
        self
    }

    /// Set the working directory for the command.
    pub fn current_dir<P: AsRef<Path>>(&mut self, dir: P) -> &mut Self {
        self.current_dir = Some(dir.as_ref().to_path_buf());
        self
    }

    /// Set an environment variable for the executed command.
    pub fn env<K, V>(&mut self, key: K, val: V) -> &mut Self
    where
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        self.envs
            .push((key.as_ref().to_os_string(), Some(val.as_ref().to_os_string())));
        self
    }

    /// Remove an environment variable for the executed command.
    pub fn env_remove<K: AsRef<OsStr>>(&mut self, key: K) -> &mut Self {
        self.envs.push((key.as_ref().to_os_string(), None));
        self
    }

    /// Set an explicit timeout duration for this command invocation.
    pub fn timeout(&mut self, timeout: Duration) -> &mut Self {
        self.timeout = timeout;
        self
    }

    /// Convert this `BoundedCommand` into a standard `std::process::Command`.
    pub fn to_std_command(&self) -> Command {
        let mut cmd = Command::new(&self.program);
        cmd.args(&self.args);
        if let Some(ref dir) = self.current_dir {
            cmd.current_dir(dir);
        }
        for (key, val) in &self.envs {
            if let Some(v) = val {
                cmd.env(key, v);
            } else {
                cmd.env_remove(key);
            }
        }
        cmd
    }

    /// Run the command and capture its stdout and stderr, bounding execution by timeout.
    pub fn output(&self) -> Result<Output, CommandError> {
        let mut cmd = self.to_std_command();
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let mut child = cmd.spawn().map_err(|source| CommandError::Io {
            program: self.program.clone(),
            source,
        })?;

        let mut stdout_handle = child.stdout.take();
        let mut stderr_handle = child.stderr.take();

        let stdout_thread = thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(ref mut r) = stdout_handle {
                let _ = r.read_to_end(&mut buf);
            }
            buf
        });

        let stderr_thread = thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(ref mut r) = stderr_handle {
                let _ = r.read_to_end(&mut buf);
            }
            buf
        });

        let start = Instant::now();
        let poll_interval = Duration::from_millis(10);

        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    let stdout = stdout_thread.join().unwrap_or_default();
                    let stderr = stderr_thread.join().unwrap_or_default();
                    return Ok(Output {
                        status,
                        stdout,
                        stderr,
                    });
                }
                Ok(None) => {
                    if start.elapsed() >= self.timeout {
                        let _ = child.kill();
                        let _ = child.wait();
                        let _ = stdout_thread.join();
                        let _ = stderr_thread.join();
                        return Err(CommandError::Timeout {
                            program: self.program.clone(),
                            timeout: self.timeout,
                        });
                    }
                    thread::sleep(poll_interval);
                }
                Err(source) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = stdout_thread.join();
                    let _ = stderr_thread.join();
                    return Err(CommandError::Io {
                        program: self.program.clone(),
                        source,
                    });
                }
            }
        }
    }

    /// Run the command and wait for its completion status, bounding execution by timeout.
    pub fn status(&self) -> Result<ExitStatus, CommandError> {
        let mut cmd = self.to_std_command();
        cmd.stdin(Stdio::null());

        let mut child = cmd.spawn().map_err(|source| CommandError::Io {
            program: self.program.clone(),
            source,
        })?;

        let start = Instant::now();
        let poll_interval = Duration::from_millis(10);

        loop {
            match child.try_wait() {
                Ok(Some(status)) => return Ok(status),
                Ok(None) => {
                    if start.elapsed() >= self.timeout {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(CommandError::Timeout {
                            program: self.program.clone(),
                            timeout: self.timeout,
                        });
                    }
                    thread::sleep(poll_interval);
                }
                Err(source) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(CommandError::Io {
                        program: self.program.clone(),
                        source,
                    });
                }
            }
        }
    }
}

/// Execute a `std::process::Command` under a specified timeout duration.
pub fn run_command_with_timeout(
    mut cmd: Command,
    program_name: &str,
    timeout: Duration,
) -> Result<Output, CommandError> {
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd.spawn().map_err(|source| CommandError::Io {
        program: program_name.to_string(),
        source,
    })?;

    let mut stdout_handle = child.stdout.take();
    let mut stderr_handle = child.stderr.take();

    let stdout_thread = thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(ref mut r) = stdout_handle {
            let _ = r.read_to_end(&mut buf);
        }
        buf
    });

    let stderr_thread = thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(ref mut r) = stderr_handle {
            let _ = r.read_to_end(&mut buf);
        }
        buf
    });

    let start = Instant::now();
    let poll_interval = Duration::from_millis(10);

    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let stdout = stdout_thread.join().unwrap_or_default();
                let stderr = stderr_thread.join().unwrap_or_default();
                return Ok(Output {
                    status,
                    stdout,
                    stderr,
                });
            }
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = stdout_thread.join();
                    let _ = stderr_thread.join();
                    return Err(CommandError::Timeout {
                        program: program_name.to_string(),
                        timeout,
                    });
                }
                thread::sleep(poll_interval);
            }
            Err(source) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_thread.join();
                let _ = stderr_thread.join();
                return Err(CommandError::Io {
                    program: program_name.to_string(),
                    source,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fast_command_succeeds() {
        let out = BoundedCommand::new("echo")
            .arg("hello world")
            .output()
            .expect("echo should execute and succeed");
        assert!(out.status.success());
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert_eq!(stdout.trim(), "hello world");
    }

    #[test]
    fn test_slow_command_times_out() {
        let start = Instant::now();
        let result = BoundedCommand::new("sleep")
            .arg("10")
            .timeout(Duration::from_millis(100))
            .output();

        let elapsed = start.elapsed();
        assert!(
            result.is_err(),
            "sleep 10 with 100ms timeout should return error"
        );
        match result.unwrap_err() {
            CommandError::Timeout { program, timeout } => {
                assert_eq!(program, "sleep");
                assert_eq!(timeout, Duration::from_millis(100));
            }
            err => panic!("expected Timeout error, got: {:?}", err),
        }
        assert!(
            elapsed < Duration::from_secs(3),
            "command should terminate rapidly on timeout, took {:?}",
            elapsed
        );
    }

    #[test]
    fn test_slow_status_times_out() {
        let start = Instant::now();
        let result = BoundedCommand::new("sleep")
            .arg("10")
            .timeout(Duration::from_millis(100))
            .status();

        let elapsed = start.elapsed();
        assert!(
            result.is_err(),
            "sleep 10 with 100ms timeout on status() should return error"
        );
        match result.unwrap_err() {
            CommandError::Timeout { program, timeout } => {
                assert_eq!(program, "sleep");
                assert_eq!(timeout, Duration::from_millis(100));
            }
            err => panic!("expected Timeout error, got: {:?}", err),
        }
        assert!(
            elapsed < Duration::from_secs(3),
            "command status should terminate rapidly on timeout, took {:?}",
            elapsed
        );
    }

    #[test]
    fn test_nonexistent_command_returns_io_error() {
        let result = BoundedCommand::new("non_existent_binary_12345xyz").output();
        assert!(result.is_err());
        match result.unwrap_err() {
            CommandError::Io { program, .. } => {
                assert_eq!(program, "non_existent_binary_12345xyz");
            }
            err => panic!("expected Io error, got: {:?}", err),
        }
    }
}
