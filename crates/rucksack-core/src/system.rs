use anyhow::{anyhow, Context, Result};
use std::env;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

const DEFAULT_COMMAND_TIMEOUT: Duration = Duration::from_secs(60);
const DEFAULT_COMMAND_OUTPUT_LIMIT: usize = 1024 * 1024;

#[derive(Debug, Clone)]
pub struct CommandResult {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl CommandResult {
    pub fn success(&self) -> bool {
        self.code == 0
    }

    pub fn combined_trimmed(&self) -> String {
        let stdout = self.stdout.trim();
        let stderr = self.stderr.trim();
        match (stdout.is_empty(), stderr.is_empty()) {
            (false, true) => stdout.to_owned(),
            (true, false) => stderr.to_owned(),
            (false, false) => format!("{stdout}\n{stderr}"),
            (true, true) => String::new(),
        }
    }
}

/// Turn a non-zero exit into an error that names the command and what it said.
pub fn require_success(name: &str, result: &CommandResult) -> Result<()> {
    if result.success() {
        Ok(())
    } else {
        Err(anyhow!(
            "{name} failed with exit code {}: {}",
            result.code,
            result.combined_trimmed()
        ))
    }
}

pub fn run(path: impl AsRef<Path>, args: &[&str]) -> Result<CommandResult> {
    let path = path.as_ref();
    let mut command = Command::new(path);
    command.args(args);
    run_prepared_bounded(
        &mut command,
        command_description(path, args),
        DEFAULT_COMMAND_TIMEOUT,
        DEFAULT_COMMAND_OUTPUT_LIMIT,
    )
}

pub fn run_owned(path: impl AsRef<Path>, args: &[String]) -> Result<CommandResult> {
    let path = path.as_ref();
    let mut command = Command::new(path);
    command.args(args);
    run_prepared_bounded(
        &mut command,
        command_description_owned(path, args),
        DEFAULT_COMMAND_TIMEOUT,
        DEFAULT_COMMAND_OUTPUT_LIMIT,
    )
}

pub fn run_bounded_cleared(
    path: impl AsRef<Path>,
    args: &[&str],
    timeout: Duration,
    max_output_bytes_per_stream: usize,
) -> Result<CommandResult> {
    let path = path.as_ref();
    let mut command = Command::new(path);
    command.args(args).env_clear();
    run_prepared_bounded(
        &mut command,
        command_description(path, args),
        timeout,
        max_output_bytes_per_stream,
    )
}

#[derive(Debug)]
struct CapturedStream {
    bytes: Vec<u8>,
    exceeded_limit: bool,
}

fn run_prepared_bounded(
    command: &mut Command,
    description: String,
    timeout: Duration,
    max_output_bytes_per_stream: usize,
) -> Result<CommandResult> {
    let deadline = Instant::now()
        .checked_add(timeout)
        .context("Bounded command timeout was too large")?;
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("Could not start {description}"))?;
    let stdout = child
        .stdout
        .take()
        .context("Bounded command stdout pipe was unavailable")?;
    let stderr = child
        .stderr
        .take()
        .context("Bounded command stderr pipe was unavailable")?;
    let stdout_reader =
        spawn_reader(stdout, max_output_bytes_per_stream, "stdout", &description)
            .or_else(|error| terminate_after_setup_error(&mut child, &description, error))?;
    let stderr_reader =
        spawn_reader(stderr, max_output_bytes_per_stream, "stderr", &description)
            .or_else(|error| terminate_after_setup_error(&mut child, &description, error))?;

    let (status, timed_out) = loop {
        match child.try_wait() {
            Ok(Some(status)) => break (status, false),
            Ok(None) => {}
            Err(error) => {
                return terminate_after_setup_error(
                    &mut child,
                    &description,
                    anyhow::Error::new(error).context(format!("Could not inspect {description}")),
                );
            }
        }
        if Instant::now() >= deadline {
            match child.kill() {
                Ok(()) => {
                    let status = child
                        .wait()
                        .with_context(|| format!("Could not reap timed-out {description}"))?;
                    break (status, true);
                }
                Err(kill_error) => {
                    if let Some(status) = child
                        .try_wait()
                        .with_context(|| format!("Could not inspect {description} after timeout"))?
                    {
                        break (status, false);
                    }
                    return Err(kill_error)
                        .with_context(|| format!("Could not terminate timed-out {description}"));
                }
            }
        }
        thread::sleep(Duration::from_millis(10));
    };

    if timed_out {
        anyhow::bail!(
            "{description} timed out after {} ms and was terminated",
            timeout.as_millis()
        );
    }

    let stdout = receive_reader(stdout_reader, deadline, "stdout", &description)?;
    let stderr = receive_reader(stderr_reader, deadline, "stderr", &description)?;
    let stdout_text = String::from_utf8_lossy(&stdout.bytes).into_owned();
    let stderr_text = String::from_utf8_lossy(&stderr.bytes).into_owned();

    if stdout.exceeded_limit || stderr.exceeded_limit {
        anyhow::bail!(
            "{description} output exceeded the {}-byte per-stream limit (stdout_exceeded={}, stderr_exceeded={})",
            max_output_bytes_per_stream,
            stdout.exceeded_limit,
            stderr.exceeded_limit
        );
    }

    Ok(CommandResult {
        code: status.code().unwrap_or(-1),
        stdout: stdout_text,
        stderr: stderr_text,
    })
}

fn read_capped(mut reader: impl Read, max_bytes: usize) -> io::Result<CapturedStream> {
    let mut captured = Vec::with_capacity(max_bytes.min(8 * 1024));
    let mut exceeded_limit = false;
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        let remaining = max_bytes.saturating_sub(captured.len());
        let retained = remaining.min(count);
        captured.extend_from_slice(&buffer[..retained]);
        exceeded_limit |= retained < count;
    }
    Ok(CapturedStream {
        bytes: captured,
        exceeded_limit,
    })
}

fn spawn_reader(
    reader: impl Read + Send + 'static,
    max_bytes: usize,
    stream_name: &str,
    description: &str,
) -> Result<Receiver<io::Result<CapturedStream>>> {
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::Builder::new()
        .name(format!("rucksack-command-{stream_name}"))
        .spawn(move || {
            let _ = sender.send(read_capped(reader, max_bytes));
        })
        .with_context(|| format!("Could not start {stream_name} reader for {description}"))?;
    Ok(receiver)
}

fn receive_reader(
    reader: Receiver<io::Result<CapturedStream>>,
    deadline: Instant,
    stream_name: &str,
    description: &str,
) -> Result<CapturedStream> {
    match reader.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        Ok(result) => {
            result.with_context(|| format!("Could not read {stream_name} from {description}"))
        }
        Err(RecvTimeoutError::Timeout) => anyhow::bail!(
            "{description} exited but its {stream_name} pipe did not close before the command timeout"
        ),
        Err(RecvTimeoutError::Disconnected) => {
            Err(anyhow!("{description} {stream_name} reader stopped"))
        }
    }
}

fn terminate_after_setup_error<T>(
    child: &mut Child,
    description: &str,
    error: anyhow::Error,
) -> Result<T> {
    let kill_result = child.kill();
    let wait_result = child.wait();
    match (kill_result, wait_result) {
        (Ok(()), Ok(_)) => Err(error),
        (kill, wait) => Err(anyhow!(
            "{error:#}; cleanup for {description} failed (kill={kill:?}, wait={wait:?})"
        )),
    }
}

fn command_description(path: &Path, args: &[&str]) -> String {
    if args.is_empty() {
        path.display().to_string()
    } else {
        format!("{} {}", path.display(), args.join(" "))
    }
}

fn command_description_owned(path: &Path, args: &[String]) -> String {
    if args.is_empty() {
        path.display().to_string()
    } else {
        format!("{} {}", path.display(), args.join(" "))
    }
}

pub fn which(name: &str) -> Option<PathBuf> {
    let candidate = Path::new(name);
    if candidate.components().count() > 1 && candidate.is_file() {
        return Some(candidate.to_path_buf());
    }
    let path = env::var_os("PATH")?;
    for directory in env::split_paths(&path) {
        let candidate = directory.join(name);
        if candidate.is_file() && is_executable(&candidate) {
            return Some(candidate);
        }
    }
    None
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .map(|meta| meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

#[derive(Debug, Clone)]
pub struct ProcessInfo {
    pub pid: u32,
    pub command: String,
    pub arguments: String,
}

pub fn processes() -> Result<Vec<ProcessInfo>> {
    let result = run("/bin/ps", &["-axo", "pid=,comm=,args="])?;
    if !result.success() {
        anyhow::bail!("ps failed: {}", result.combined_trimmed());
    }
    let mut processes = Vec::new();
    for line in result.stdout.lines() {
        let trimmed = line.trim_start();
        let Some((pid_text, rest)) = trimmed.split_once(char::is_whitespace) else {
            continue;
        };
        let Ok(pid) = pid_text.parse::<u32>() else {
            continue;
        };
        let rest = rest.trim_start();
        let (command, arguments) = rest
            .split_once(char::is_whitespace)
            .map(|(command, args)| (command.to_owned(), args.trim().to_owned()))
            .unwrap_or_else(|| (rest.to_owned(), String::new()));
        processes.push(ProcessInfo {
            pid,
            command,
            arguments,
        });
    }
    Ok(processes)
}

pub fn current_uid() -> u32 {
    #[cfg(unix)]
    unsafe {
        libc::geteuid()
    }

    #[cfg(not(unix))]
    {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn bounded_command_captures_successful_output() {
        let result = run_bounded_cleared(
            "/usr/bin/printf",
            &["%s", "ready"],
            Duration::from_secs(2),
            64,
        )
        .unwrap();

        assert!(result.success());
        assert_eq!(result.stdout, "ready");
    }

    #[cfg(unix)]
    #[test]
    fn bounded_command_rejects_excess_output() {
        let output = "x".repeat(4 * 1024);
        let error = run_bounded_cleared(
            "/usr/bin/printf",
            &["%s", output.as_str()],
            Duration::from_secs(2),
            16,
        )
        .unwrap_err();

        assert!(error.to_string().contains("output exceeded"));
    }

    #[cfg(unix)]
    #[test]
    fn bounded_command_terminates_on_timeout() {
        let error =
            run_bounded_cleared("/bin/sleep", &["2"], Duration::from_millis(50), 64).unwrap_err();

        assert!(error.to_string().contains("timed out"));
    }

    /// The child exits at once, but the grandchild it backgrounds holds the inherited stdout and
    /// stderr pipes far longer than the deadline. `run_bounded_cleared` has to give up on those
    /// pipes at the deadline instead of blocking until the grandchild is gone.
    ///
    /// Both timings carry deliberate slack. A one-second deadline is around twenty times the worst
    /// child-exit latency measured on a loaded Mac, so `sh` is reaped well before the deadline and
    /// the error names the pipe rather than a timeout. The ten-second bound leaves room for a
    /// stalled machine while staying a third of the grandchild's lifetime, so a version that
    /// waited on the pipe would miss it by a wide margin.
    #[cfg(unix)]
    #[test]
    fn bounded_command_does_not_wait_for_inherited_output_pipe() {
        let started = Instant::now();
        let error = run_bounded_cleared(
            "/bin/sh",
            &["-c", "/bin/sleep 30 &"],
            Duration::from_secs(1),
            64,
        )
        .unwrap_err();

        assert!(started.elapsed() < Duration::from_secs(10));
        assert!(error.to_string().contains("pipe did not close"));
    }
}
