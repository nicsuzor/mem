use mem::cmd::{run_command_with_timeout, BoundedCommand, CommandError, DEFAULT_COMMAND_TIMEOUT};
use std::process::Command;
use std::time::{Duration, Instant};

#[test]
fn test_default_timeout_is_30_seconds() {
    assert_eq!(DEFAULT_COMMAND_TIMEOUT, Duration::from_secs(30));
}

#[test]
fn test_bounded_command_fast_command_succeeds() {
    let out = BoundedCommand::new("echo")
        .arg("hello-mem-timeout-test")
        .output()
        .expect("echo should execute successfully");

    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(stdout.trim(), "hello-mem-timeout-test");
}

#[test]
fn test_bounded_command_slow_command_cuts_off_on_timeout() {
    let start = Instant::now();
    let timeout = Duration::from_millis(150);
    let result = BoundedCommand::new("sleep")
        .arg("10")
        .timeout(timeout)
        .output();

    let elapsed = start.elapsed();
    assert!(result.is_err(), "sleep 10 must time out");

    match result.unwrap_err() {
        CommandError::Timeout { program, timeout: err_timeout } => {
            assert_eq!(program, "sleep");
            assert_eq!(err_timeout, timeout);
        }
        other => panic!("expected CommandError::Timeout, got {:?}", other),
    }

    assert!(
        elapsed < Duration::from_secs(3),
        "command should terminate immediately on timeout, took {:?}",
        elapsed
    );
}

#[test]
fn test_bounded_command_status_slow_command_cuts_off() {
    let start = Instant::now();
    let timeout = Duration::from_millis(150);
    let result = BoundedCommand::new("sleep")
        .arg("10")
        .timeout(timeout)
        .status();

    let elapsed = start.elapsed();
    assert!(result.is_err(), "sleep 10 on status() must time out");

    match result.unwrap_err() {
        CommandError::Timeout { program, timeout: err_timeout } => {
            assert_eq!(program, "sleep");
            assert_eq!(err_timeout, timeout);
        }
        other => panic!("expected CommandError::Timeout, got {:?}", other),
    }

    assert!(
        elapsed < Duration::from_secs(3),
        "command status should terminate immediately on timeout, took {:?}",
        elapsed
    );
}

#[test]
fn test_run_command_with_timeout_helper_cuts_off() {
    let mut cmd = Command::new("sleep");
    cmd.arg("10");

    let start = Instant::now();
    let timeout = Duration::from_millis(150);
    let result = run_command_with_timeout(cmd, "sleep", timeout);
    let elapsed = start.elapsed();

    assert!(result.is_err(), "run_command_with_timeout must time out");
    match result.unwrap_err() {
        CommandError::Timeout { program, timeout: err_timeout } => {
            assert_eq!(program, "sleep");
            assert_eq!(err_timeout, timeout);
        }
        other => panic!("expected CommandError::Timeout, got {:?}", other),
    }

    assert!(
        elapsed < Duration::from_secs(3),
        "run_command_with_timeout should terminate immediately on timeout, took {:?}",
        elapsed
    );
}

#[test]
fn test_bounded_command_large_output_streaming() {
    // Generate 100,000 characters of stdout to ensure reader thread doesn't deadlock the pipe
    let out = BoundedCommand::new("sh")
        .args(["-c", "printf '%0.sA' $(seq 1 100000)"])
        .timeout(Duration::from_secs(5))
        .output()
        .expect("large output command should succeed without deadlock");

    assert!(out.status.success());
    assert_eq!(out.stdout.len(), 100000);
}
