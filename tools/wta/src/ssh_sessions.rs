//! One-shot, read-only remote history over the user's existing OpenSSH setup.
//! This transport never joins the master agent pool or exposes client tools.

use std::collections::HashSet;
use std::ffi::OsString;
use std::net::Ipv6Addr;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::agent_sessions::{AgentSession, CliSource, SessionLocation};
use crate::coordinator::{quote_windows_commandline_arg, sh_quote};

const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(60);
const LIST_TIMEOUT: Duration = Duration::from_secs(30);
const REAP_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(
    Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, clap::ValueEnum,
)]
#[serde(rename_all = "lowercase")]
pub enum SshPlatform {
    #[default]
    Posix,
    Windows,
}

impl SshPlatform {
    pub(crate) fn is_posix(&self) -> bool {
        *self == Self::Posix
    }

    pub(crate) fn validate_agent(self, agent_id: &str) -> Result<()> {
        if self == Self::Windows && agent_id != "copilot" {
            bail!("Windows SSH sessions support only agent_id copilot; select Copilot for this source");
        }
        known_profile(agent_id)?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "UncheckedSshTarget")]
pub struct SshTarget {
    destination: String,
    port: Option<u16>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UncheckedSshTarget {
    destination: String,
    port: Option<u16>,
}

impl TryFrom<UncheckedSshTarget> for SshTarget {
    type Error = anyhow::Error;

    fn try_from(value: UncheckedSshTarget) -> Result<Self> {
        Self::new(&value.destination, value.port)
    }
}

impl SshTarget {
    pub(crate) fn new(destination: &str, port: Option<u16>) -> Result<Self> {
        if port == Some(0) {
            bail!("SSH port must be between 1 and 65535");
        }
        let host = match destination.split_once('@') {
            Some((user, host)) => {
                if !valid_name(user) {
                    bail!("SSH destination has an invalid user name");
                }
                host
            }
            None => destination,
        };
        let valid_host = if host.contains(':') || host.starts_with('[') {
            let address = if host.starts_with('[') {
                host.strip_prefix('[')
                    .and_then(|s| s.strip_suffix(']'))
                    .unwrap_or("")
            } else {
                host
            };
            let address = match address.split_once('%') {
                Some((ip, zone)) if valid_name(zone) => ip,
                Some(_) => "",
                None => address,
            };
            address.parse::<Ipv6Addr>().is_ok()
        } else {
            valid_name(host)
        };
        if !valid_host {
            bail!("SSH destination must be an OpenSSH alias or [user@]host/IP, without options, whitespace, or shell metacharacters");
        }
        Ok(Self {
            destination: destination.to_string(),
            port,
        })
    }

    pub(crate) fn destination(&self) -> &str {
        &self.destination
    }

    pub(crate) fn port(&self) -> Option<u16> {
        self.port
    }

    pub(crate) fn display_name(&self) -> String {
        let Some(port) = self.port else {
            return self.destination.clone();
        };
        let (prefix, host) = self
            .destination
            .rsplit_once('@')
            .map(|(user, host)| (format!("{user}@"), host))
            .unwrap_or_else(|| (String::new(), self.destination.as_str()));
        if host.contains(':') && !host.starts_with('[') {
            format!("{prefix}[{host}]:{port}")
        } else {
            format!("{}:{port}", self.destination)
        }
    }
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

fn known_profile(agent_id: &str) -> Result<&'static crate::agent_registry::AgentProfile> {
    if !crate::agent_registry::is_known_id(agent_id) {
        bail!("SSH session history requires a known built-in agent CLI id");
    }
    Ok(crate::agent_registry::lookup_profile_by_id(agent_id))
}

fn listing_script(agent_id: &str) -> Result<String> {
    known_profile(agent_id)?;
    // Only tokenize the registry's command. Never resolve Windows executables
    // or apply a local provider/credential environment to this remote agent.
    let command = crate::agent_registry::build_acp_command(agent_id, None);
    let args = crate::coordinator::split_windows_commandline(&command);
    if args.is_empty() {
        bail!("Remote ACP command is empty");
    }
    Ok(format!(
        "exec {}",
        args.iter()
            .map(|arg| sh_quote(arg))
            .collect::<Vec<_>>()
            .join(" ")
    ))
}

fn ssh_arguments(target: &SshTarget, interactive: bool, script: &str) -> Vec<String> {
    ssh_command_arguments(target, interactive, remote_command(interactive, script))
}

fn ssh_command_arguments(target: &SshTarget, interactive: bool, command: String) -> Vec<String> {
    let mut args = vec![if interactive { "-t" } else { "-T" }.to_string()];
    for option in [
        "BatchMode=yes",
        "StrictHostKeyChecking=yes",
        "ConnectTimeout=10",
        "ClearAllForwardings=yes",
        "EscapeChar=none",
        "ForwardAgent=no",
        "ForwardX11=no",
        "PermitLocalCommand=no",
        "RemoteCommand=none",
        "ControlMaster=no",
        "ControlPath=none",
        "Tunnel=no",
        // SetEnv is first-value-wins, unlike additive SendEnv. Supplying this
        // fixed terminal type prevents config SetEnv from injecting credentials
        // or routing metadata; no local values are interpolated.
        "SetEnv=TERM=xterm-256color",
    ] {
        args.extend(["-o".to_string(), option.to_string()]);
    }
    if let Some(port) = target.port() {
        args.extend(["-p".to_string(), port.to_string()]);
    }
    args.push("--".to_string());
    args.push(target.destination().to_string());
    // One process argument, never a local shell command.
    args.push(command);
    args
}

fn remote_command(interactive: bool, script: &str) -> String {
    if interactive {
        format!("sh -lc {}", sh_quote(script))
    } else {
        // Preserve login PATH/environment, but reserve stdout for ACP only.
        // Restore the saved descriptor after startup files have finished.
        format!(
            "sh -lc {} 3>&1 1>&2",
            sh_quote(&format!("exec 1>&3 3>&-; {script}"))
        )
    }
}

fn listing_arguments(
    target: &SshTarget,
    agent_id: &str,
    platform: SshPlatform,
) -> Result<Vec<String>> {
    platform.validate_agent(agent_id)?;
    match platform {
        SshPlatform::Posix => Ok(ssh_arguments(target, false, &listing_script(agent_id)?)),
        SshPlatform::Windows => Ok(ssh_command_arguments(
            target,
            false,
            "cmd.exe /d /q /v:off /c copilot --acp --stdio".into(),
        )),
    }
}

fn system_ssh_executable() -> Result<PathBuf> {
    let root = std::env::var_os("SystemRoot").context("SystemRoot is not set")?;
    let root = PathBuf::from(root);
    if !root.is_absolute() {
        bail!("SystemRoot must be an absolute Windows path");
    }
    Ok(root.join(r"System32\OpenSSH\ssh.exe"))
}

fn configure_ssh_environment(
    command: &mut tokio::process::Command,
    environment: impl IntoIterator<Item = (OsString, OsString)>,
) {
    // An SSH config may contain broad SendEnv patterns. Keep the environment
    // needed for Windows, user configuration, and SSH authentication, but not
    // inherited WTA routing/MCP metadata, provider keys, or agent credentials.
    command.env_clear();
    for (name, value) in environment {
        if matches!(
            name.to_string_lossy().to_ascii_uppercase().as_str(),
            "SYSTEMROOT"
                | "WINDIR"
                | "SYSTEMDRIVE"
                | "COMSPEC"
                | "PATH"
                | "PATHEXT"
                | "USERPROFILE"
                | "HOME"
                | "HOMEDRIVE"
                | "HOMEPATH"
                | "APPDATA"
                | "LOCALAPPDATA"
                | "PROGRAMDATA"
                | "PROGRAMFILES"
                | "PROGRAMFILES(X86)"
                | "TEMP"
                | "TMP"
                | "USERNAME"
                | "USERDOMAIN"
                | "LOGONSERVER"
                | "OS"
                | "SSH_AUTH_SOCK"
                | "SSH_AGENT_PID"
        ) {
            command.env(name, value);
        }
    }
}

/// Call inside a `LocalSet`. No session is created, loaded, or prompted.
pub(crate) async fn list_sessions(
    target: &SshTarget,
    agent_id: &str,
    platform: SshPlatform,
) -> Result<Vec<AgentSession>> {
    let args = listing_arguments(target, agent_id, platform)?;
    let executable = system_ssh_executable()?;
    validate_process_length(&executable, &args)?;
    let mut command = tokio::process::Command::new(executable);
    command
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    configure_ssh_environment(&mut command, std::env::vars_os());
    #[cfg(windows)]
    command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    let mut child = command
        .spawn()
        .context("Start Windows system OpenSSH ssh.exe")?;

    let result = crate::protocol::acp::session_list::fetch_session_list(
        &mut child,
        &format!("ssh:{}:{agent_id}", target.display_name()),
        INITIALIZE_TIMEOUT,
        LIST_TIMEOUT,
    )
    .await
    .and_then(|(_, outcome)| outcome.map_err(anyhow::Error::msg));

    // Even a successful list must terminate and reap this exact one-shot SSH
    // process. Dropping/cancelling the future also kills it via kill_on_drop.
    let cleanup = tokio::time::timeout(REAP_TIMEOUT, async {
        if child
            .try_wait()
            .context("Check SSH child status")?
            .is_none()
        {
            child.kill().await.context("Kill and reap SSH child")?;
        }
        Ok::<(), anyhow::Error>(())
    })
    .await
    .map_err(|_| anyhow!("Timed out reaping SSH child"))
    .and_then(|result| result);
    let sessions = match (result, cleanup) {
        (Ok(sessions), Ok(())) => sessions,
        (Ok(_), Err(error)) => return Err(error),
        (Err(error), cleanup) => {
            let error = if let Err(cleanup_error) = cleanup {
                error.context(format!("SSH cleanup also failed: {cleanup_error:#}"))
            } else {
                error
            };
            return Err(error.context(format!(
                "Read SSH session history from {} failed. The host must already be verified in known_hosts and configured for non-interactive SSH authentication",
                target.display_name()
            )));
        }
    };
    Ok(map_remote_sessions(target, agent_id, &sessions))
}

fn map_remote_sessions(
    target: &SshTarget,
    agent_id: &str,
    sessions: &[agent_client_protocol::schema::v1::SessionInfo],
) -> Vec<AgentSession> {
    // Host origin IDs are not meaningful on another machine. Reuse only the
    // shared placeholder classifier, without touching any host registry/index.
    crate::session_history::classify_and_map(
        sessions,
        &HashSet::new(),
        SessionLocation::Ssh {
            target: target.clone(),
        },
        &CliSource::parse(Some(agent_id)),
    )
}

pub(crate) fn resume_commandline(
    target: &SshTarget,
    agent_id: &str,
    session_id: &str,
    cwd: &str,
    platform: SshPlatform,
) -> Result<String> {
    let args = resume_arguments(target, agent_id, session_id, cwd, platform)?;
    validate_process_length(&system_ssh_executable()?, &args)?;
    let executable = std::env::current_exe().context("Resolve SSH resume launcher")?;
    let executable = executable
        .to_str()
        .context("SSH resume launcher path is not Unicode")?;
    if !std::path::Path::new(executable).is_absolute() || executable.contains('%') {
        bail!("SSH resume launcher requires an absolute path without percent signs");
    }
    let payload = serde_json::to_string(&ResumeRequest {
        target: target.clone(),
        agent_id: agent_id.to_string(),
        session_id: session_id.to_string(),
        cwd: cwd.to_string(),
        platform,
    })?;
    // JSON Unicode escaping is decoded locally, after Terminal's environment
    // expansion. No shell wrapper, remote decoder, or credential-bearing argv.
    let payload = payload.replace('%', r"\u0025");
    let commandline = format!(
        "{} ssh-resume --payload {}",
        quote_windows_commandline_arg(executable),
        quote_windows_commandline_arg(&payload)
    );
    validate_local_command_length(&commandline)?;
    Ok(commandline)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResumeRequest {
    target: SshTarget,
    agent_id: String,
    session_id: String,
    cwd: String,
    #[serde(default)]
    platform: SshPlatform,
}

fn validate_local_command_length(commandline: &str) -> Result<()> {
    if commandline.encode_utf16().count() >= 32767 {
        bail!("SSH command exceeds the local Windows 32767-character command line limit");
    }
    Ok(())
}

fn validate_process_length(executable: &std::path::Path, args: &[String]) -> Result<()> {
    // Include the executable's quotes even when they are not needed.
    let commandline = format!(
        "\"{}\" {}",
        executable
            .to_str()
            .context("SSH executable path is not Unicode")?,
        args.iter()
            .map(|arg| quote_windows_commandline_arg(arg))
            .collect::<Vec<_>>()
            .join(" ")
    );
    validate_local_command_length(&commandline)
}

fn validate_windows_cwd(cwd: &str) -> Result<()> {
    let bytes = cwd.as_bytes();
    let valid = bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1..3] == *b":\\"
        && !cwd[3..]
            .chars()
            .any(|c| c.is_control() || "<>:\"/|?*".contains(c))
        && cwd[3..].split('\\').all(|part| {
            let basename = part
                .split('.')
                .next()
                .unwrap_or("")
                .trim_end_matches(' ')
                .to_ascii_uppercase();
            let device_number = basename
                .strip_prefix("COM")
                .or_else(|| basename.strip_prefix("LPT"));
            !matches!(
                basename.as_str(),
                "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
            ) && !device_number.is_some_and(|number| {
                matches!(
                    number,
                    "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
            }) && !part.ends_with(['.', ' '])
        });
    if !valid {
        bail!("Windows SSH cwd must be an ordinary absolute drive directory (for example C:\\repo or Q:\\Copilot); relative, UNC, provider and device paths are unsupported");
    }
    Ok(())
}

fn windows_resume_script(agent_id: &str, session_id: &str, cwd: &str) -> Result<String> {
    SshPlatform::Windows.validate_agent(agent_id)?;
    if session_id.len() != 36
        || !session_id.bytes().enumerate().all(|(i, b)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
    {
        bail!("Windows SSH Copilot session ID must be a 36-character hyphenated UUID; refresh the Copilot history and select a supported session");
    }
    validate_windows_cwd(cwd)?;
    // Only Base64 literals enter executable source. Decoded data is never parsed
    // as PowerShell, including typographic quotes and percent expansions.
    let cwd = crate::osc52::base64_encode(cwd.as_bytes());
    let sid = crate::osc52::base64_encode(session_id.as_bytes());
    Ok(format!(
        "$ErrorActionPreference='Stop';\
         $cmd=$env:ComSpec;\
         $env:NoDefaultCurrentDirectoryInExePath='1';\
         $cwd=[Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('{cwd}'));\
         $sid=[Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('{sid}'));\
         try {{ Set-Location -LiteralPath $cwd -ErrorAction Stop; \
         & $cmd /d /q /v:off /c copilot ('--resume=' + $sid); exit $LASTEXITCODE }} \
         catch {{ [Console]::Error.WriteLine($_.Exception.Message); exit 1 }}"
    ))
}

fn validate_windows_remote_length(command: &str) -> Result<()> {
    // CMD permits 8191 characters. Reserve 256 for sshd's shell path, /c and
    // quoting rather than assuming the server's exact SystemRoot spelling.
    if command.encode_utf16().count() > 8191 - 256 {
        bail!("Windows SSH command exceeds the remote CMD command line budget (8191 characters including a 256-character sshd shell reserve); use a shorter cwd");
    }
    Ok(())
}

fn resume_arguments(
    target: &SshTarget,
    agent_id: &str,
    session_id: &str,
    cwd: &str,
    platform: SshPlatform,
) -> Result<Vec<String>> {
    match platform {
        SshPlatform::Posix => Ok(ssh_arguments(
            target,
            true,
            &resume_script(agent_id, session_id, cwd)?,
        )),
        SshPlatform::Windows => {
            let script = windows_resume_script(agent_id, session_id, cwd)?;
            let bytes: Vec<_> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
            let command = format!(
                "powershell.exe -NoLogo -NoProfile -EncodedCommand {}",
                crate::osc52::base64_encode(&bytes)
            );
            validate_windows_remote_length(&command)?;
            Ok(ssh_command_arguments(target, true, command))
        }
    }
}

fn resume_script(agent_id: &str, session_id: &str, cwd: &str) -> Result<String> {
    let profile = known_profile(agent_id)?;
    if profile.resume_flag.is_empty() {
        bail!("The remote agent CLI does not support session resume");
    }
    if session_id.trim().is_empty()
        || session_id.starts_with('-')
        || session_id.chars().any(char::is_control)
    {
        bail!("Remote session id must be nonempty, without controls or a leading option");
    }
    // Path::is_absolute on Windows rejects ordinary POSIX paths. The cwd is
    // remote data: validate its syntax, never inspect it on the host.
    if !cwd.starts_with('/') || cwd.chars().any(char::is_control) {
        bail!("Remote session cwd must be an absolute POSIX directory without controls");
    }
    Ok(format!(
        "cd -- {} && exec {} {} {}",
        sh_quote(cwd),
        sh_quote(profile.id),
        sh_quote(profile.resume_flag),
        sh_quote(session_id)
    ))
}

fn resume_process(
    payload: &str,
    environment: impl IntoIterator<Item = (OsString, OsString)>,
) -> Result<tokio::process::Command> {
    if payload.len() > 131068 || payload.contains('%') {
        bail!("Invalid SSH resume payload");
    }
    // Do not attach deserializer errors: unknown field names are payload data.
    let request: ResumeRequest = serde_json::from_str(payload).map_err(|_| {
        anyhow!(
            "Invalid SSH resume payload or unsupported platform; update Terminal and WTA together"
        )
    })?;
    let args = resume_arguments(
        &request.target,
        &request.agent_id,
        &request.session_id,
        &request.cwd,
        request.platform,
    )?;
    let executable = system_ssh_executable()?;
    validate_process_length(&executable, &args)?;
    let mut command = tokio::process::Command::new(executable);
    command
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .kill_on_drop(true);
    configure_ssh_environment(&mut command, environment);
    Ok(command)
}

pub(crate) async fn run_resume(payload: &str) -> Result<()> {
    let status = resume_process(payload, std::env::vars_os())?
        .status()
        .await
        .context("Run Windows system OpenSSH ssh.exe")?;
    crate::logging::shutdown_flush();
    std::process::exit(status.code().unwrap_or(255));
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOWS_SID: &str = "12345678-abcd-1234-abcd-123456789abc";

    #[test]
    fn windows_listing_is_one_fixed_cmd_argument_without_posix_wrappers() {
        let target = SshTarget::new("devbox", Some(2222)).unwrap();
        let args = listing_arguments(&target, "copilot", SshPlatform::Windows).unwrap();
        let posix = listing_arguments(&target, "copilot", SshPlatform::Posix).unwrap();
        assert_eq!(&args[..args.len() - 1], &posix[..posix.len() - 1]);
        assert_eq!(
            args.last().unwrap(),
            "cmd.exe /d /q /v:off /c copilot --acp --stdio"
        );
        assert!(!args.last().unwrap().contains("sh -lc"));
        assert!(!args.last().unwrap().contains("3>&1"));
        for agent in [
            "claude",
            "codex",
            "gemini",
            "opencode",
            "unknown",
            "custom:copilot",
        ] {
            let error = listing_arguments(&target, agent, SshPlatform::Windows).unwrap_err();
            assert!(error.to_string().contains("only agent_id copilot"));
            assert!(resume_arguments(
                &target,
                agent,
                WINDOWS_SID,
                r"Q:\Copilot",
                SshPlatform::Windows
            )
            .is_err());
        }
        assert!(SshTarget::new(r"redmond\haonantang@devbox", None).is_err());
    }

    #[test]
    fn windows_cwd_is_remote_drive_data_not_a_local_path_or_provider() {
        for cwd in [
            r"C:\",
            r"C:\Windows\system32",
            r"Q:\Copilot",
            r"z:\not-on-this-host",
            r"Q:\世界\%PATH%\O'Brien’s [repo] & $(whoami);`x`",
        ] {
            validate_windows_cwd(cwd).unwrap();
            let script = windows_resume_script("copilot", WINDOWS_SID, cwd).unwrap();
            assert!(script.contains(&crate::osc52::base64_encode(cwd.as_bytes())));
            assert!(!script.contains(cwd));
            assert!(!script.contains("Invoke-Expression"));
        }
        for cwd in [
            "",
            ".",
            r"relative\repo",
            r"C:repo",
            r"\repo",
            "/repo",
            r"\\host\share",
            r"\\?\C:\repo",
            r"\\.\C:\repo",
            r"\??\C:\repo",
            r"FileSystem::C:\repo",
            r"Microsoft.PowerShell.Core\FileSystem::C:\repo",
            r"HKLM:\Software",
            r"C:/repo",
            r"C:\repo:stream",
            r"C:\NUL",
            r"C:\CON.txt",
            r"C:\COM1",
            r"C:\COM¹",
            r"C:\LPT².txt",
            r"C:\CON .txt",
            "C:\\repo\n",
            "C:\\repo\0",
            r"C:\repo*",
            r"C:\repo?",
            "C:\\repo.",
        ] {
            assert!(validate_windows_cwd(cwd).is_err(), "{cwd:?}");
        }
    }

    #[test]
    fn windows_session_ids_are_exact_uuid_data_and_not_arguments() {
        for id in [WINDOWS_SID.to_string(), WINDOWS_SID.to_ascii_uppercase()] {
            let script = windows_resume_script("copilot", &id, r"Q:\Copilot").unwrap();
            assert!(script.contains(&crate::osc52::base64_encode(id.as_bytes())));
            assert!(!script.contains(&id));
            assert!(script.contains(
                "& $cmd /d /q /v:off /c copilot ('--resume=' + $sid); exit $LASTEXITCODE"
            ));
        }
        for id in [
            "",
            "id",
            "-option",
            "--help",
            "name with space",
            "a;b",
            "a'b",
            "a’b",
            "a%PATH%",
            "a\n",
            "a\0",
            "12345678-abcd-1234-abcd-123456789abz",
            "{12345678-abcd-1234-abcd-123456789abc}",
            "12345678abcd1234abcd123456789abc",
            "12345678-abcd-1234-abcd-123456789abc --help",
        ] {
            assert!(
                windows_resume_script("copilot", id, r"Q:\Copilot").is_err(),
                "{id:?}"
            );
        }
        assert!(windows_resume_script("copilot", &"a".repeat(10000), r"Q:\Copilot").is_err());
        assert!(resume_script("copilot", "arbitrary'POSIX id", "/repo").is_ok());
    }

    #[test]
    #[cfg(windows)]
    fn windows_resume_payload_roundtrips_data_and_uses_utf16le_powershell_pty() {
        let target = SshTarget::new("devbox", None).unwrap();
        let cwd = r"Q:\Copilot\世界 %PATH% O’Brien";
        let command =
            resume_commandline(&target, "copilot", WINDOWS_SID, cwd, SshPlatform::Windows).unwrap();
        assert!(!command.contains('%'));
        let launcher = windows_argv(&command);
        let request: ResumeRequest = serde_json::from_str(&launcher[3]).unwrap();
        assert_eq!(request.platform, SshPlatform::Windows);
        assert_eq!(request.cwd, cwd);
        assert_eq!(request.session_id, WINDOWS_SID);
        let process = resume_process(&launcher[3], []).unwrap();
        let args: Vec<_> = process
            .as_std()
            .get_args()
            .map(|s| s.to_str().unwrap())
            .collect();
        assert_eq!(args[0], "-t");
        let script = windows_resume_script("copilot", WINDOWS_SID, cwd).unwrap();
        let bytes: Vec<_> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
        assert_eq!(
            *args.last().unwrap(),
            format!(
                "powershell.exe -NoLogo -NoProfile -EncodedCommand {}",
                crate::osc52::base64_encode(&bytes)
            )
        );
        assert!(!args.last().unwrap().contains("sh -lc"));
    }

    #[test]
    fn ssh_resume_missing_platform_is_posix_and_unknown_fields_fail_closed() {
        let legacy = serde_json::json!({
            "target": { "destination": "host", "port": null },
            "agent_id": "copilot", "session_id": "arbitrary-posix-id", "cwd": "/repo"
        });
        let restored: ResumeRequest = serde_json::from_value(legacy.clone()).unwrap();
        assert_eq!(restored.platform, SshPlatform::Posix);
        assert_eq!(serde_json::to_value(restored).unwrap()["platform"], "posix");
        for field in ["platform", "future_field"] {
            let mut value = legacy.clone();
            value[field] = serde_json::json!("auto");
            assert!(resume_process(&value.to_string(), []).is_err());
        }
        let mut wrong = legacy;
        wrong["platform"] = serde_json::json!("windows");
        assert!(resume_process(&wrong.to_string(), []).is_err());
    }

    #[test]
    fn ssh_command_length_limits_count_utf16_and_include_transport_overhead() {
        assert!(validate_local_command_length(&"x".repeat(32766)).is_ok());
        assert!(validate_local_command_length(&"x".repeat(32767)).is_err());
        assert!(validate_local_command_length(&"😀".repeat(16383)).is_ok());
        assert!(validate_local_command_length(&"😀".repeat(16384)).is_err());
        assert!(validate_windows_remote_length(&"x".repeat(8191 - 256)).is_ok());
        assert!(validate_windows_remote_length(&"x".repeat(8192 - 256)).is_err());
        let target = SshTarget::new("host", None).unwrap();
        assert!(resume_arguments(
            &target,
            "copilot",
            WINDOWS_SID,
            &format!("Q:\\{}", "a".repeat(3000)),
            SshPlatform::Windows
        )
        .is_err());
        let executable = std::path::Path::new(r"C:\Windows\System32\OpenSSH\ssh.exe");
        let overhead = executable.to_str().unwrap().encode_utf16().count() + 3;
        assert!(validate_process_length(executable, &["x".repeat(32766 - overhead)]).is_ok());
        assert!(validate_process_length(executable, &["x".repeat(32767 - overhead)]).is_err());
    }

    #[cfg(windows)]
    fn run_windows_powershell_with_path(
        script: &str,
        path_prefix: Option<&std::path::Path>,
    ) -> std::process::Output {
        let executable = PathBuf::from(std::env::var_os("SystemRoot").unwrap())
            .join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
        let bytes: Vec<_> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let mut command = std::process::Command::new(executable);
        command
            .args(["-NoLogo", "-NoProfile", "-EncodedCommand"])
            .arg(crate::osc52::base64_encode(&bytes));
        if let Some(prefix) = path_prefix {
            let path = std::env::join_paths(std::iter::once(prefix.to_path_buf()).chain(
                std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
            ))
            .unwrap();
            command.env("PATH", path);
        }
        command.output().unwrap()
    }

    #[test]
    #[cfg(windows)]
    fn windows_powershell_51_decodes_data_and_uses_cmd_shim() {
        let cwd = r"Q:\世界\%PATH%\O'Brien’s [repo] & $(whoami);`x`";
        let shim_dir =
            std::env::temp_dir().join(format!("wta-ssh-copilot-shim-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&shim_dir).unwrap();
        std::fs::write(
            shim_dir.join("copilot.cmd"),
            "@echo off\r\necho %*\r\nexit /b 23\r\n",
        )
        .unwrap();
        std::fs::write(
            shim_dir.join("copilot.ps1"),
            "throw 'PowerShell shim must not run'\r\n",
        )
        .unwrap();
        let stubs = "function Set-Location { param($LiteralPath,$ErrorAction) \
            [Console]::WriteLine([Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($LiteralPath))) };";
        let script = windows_resume_script("copilot", WINDOWS_SID, cwd).unwrap();
        let output = run_windows_powershell_with_path(&format!("{stubs}{script}"), Some(&shim_dir));
        let _ = std::fs::remove_dir_all(&shim_dir);
        assert_eq!(output.status.code(), Some(23), "{:?}", output);
        let text = String::from_utf8(output.stdout).unwrap();
        assert_eq!(
            text.lines().collect::<Vec<_>>(),
            [
                crate::osc52::base64_encode(cwd.as_bytes()),
                format!("--resume={WINDOWS_SID}")
            ]
        );
    }

    #[test]
    #[cfg(windows)]
    fn windows_powershell_51_location_failure_never_runs_copilot() {
        let cwd = std::env::current_dir().unwrap();
        let shim_dir =
            std::env::temp_dir().join(format!("wta-ssh-copilot-shim-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&shim_dir).unwrap();
        std::fs::write(
            shim_dir.join("copilot.cmd"),
            "@echo off\r\necho copilot-ran\r\nexit /b 19\r\n",
        )
        .unwrap();
        let valid = windows_resume_script("copilot", WINDOWS_SID, cwd.to_str().unwrap()).unwrap();
        assert_eq!(
            run_windows_powershell_with_path(&valid, Some(&shim_dir))
                .status
                .code(),
            Some(19)
        );
        let missing = cwd.join(format!("missing-ssh-test-{}", uuid::Uuid::new_v4()));
        let invalid =
            windows_resume_script("copilot", WINDOWS_SID, missing.to_str().unwrap()).unwrap();
        let output = run_windows_powershell_with_path(&invalid, Some(&shim_dir));
        let _ = std::fs::remove_dir_all(&shim_dir);
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        assert!(!output.stderr.is_empty());
    }

    #[test]
    #[cfg(windows)]
    fn windows_powershell_51_does_not_run_a_cwd_copilot_shim() {
        let root =
            std::env::temp_dir().join(format!("wta-ssh-copilot-search-{}", uuid::Uuid::new_v4()));
        let cwd = root.join("repo");
        let shim_dir = root.join("path");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::create_dir(&shim_dir).unwrap();
        std::fs::write(
            cwd.join("copilot.cmd"),
            "@echo off\r\necho cwd-hijack\r\nexit /b 31\r\n",
        )
        .unwrap();
        std::fs::write(
            shim_dir.join("copilot.cmd"),
            "@echo off\r\necho trusted %*\r\nexit /b 23\r\n",
        )
        .unwrap();
        let script = windows_resume_script("copilot", WINDOWS_SID, cwd.to_str().unwrap()).unwrap();
        let output = run_windows_powershell_with_path(&script, Some(&shim_dir));
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(output.status.code(), Some(23), "{:?}", output);
        assert_eq!(
            String::from_utf8(output.stdout).unwrap().trim(),
            format!("trusted --resume={WINDOWS_SID}")
        );
    }

    #[cfg(windows)]
    fn windows_argv(commandline: &str) -> Vec<String> {
        let input: Vec<u16> = commandline.encode_utf16().chain(Some(0)).collect();
        // SAFETY: input is NUL-terminated and both calls provide valid buffers.
        let wide = unsafe {
            use windows_sys::Win32::System::Environment::ExpandEnvironmentStringsW;
            let size = ExpandEnvironmentStringsW(input.as_ptr(), std::ptr::null_mut(), 0);
            assert!(size > 0);
            let mut expanded = vec![0; size as usize];
            assert_eq!(
                ExpandEnvironmentStringsW(input.as_ptr(), expanded.as_mut_ptr(), size),
                size
            );
            expanded
        };
        let mut count = 0;
        // SAFETY: the input is NUL-terminated. Windows returns count valid,
        // NUL-terminated strings in one allocation, freed after copying them.
        unsafe {
            let argv = windows_sys::Win32::UI::Shell::CommandLineToArgvW(wide.as_ptr(), &mut count);
            assert!(!argv.is_null());
            let result = std::slice::from_raw_parts(argv, count as usize)
                .iter()
                .map(|&arg| {
                    let mut len = 0;
                    while *arg.add(len) != 0 {
                        len += 1;
                    }
                    String::from_utf16_lossy(std::slice::from_raw_parts(arg, len))
                })
                .collect();
            windows_sys::Win32::Foundation::LocalFree(argv.cast());
            result
        }
    }

    #[test]
    fn targets_preserve_alias_case_and_support_user_ip_and_ipv6() {
        for destination in [
            "Production-Alias",
            "user@host.example",
            "127.0.0.1",
            "user@127.0.0.1",
            "::1",
            "[2001:db8::1]",
            "user@[fe80::1%eth0]",
        ] {
            let target = SshTarget::new(destination, Some(2222)).unwrap();
            assert_eq!(target.destination(), destination);
            assert_eq!(target.port(), Some(2222));
            let json = serde_json::to_string(&target).unwrap();
            assert_eq!(serde_json::from_str::<SshTarget>(&json).unwrap(), target);
        }
        assert_eq!(
            SshTarget::new("user@::1", Some(2222))
                .unwrap()
                .display_name(),
            "user@[::1]:2222"
        );
        assert_eq!(
            SshTarget::new("Alias", None).unwrap().display_name(),
            "Alias"
        );
    }

    #[test]
    fn invalid_targets_are_rejected_by_constructor_and_deserializer() {
        for destination in [
            "",
            "-oProxyCommand=evil",
            "user@-host",
            "@host",
            "user@",
            "a@b@host",
            "host name",
            "host\n",
            "host\0",
            "host\t",
            "host;id",
            "host&&id",
            "host|id",
            "$(id)",
            "`id`",
            "host'quote",
            "host\"quote",
            "host\\path",
            "ssh://host",
            "host:22",
            "[::1]:22",
            "[host]",
            "::invalid",
            "::1%eth0%more",
            "host*",
            "host?",
            "host#comment",
            "host>file",
            "host<file",
            "host(1)",
            "host{a,b}",
        ] {
            assert!(
                SshTarget::new(destination, None).is_err(),
                "{destination:?}"
            );
            assert!(
                serde_json::from_value::<SshTarget>(serde_json::json!({
                    "destination": destination, "port": null
                }))
                .is_err(),
                "{destination:?}"
            );
        }
        assert!(SshTarget::new("host", Some(0)).is_err());
        for port in [0, 65536, -1] {
            assert!(serde_json::from_value::<SshTarget>(serde_json::json!({
                "destination": "host", "port": port
            }))
            .is_err());
        }
        assert!(serde_json::from_value::<SshTarget>(serde_json::json!({
            "destination": "host", "command": "evil"
        }))
        .is_err());
    }

    #[test]
    fn listing_is_noninteractive_strict_and_has_no_forwarding_or_custom_command() {
        let target = SshTarget::new("Remote-Alias", Some(2222)).unwrap();
        let script = listing_script("copilot").unwrap();
        assert_eq!(script, "exec 'copilot' '--acp' '--stdio'");
        let args = ssh_arguments(&target, false, &script);
        assert_eq!(args[0], "-T");
        for option in [
            "BatchMode=yes",
            "StrictHostKeyChecking=yes",
            "ConnectTimeout=10",
            "ClearAllForwardings=yes",
            "EscapeChar=none",
            "ForwardAgent=no",
            "ForwardX11=no",
            "PermitLocalCommand=no",
            "RemoteCommand=none",
            "ControlPath=none",
        ] {
            assert!(
                args.windows(2).any(|pair| pair == ["-o", option]),
                "{option}"
            );
        }
        assert_eq!(
            &args[args.len() - 5..args.len() - 1],
            ["-p", "2222", "--", "Remote-Alias"]
        );
        assert_eq!(args.last().unwrap(), &remote_command(false, &script));
        assert!(!args
            .iter()
            .any(|arg| matches!(arg.as_str(), "-R" | "-L" | "-D" | "-A")));
        for id in [
            "custom:copilot",
            "unknown",
            "copilot --acp",
            "Copilot",
            "C:\\copilot.exe",
        ] {
            assert!(listing_script(id).is_err(), "{id}");
        }
    }

    #[test]
    fn listing_commands_use_each_builtin_registry_entry() {
        for profile in crate::agent_registry::KNOWN_AGENTS {
            let command = crate::agent_registry::build_acp_command(profile.id, None);
            let expected = crate::coordinator::split_windows_commandline(&command)
                .iter()
                .map(|arg| sh_quote(arg))
                .collect::<Vec<_>>()
                .join(" ");
            assert_eq!(
                listing_script(profile.id).unwrap(),
                format!("exec {expected}")
            );
        }
    }

    #[test]
    fn listing_environment_keeps_ssh_setup_but_not_local_credentials_or_routing() {
        let retained = [
            ("SystemRoot", r"C:\Windows"),
            ("Path", r"C:\Windows\System32"),
            ("USERPROFILE", r"C:\Users\ssh-user"),
            ("HOME", r"C:\Users\ssh-user"),
            ("SSH_AUTH_SOCK", r"\\.\pipe\openssh-ssh-agent"),
        ];
        let excluded = [
            "GITHUB_TOKEN",
            "GH_TOKEN",
            "COPILOT_GITHUB_TOKEN",
            "COPILOT_PROVIDER_API_KEY",
            "ANTHROPIC_API_KEY",
            "OPENAI_API_KEY",
            "GEMINI_API_KEY",
            "OPENCODE_CONFIG_CONTENT",
            "INTELLIGENT_TERMINAL_MODEL_API_KEY",
            "WTA_CUSTOM_MODEL_CREDENTIAL_ID",
            "WTA_MCP_TOKEN",
            "WTA_CLI_PATH",
            "WT_COM_CLSID",
            "WT_SESSION",
            "UNRECOGNIZED_PROVIDER_SECRET",
        ];
        let mut command = tokio::process::Command::new("ssh.exe");
        command.env("PREEXISTING_SECRET", "not-retained");
        configure_ssh_environment(
            &mut command,
            retained
                .iter()
                .copied()
                .chain(excluded.iter().map(|name| (*name, "not-retained")))
                .map(|(name, value)| (OsString::from(name), OsString::from(value))),
        );
        let environment: std::collections::HashMap<_, _> = command.as_std().get_envs().collect();
        for (name, value) in retained {
            assert_eq!(
                environment.get(std::ffi::OsStr::new(name)),
                Some(&Some(std::ffi::OsStr::new(value)))
            );
        }
        assert_eq!(environment.len(), retained.len());
        for name in excluded {
            assert!(
                !environment.contains_key(std::ffi::OsStr::new(name)),
                "{name}"
            );
        }
    }

    #[test]
    #[cfg(windows)]
    fn resume_uses_each_cli_resume_flag_and_remote_cwd_without_host_resolution() {
        let target = SshTarget::new("Alias", None).unwrap();
        for profile in crate::agent_registry::KNOWN_AGENTS {
            let command = resume_commandline(
                &target,
                profile.id,
                "sid",
                "/remote/repo",
                SshPlatform::Posix,
            )
            .unwrap();
            let launcher = windows_argv(&command);
            assert_eq!(
                PathBuf::from(&launcher[0]),
                std::env::current_exe().unwrap()
            );
            assert_eq!(&launcher[1..3], ["ssh-resume", "--payload"]);
            let process = resume_process(&launcher[3], []).unwrap();
            assert_eq!(
                process.as_std().get_program(),
                system_ssh_executable().unwrap().as_os_str()
            );
            let args: Vec<_> = process
                .as_std()
                .get_args()
                .map(|arg| arg.to_str().unwrap())
                .collect();
            assert_eq!(args[0], "-t");
            assert!(!args.contains(&"cmd.exe"));
            let script = format!(
                "cd -- '/remote/repo' && exec '{}' '{}' 'sid'",
                profile.id, profile.resume_flag
            );
            assert_eq!(
                args.last().copied(),
                Some(format!("sh -lc {}", sh_quote(&script)).as_str())
            );
        }
    }

    #[test]
    #[cfg(windows)]
    fn resume_quotes_shell_metacharacters_at_both_shell_layers_and_windows_argv() {
        let target = SshTarget::new("user@[::1]", Some(2222)).unwrap();
        let id = r#"session'";$(echo injected)`id`&%PATH%\"#;
        let cwd = r#"/home/a b/世界/%PATH%/'";$(touch nope)\last"#;
        let command = resume_commandline(&target, "codex", id, cwd, SshPlatform::Posix).unwrap();
        assert!(!command.contains('%'));
        let launcher = windows_argv(&command);
        let request: ResumeRequest = serde_json::from_str(&launcher[3]).unwrap();
        assert_eq!(request.session_id, id);
        assert_eq!(request.cwd, cwd);
        let process = resume_process(&launcher[3], []).unwrap();
        let args: Vec<_> = process
            .as_std()
            .get_args()
            .map(|arg| arg.to_str().unwrap())
            .collect();
        assert_eq!(args[args.len() - 2], "user@[::1]");
        let script = format!(
            "cd -- {} && exec 'codex' 'resume' {}",
            sh_quote(cwd),
            sh_quote(id)
        );
        assert_eq!(
            args.last().copied(),
            Some(format!("sh -lc {}", sh_quote(&script)).as_str())
        );
        assert!(args.last().unwrap().contains(r"'\''"));
    }

    #[test]
    fn resume_rejects_options_controls_unknown_agents_and_non_posix_cwd() {
        let target = SshTarget::new("host", None).unwrap();
        for id in ["", " ", "-x", "--help", "id\0", "id\n", "id\r"] {
            assert!(
                resume_commandline(&target, "copilot", id, "/repo", SshPlatform::Posix).is_err()
            );
        }
        for cwd in [
            "",
            ".",
            "relative/path",
            "~/repo",
            "C:\\repo",
            "\\\\host\\share",
            "/repo\0",
            "/repo\n",
        ] {
            assert!(
                resume_commandline(&target, "copilot", "sid", cwd, SshPlatform::Posix).is_err()
            );
        }
        for agent in ["unknown", "custom:copilot", "copilot;id"] {
            assert!(
                resume_commandline(&target, agent, "sid", "/repo", SshPlatform::Posix).is_err()
            );
        }
        assert!(resume_commandline(&target, "copilot", "sid", "/", SshPlatform::Posix).is_ok());
    }

    #[test]
    fn resume_child_scrubs_inherited_environment_and_checks_payload() {
        let request = ResumeRequest {
            platform: SshPlatform::Posix,
            target: SshTarget::new("host", None).unwrap(),
            agent_id: "copilot".into(),
            session_id: "sid".into(),
            cwd: "/repo".into(),
        };
        let payload = serde_json::to_string(&request).unwrap();
        let process = resume_process(
            &payload,
            [
                ("USERPROFILE", "ssh-user"),
                ("SSH_AUTH_SOCK", "ssh-agent"),
                ("WTA_MCP_TOKEN", "secret"),
                ("OPENAI_API_KEY", "secret"),
                ("WT_COM_CLSID", "route"),
                ("UNKNOWN_PROVIDER_SECRET", "secret"),
            ]
            .map(|(name, value)| (OsString::from(name), OsString::from(value))),
        )
        .unwrap();
        let env: Vec<_> = process.as_std().get_envs().collect();
        assert_eq!(env.len(), 2);
        assert!(env
            .iter()
            .all(|(name, _)| *name == "USERPROFILE" || *name == "SSH_AUTH_SOCK"));
        for invalid in [
            "{}",
            r#"{"target":{"destination":"-oProxyCommand=bad","port":null},"agent_id":"copilot","session_id":"sid","cwd":"/repo"}"#,
            r#"{"target":{"destination":"host","port":null},"agent_id":"unknown","session_id":"sid","cwd":"/repo"}"#,
            r#"{"target":{"destination":"host","port":null},"agent_id":"copilot","session_id":"-option","cwd":"/repo"}"#,
            r#"{"target":{"destination":"host","port":null},"agent_id":"copilot","session_id":"sid","cwd":"/%PATH%"}"#,
        ] {
            assert!(resume_process(invalid, []).is_err());
        }
    }

    #[test]
    #[cfg(windows)]
    fn openssh_config_cannot_restore_setenv_but_sendenv_is_additive() {
        let fixture =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(r"tests\fixtures\ssh_sessions.conf");
        for interactive in [false, true] {
            let mut command = tokio::process::Command::new(system_ssh_executable().unwrap());
            configure_ssh_environment(&mut command, std::env::vars_os());
            command.args(["-G", "-F"]).arg(&fixture);
            command.args(["-o", "SendEnv=-*"]);
            command.args(ssh_arguments(
                &SshTarget::new("fixture", None).unwrap(),
                interactive,
                "true",
            ));
            let output = command.as_std_mut().output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let config = String::from_utf8(output.stdout).unwrap();
            assert!(config.lines().any(|line| line == "sendenv *"));
            assert!(config.lines().any(|line| line == "escapechar none"));
            assert_eq!(
                config
                    .lines()
                    .filter(|line| line.starts_with("setenv "))
                    .collect::<Vec<_>>(),
                ["setenv TERM=xterm-256color"]
            );
            assert!(config.contains("identityfile ~/.ssh/fixture"));
            assert!(config.contains("proxycommand ssh -W %h:%p jump"));
        }
    }

    #[test]
    fn login_startup_banner_is_stderr_but_login_path_and_acp_stdio_survive() {
        #[cfg(windows)]
        let shell = PathBuf::from(r"C:\Program Files\Git\bin\bash.exe");
        #[cfg(not(windows))]
        let shell = PathBuf::from("/bin/bash");
        if !shell.is_file() {
            eprintln!("Skipping POSIX fixture: bash is not installed");
            return;
        }
        // Emulate login startup without reading or modifying real dotfiles.
        let fixture = r#"sh() {
            test "$1" = -lc || return 99
            printf 'login-banner\n'
            export PATH="/fixture/login:$PATH"
            command sh -c "$2"
        }; "#;
        let script = r#"read -r request; printf 'ACP:%s:%s\n' "$request" "$PATH"; printf 'agent-diagnostic\n' >&2; exit 23"#;
        let mut child = std::process::Command::new(shell)
            .args(["--noprofile", "--norc", "-c"])
            .arg(format!("{fixture}{}", remote_command(false, script)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        use std::io::Write;
        child.stdin.take().unwrap().write_all(b"request\n").unwrap();
        let output = child.wait_with_output().unwrap();
        assert_eq!(output.status.code(), Some(23));
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(stdout.starts_with("ACP:request:/fixture/login:"));
        assert!(!stdout.contains("banner"));
        assert_eq!(
            String::from_utf8(output.stderr).unwrap(),
            ["login-banner", "agent-diagnostic", ""].join("\n")
        );
    }

    #[test]
    fn remote_rows_are_historical_unbound_and_use_only_shared_placeholder_filter() {
        use agent_client_protocol::schema::v1::{SessionId, SessionInfo};
        let target = SshTarget::new("host", None).unwrap();
        let mut placeholder = SessionInfo::new(SessionId::new("empty"), PathBuf::from("/repo"));
        placeholder.title = Some("New session - 2026-07-23T01:14:00.422Z".into());
        let real = SessionInfo::new(SessionId::new("same-as-host-id"), PathBuf::from("/repo"));
        let rows = map_remote_sessions(&target, "opencode", &[placeholder, real]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].key, "same-as-host-id");
        assert_eq!(rows[0].cli_source, CliSource::OpenCode);
        assert_eq!(rows[0].location, SessionLocation::Ssh { target });
        assert_eq!(
            rows[0].status,
            crate::agent_sessions::AgentStatus::Historical
        );
        assert_eq!(
            rows[0].origin,
            crate::agent_sessions::SessionOrigin::Unknown
        );
        assert!(rows[0].pane_session_id.is_none());
        assert!(rows[0].window_id.is_none());
        assert!(rows[0].tab_id.is_none());
    }
}
