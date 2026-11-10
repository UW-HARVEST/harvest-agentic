use crate::error::HarvestResult;
use std::io::Write;
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::time::Duration;

use crate::harness::TestCase;

/// Runs a binary with test case inputs and enforces a timeout
pub fn run_binary_with_timeout(
    binary_path: &Path,
    test_case: &TestCase,
    cwd: &Path,
    timeout: Duration,
) -> HarvestResult<Output> {
    let mut child = spawn_process_with_args(binary_path, test_case, cwd)?;
    write_stdin_to_process(&mut child, test_case.stdin.as_deref(), binary_path)?;
    wait_for_process_with_timeout(child, timeout)
}

/// Spawns a process in `cwd` with the test case's arguments and environment
fn spawn_process_with_args(
    binary_path: &Path,
    test_case: &TestCase,
    cwd: &Path,
) -> HarvestResult<Child> {
    let mut cmd = Command::new(binary_path);
    cmd.args(&test_case.argv[..])
        .envs(&test_case.env)
        .current_dir(cwd);

    // Configure stdio pipes
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // Start the process
    cmd.spawn().map_err(|e| -> Box<dyn std::error::Error> {
        format!("Failed to spawn process: {}: {}", binary_path.display(), e).into()
    })
}

/// Writes stdin data to the child process
fn write_stdin_to_process(
    child: &mut Child,
    stdin_data: Option<&str>,
    binary_path: &Path,
) -> HarvestResult<()> {
    // Early return if no stdin data - just close the pipe
    let Some(data) = stdin_data else {
        if let Some(stdin) = child.stdin.take() {
            drop(stdin);
        }
        return Ok(());
    };

    // Get stdin handle or return error
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| -> Box<dyn std::error::Error> {
            format!(
                "Failed to open stdin for process: {}",
                binary_path.display()
            )
            .into()
        })?;

    // Write data to stdin
    stdin
        .write_all(data.as_bytes())
        .map_err(|e| -> Box<dyn std::error::Error> {
            format!("Failed to write to stdin: {}", e).into()
        })?;

    // stdin is automatically closed when dropped
    Ok(())
}

/// Waits for the process to complete within the timeout, handling cleanup on timeout or error
fn wait_for_process_with_timeout(mut child: Child, timeout: Duration) -> HarvestResult<Output> {
    use wait_timeout::ChildExt;

    match child.wait_timeout(timeout) {
        Ok(Some(_status)) => {
            // Process completed within timeout, get the output
            child
                .wait_with_output()
                .map_err(|e| -> Box<dyn std::error::Error> {
                    format!("Failed to read process output: {}", e).into()
                })
        }
        Ok(None) => {
            // Timeout occurred - clean up and return error
            cleanup_process(&mut child);
            Err(format!("Process timed out after {} seconds", timeout.as_secs()).into())
        }
        Err(e) => {
            // Error waiting for process - clean up and return error
            cleanup_process(&mut child);
            Err(format!("Error waiting for process: {}", e).into())
        }
    }
}

/// Kills and waits for a process, ignoring any errors during cleanup
fn cleanup_process(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}
