use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};
use tokio_util::sync::CancellationToken;

const OUTPUT_LIMIT: usize = 8 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Recipe {
    pub executable: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub cwd_relative: String,
    pub timeout_seconds: u64,
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
    #[serde(default)]
    pub environment_ref: String,
    #[serde(default)]
    pub evidence_parser_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CommandOutcome {
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub cancelled: bool,
    pub stdout: String,
    pub stderr: String,
    pub output_truncated: bool,
    pub error: Option<String>,
}

impl CommandOutcome {
    pub fn disposition(&self) -> &'static str {
        if self.timed_out || self.cancelled || self.error.is_some() || self.exit_code.is_none() {
            "Inconclusive"
        } else if self.exit_code == Some(0) {
            "Passed"
        } else {
            "Failed"
        }
    }
}

pub(super) fn command(executable: &str, cwd: &Path) -> Command {
    let mut command = Command::new(executable);
    command.env_clear();
    // Deliberate allowlist: never pass the parent invocation's bearer or provider tokens.
    for name in [
        "SystemRoot",
        "WINDIR",
        "COMSPEC",
        "PATH",
        "PATHEXT",
        "USERPROFILE",
        "APPDATA",
        "LOCALAPPDATA",
        "PROGRAMFILES",
        "PROGRAMFILES(X86)",
        "HOME",
    ] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    command
}

pub(super) async fn drain(reader: impl AsyncRead + Unpin) -> Result<(Vec<u8>, bool)> {
    let mut reader = reader;
    let mut output = Vec::new();
    let mut truncated = false;
    let mut buffer = [0; 8192];
    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            return Ok((output, truncated));
        }
        let keep = read.min(OUTPUT_LIMIT.saturating_sub(output.len()));
        output.extend_from_slice(&buffer[..keep]);
        truncated |= keep != read;
    }
}

pub(super) async fn run(
    recipe: &Recipe,
    workspace: &Path,
    cancel: CancellationToken,
) -> Result<CommandOutcome> {
    if cancel.is_cancelled() {
        return Ok(CommandOutcome {
            exit_code: None,
            timed_out: false,
            cancelled: true,
            stdout: String::new(),
            stderr: String::new(),
            output_truncated: false,
            error: Some("command cancelled before startup".into()),
        });
    }
    if !(1..=3600).contains(&recipe.timeout_seconds) {
        bail!("command timeout must be within 1..3600 seconds");
    }
    let cwd = super::artifacts::resolve(workspace, &recipe.cwd_relative)?;
    let mut process = command(&recipe.executable, &cwd);
    process.args(&recipe.args).envs(&recipe.environment);
    let mut child = match process.spawn() {
        Ok(child) => child,
        Err(error) => {
            return Ok(CommandOutcome {
                exit_code: None,
                timed_out: false,
                cancelled: false,
                stdout: String::new(),
                stderr: String::new(),
                output_truncated: false,
                error: Some(error.to_string()),
            });
        }
    };
    let job = ProcessJob::attach(&child)?;
    let stdout = tokio::spawn(drain(
        child.stdout.take().context("missing command stdout")?,
    ));
    let stderr = tokio::spawn(drain(
        child.stderr.take().context("missing command stderr")?,
    ));
    let mut timed_out = false;
    let mut cancelled = false;
    let status = tokio::select! {
        result = child.wait() => result,
        _ = tokio::time::sleep(Duration::from_secs(recipe.timeout_seconds)) => {
            timed_out = true;
            job.terminate()?;
            child.wait().await
        }
        _ = cancel.cancelled() => {
            cancelled = true;
            job.terminate()?;
            child.wait().await
        }
    };
    // A recipe may leave descendants behind after its foreground process exits.
    job.terminate()?;
    job.settle().await?;
    let (out, out_truncated) = tokio::time::timeout(Duration::from_secs(5), stdout).await???;
    let (err, err_truncated) = tokio::time::timeout(Duration::from_secs(5), stderr).await???;
    Ok(CommandOutcome {
        exit_code: status.as_ref().ok().and_then(|status| status.code()),
        timed_out,
        cancelled,
        stdout: String::from_utf8_lossy(&out).into_owned(),
        stderr: String::from_utf8_lossy(&err).into_owned(),
        output_truncated: out_truncated || err_truncated,
        error: status.err().map(|error| error.to_string()),
    })
}

#[cfg(windows)]
pub(super) struct ProcessJob(std::os::windows::io::OwnedHandle);

#[cfg(windows)]
impl ProcessJob {
    pub fn attach(child: &Child) -> Result<Self> {
        use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
        use windows_sys::Win32::System::JobObjects::*;
        // The newly spawned, runtime-owned child is the only process assigned here.
        // The RAII handle is never inherited and kills only this invocation's tree.
        unsafe {
            let raw = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if raw.is_null() {
                return Err(std::io::Error::last_os_error()).context("create invocation job");
            }

            let job = OwnedHandle::from_raw_handle(raw);
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                job.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                std::ptr::addr_of!(limits).cast(),
                std::mem::size_of_val(&limits) as u32,
            ) == 0
                || AssignProcessToJobObject(
                    job.as_raw_handle(),
                    child.raw_handle().context("child process handle missing")?,
                ) == 0
            {
                return Err(std::io::Error::last_os_error()).context("bind invocation job");
            }
            Ok(Self(job))
        }
    }

    pub fn terminate(&self) -> Result<()> {
        use std::os::windows::io::AsRawHandle;
        // The handle owns only this invocation; no process-name or global termination.
        if unsafe {
            windows_sys::Win32::System::JobObjects::TerminateJobObject(self.0.as_raw_handle(), 1)
        } == 0
        {
            return Err(std::io::Error::last_os_error()).context("terminate invocation job");
        }
        Ok(())
    }

    pub async fn settle(&self) -> Result<()> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::JobObjects::*;
        for _ in 0..100 {
            let mut info = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
            // The buffer type and size match the queried job information class.
            if unsafe {
                QueryInformationJobObject(
                    self.0.as_raw_handle(),
                    JobObjectBasicAccountingInformation,
                    std::ptr::addr_of_mut!(info).cast(),
                    std::mem::size_of_val(&info) as u32,
                    std::ptr::null_mut(),
                )
            } == 0
            {
                return Err(std::io::Error::last_os_error()).context("query invocation settlement");
            }
            if info.ActiveProcesses == 0 {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        bail!("invocation descendants did not settle within five seconds")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recipe(script: &str, timeout_seconds: u64) -> Recipe {
        Recipe {
            executable: "powershell.exe".into(),
            args: vec![
                "-NoLogo".into(),
                "-NoProfile".into(),
                "-NonInteractive".into(),
                "-Command".into(),
                script.into(),
            ],
            cwd_relative: String::new(),
            timeout_seconds,
            environment: BTreeMap::new(),
            environment_ref: "clean".into(),
            evidence_parser_id: "process-exit-v1".into(),
        }
    }

    #[tokio::test]
    async fn native_check_records_real_pass_failure_and_streams() {
        let cwd = std::env::current_dir().unwrap();
        let passed = run(&recipe("[Console]::Out.Write('actual stdout'); [Console]::Error.Write('actual stderr'); exit 0", 30), &cwd, CancellationToken::new()).await.unwrap();
        assert_eq!(passed.disposition(), "Passed");
        assert_eq!(passed.exit_code, Some(0));
        assert_eq!(passed.stdout, "actual stdout");
        assert_eq!(passed.stderr, "actual stderr");
        let failed = run(
            &recipe("[Console]::Error.Write('assertion failed'); exit 7", 30),
            &cwd,
            CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(failed.disposition(), "Failed");
        assert_eq!(failed.exit_code, Some(7));
        assert!(failed.stderr.contains("assertion failed"));
    }

    #[tokio::test]
    async fn native_timeout_is_inconclusive_not_failed() {
        let cwd = std::env::current_dir().unwrap();
        let result = run(
            &recipe("Start-Sleep -Seconds 30", 1),
            &cwd,
            CancellationToken::new(),
        )
        .await
        .unwrap();
        assert!(result.timed_out);
        assert_eq!(result.disposition(), "Inconclusive");
    }

    #[tokio::test]
    async fn native_start_failure_preserves_actual_error() {
        let cwd = std::env::current_dir().unwrap();
        let mut recipe = recipe("", 1);
        recipe.executable = format!("missing-agent-center-command-{}.exe", uuid::Uuid::new_v4());
        let result = run(&recipe, &cwd, CancellationToken::new()).await.unwrap();
        assert_eq!(result.disposition(), "Inconclusive");
        assert!(result.error.is_some());
        assert_eq!(result.exit_code, None);
    }

    #[tokio::test]
    async fn cancellation_before_start_does_not_launch_a_process() {
        let mut recipe = recipe("", 30);
        recipe.executable = "missing-command-must-not-be-launched.exe".into();
        let cancel = CancellationToken::new();
        cancel.cancel();
        let result = run(&recipe, &std::env::current_dir().unwrap(), cancel)
            .await
            .unwrap();
        assert!(result.cancelled);
        assert_eq!(
            result.error.as_deref(),
            Some("command cancelled before startup")
        );
        assert_eq!(result.disposition(), "Inconclusive");
    }
}
