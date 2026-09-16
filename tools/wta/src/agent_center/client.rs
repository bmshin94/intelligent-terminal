// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use super::commands::{self, Action, CommandContext, Operation};
use super::transport::Client;
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::io::Write;
use tokio::io::AsyncReadExt;

const MAX_JSON_INPUT_BYTES: u64 = 1_048_576;

/// Resolve external input before pure command compilation. A path is never
/// guessed to be inline JSON, and the Console never races its keyboard reader.
pub async fn resolve_input_args(args: Vec<String>, allow_stdin: bool) -> Result<Vec<String>> {
    if args.last().is_some_and(|arg| arg == "--help") && !args.iter().any(|arg| arg == "--") {
        return Ok(args);
    }
    let mut resolved = Vec::with_capacity(args.len());
    let mut args = args.into_iter();
    let mut payload_seen = false;
    while let Some(argument) = args.next() {
        if argument == "--" {
            resolved.push(argument);
            resolved.extend(args);
            break;
        }
        let flag = matches!(
            argument.as_str(),
            "--confirm" | "--advance" | "--json" | "--jsonl"
        );
        if argument.starts_with("--") && !flag {
            let value = args.next().with_context(|| {
                t!("agent_center.missing_argument", argument = &argument).into_owned()
            })?;
            if matches!(
                argument.as_str(),
                "--input-json" | "--params-json" | "--request-json"
            ) {
                if payload_seen {
                    anyhow::bail!(
                        "{}",
                        t!(
                            "agent_center.duplicate_argument",
                            argument = "--input-json / --params-json"
                        )
                    );
                }
                payload_seen = true;
            }
            if argument == "--input-json" {
                let text = read_json_source(&value, allow_stdin)
                    .await
                    .with_context(|| {
                        t!("agent_center.request_failed", method = "--input-json").into_owned()
                    })?;
                let document: Value =
                    serde_json::from_str(&text).context("Parsing JSON input document")?;
                let request = document.as_object().is_some_and(|object| {
                    [
                        "method",
                        "params",
                        "ifMatch",
                        "commandId",
                        "requestId",
                        "type",
                    ]
                    .iter()
                    .any(|key| object.contains_key(*key))
                });
                resolved.push(
                    if request {
                        "--request-json"
                    } else {
                        "--params-json"
                    }
                    .into(),
                );
                resolved.push(text);
            } else {
                resolved.push(argument);
                resolved.push(value);
            }
        } else {
            resolved.push(argument);
        }
    }
    Ok(resolved)
}

async fn read_json_source(source: &str, allow_stdin: bool) -> Result<String> {
    let mut text = String::new();
    if source == "-" {
        if !allow_stdin {
            anyhow::bail!("--input-json - cannot share stdin with Console keyboard input; use a JSON file or --params-json");
        }
        tokio::io::stdin()
            .take(MAX_JSON_INPUT_BYTES + 1)
            .read_to_string(&mut text)
            .await
            .context("Reading JSON from stdin")?;
    } else {
        tokio::fs::File::open(source)
            .await
            .with_context(|| format!("Opening JSON input file {source:?}"))?
            .take(MAX_JSON_INPUT_BYTES + 1)
            .read_to_string(&mut text)
            .await
            .with_context(|| format!("Reading JSON input file {source:?}"))?;
    }
    if text.len() as u64 > MAX_JSON_INPUT_BYTES {
        anyhow::bail!("JSON input exceeds the 1 MiB protocol limit");
    }
    if text.starts_with('\u{feff}') {
        text.remove(0);
    }
    Ok(text)
}

pub fn status_exit_code(response: &Value) -> i32 {
    match response["status"].as_str() {
        Some("ok") => 0,
        Some("pending") => 2,
        Some("needs_input") => 3,
        Some("conflict") => 4,
        Some("unsupported") => 5,
        _ => 1,
    }
}

pub fn local_response(status: &str, code: &str, message: String) -> Value {
    json!({
        "type":"response", "requestId":uuid::Uuid::new_v4().to_string(),
        "status":status, "subjects":[],
        "failure":{"code":code,"message":message,"fieldErrors":[],
            "recovery":"none","subjects":[]},
    })
}

pub async fn send(client: &Client, operation: &Operation) -> Result<Value> {
    let request = serde_json::from_value(operation.envelope())
        .with_context(|| t!("agent_center.invalid_request").into_owned())?;
    let response = client.request(request).await.with_context(|| {
        t!("agent_center.request_failed", method = &operation.method).into_owned()
    })?;
    serde_json::to_value(response).with_context(|| t!("agent_center.invalid_response").into_owned())
}

pub fn new_context() -> CommandContext {
    CommandContext {
        conversation_id: uuid::Uuid::new_v4().to_string(),
        console_session_id: uuid::Uuid::new_v4().to_string(),
        context_version: 1,
        ..Default::default()
    }
}

fn print_response(response: &Value) -> Result<()> {
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    serde_json::to_writer(&mut output, response)?;
    writeln!(output)?;
    output.flush()?;
    Ok(())
}

/// Structured callers never infer a target from shell or window focus.
pub async fn run_command_async(args: Vec<String>) -> Result<i32> {
    let context = new_context();
    let action = match resolve_input_args(args, true).await {
        Ok(args) => commands::compile(&args, &context, false),
        Err(error) => Err(error),
    };
    let response = match action {
        Err(error) => local_response("error", "INVALID_ARGUMENT", format!("{error:#}")),
        Ok(Action::Help(prefix)) => json!({
            "type":"response", "requestId":uuid::Uuid::new_v4().to_string(),
            "status":"ok", "subjects":[], "data":commands::help(&prefix),
        }),
        Ok(Action::Operation(operation)) => {
            if operation.mutation {
                eprintln!("commandId={}", operation.command_id);
            }
            let result: Result<Value> = async {
                let client = Client::connect()
                    .await
                    .with_context(|| t!("agent_center.service_unavailable").into_owned())?;
                let response = send(&client, &operation).await?;
                if operation.method == "events.subscribe" && status_exit_code(&response) == 0 {
                    print_response(&response)?;
                    loop {
                        tokio::select! {
                            event = client.next_event() => {
                                let event = event?;
                                if event["type"] == "stream_error" { return Ok(event); }
                                print_response(&event)?;
                            }
                            cancelled = tokio::signal::ctrl_c() => {
                                cancelled?;
                                return send(&client, &Operation::read("events.unsubscribe",
                                    json!({"subscriptionId":response["data"]["subscriptionId"]}))).await;
                            }
                        }
                    }
                }
                Ok(response)
            }
            .await;
            match result {
                Ok(response) => response,
                Err(error) => {
                    eprintln!("{error:#}");
                    local_response("error", "EXECUTION_FAILED", format!("{error:#}"))
                }
            }
        }
        Ok(Action::PrepareApply(target)) => {
            eprintln!("commandId={}", target.command_id);
            let result: Result<Value> = async {
                let client = Client::connect()
                    .await
                    .with_context(|| t!("agent_center.service_unavailable").into_owned())?;
                match commands::preparation::prepare(&client, &target.work_id, Some(&target))
                    .await?
                {
                    commands::preparation::Preparation::Ready { operation, .. } => {
                        send(&client, &operation).await
                    }
                    commands::preparation::Preparation::Response(response) => Ok(response),
                }
            }
            .await;
            match result {
                Ok(response) => response,
                Err(error) => local_response("error", "EXECUTION_FAILED", format!("{error:#}")),
            }
        }
        Ok(Action::PrepareTransfer(target)) => {
            eprintln!("commandId={}", target.command_id);
            let result: Result<Value> = async {
                let client = Client::connect()
                    .await
                    .with_context(|| t!("agent_center.service_unavailable").into_owned())?;
                match commands::preparation::prepare_transfer(&client, &target).await? {
                    commands::preparation::Preparation::Ready { operation, .. } => {
                        send(&client, &operation).await
                    }
                    commands::preparation::Preparation::Response(response) => Ok(response),
                }
            }
            .await;
            match result {
                Ok(response) => response,
                Err(error) => local_response("error", "EXECUTION_FAILED", format!("{error:#}")),
            }
        }
        Ok(Action::PrepareProposal(intake)) => {
            eprintln!("commandId={}", intake.command_id);
            let result: Result<Value> = async {
                let client = Client::connect()
                    .await
                    .with_context(|| t!("agent_center.service_unavailable").into_owned())?;
                match commands::preparation::prepare_proposal(&client, &intake).await? {
                    commands::preparation::Preparation::Ready { operation, .. } => {
                        send(&client, &operation).await
                    }
                    commands::preparation::Preparation::Response(response) => Ok(response),
                }
            }
            .await;
            match result {
                Ok(response) => response,
                Err(error) => local_response("error", "EXECUTION_FAILED", format!("{error:#}")),
            }
        }
        Ok(Action::Unsupported(name)) => {
            let (code, message) = commands::unsupported_failure(&name);
            local_response("unsupported", code, message)
        }
        Ok(_) => local_response(
            "unsupported",
            "CAPABILITY_UNAVAILABLE",
            t!("agent_center.console_only").into_owned(),
        ),
    };
    print_response(&response)?;
    Ok(status_exit_code(&response))
}

/// Capture authoritative references without allowing older events to rewind guards.
pub fn capture_versions(context: &mut CommandContext, response: &Value) {
    if let Some(data) = response.get("data") {
        capture_view(context, data);
    }
    if let Some(subjects) = response["subjects"].as_array() {
        for subject in subjects {
            capture_reference(context, subject);
        }
    }
    if let Some(input) = response.get("inputRequest") {
        let kind = match input["kind"].as_str() {
            Some("Decision") => "DecisionRequest",
            Some("Intake") => "IntakeRequest",
            Some("Context") => "ContextRequest",
            _ => return,
        };
        capture_reference(
            context,
            &json!({"kind":kind,"id":input["id"],"version":input["version"]}),
        );
    }
    if let Some(changes) = response["changes"].as_array() {
        for change in changes {
            capture_reference(context, &change["subject"]);
        }
    }
}

fn capture_view(context: &mut CommandContext, view: &Value) {
    capture_reference(context, view);
    for key in [
        "work",
        "task",
        "candidate",
        "decision",
        "operation",
        "workspace",
        "proposal",
        "project",
    ] {
        if let Some(record) = view.get(key) {
            capture_reference(context, record);
        }
    }
    for field in ["items", "changeProposals"] {
        if let Some(items) = view.get(field).and_then(Value::as_array) {
            for item in items {
                capture_view(context, item);
            }
        }
    }
}

fn capture_reference(context: &mut CommandContext, reference: &Value) {
    if let (Some(kind), Some(id), Some(version)) = (
        reference["kind"].as_str(),
        reference["id"].as_str(),
        reference["version"].as_u64(),
    ) {
        if version > 0 {
            let current = context
                .versions
                .entry((kind.into(), id.into()))
                .or_default();
            *current = (*current).max(version);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct InputDirectory(std::path::PathBuf);

    impl InputDirectory {
        fn new() -> Self {
            let path = std::path::PathBuf::from("target")
                .join("agent center input tests")
                .join(uuid::Uuid::new_v4().to_string());
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for InputDirectory {
        fn drop(&mut self) {
            if let Err(error) = std::fs::remove_dir_all(&self.0) {
                eprintln!("Input fixture cleanup failed: {error}");
            }
        }
    }

    #[test]
    fn every_status_has_a_distinct_documented_exit_code() {
        for (status, expected) in [
            ("ok", 0),
            ("pending", 2),
            ("needs_input", 3),
            ("conflict", 4),
            ("unsupported", 5),
            ("error", 1),
            ("unknown", 1),
        ] {
            assert_eq!(status_exit_code(&json!({"status":status})), expected);
        }
    }

    #[test]
    fn event_versions_do_not_rewind_or_change_selection() {
        let mut context = new_context();
        context.work_id = Some("foreground".into());
        capture_versions(
            &mut context,
            &json!({"subjects":[{"kind":"Work","id":"background","version":5}]}),
        );
        capture_versions(
            &mut context,
            &json!({"changes":[{"subject":{"kind":"Work","id":"background","version":2}}]}),
        );
        assert_eq!(context.versions[&("Work".into(), "background".into())], 5);
        assert_eq!(context.work_id.as_deref(), Some("foreground"));
    }

    #[test]
    fn raw_read_records_and_candidate_work_pairs_supply_exact_guards() {
        let mut context = new_context();
        capture_versions(
            &mut context,
            &json!({"status":"ok","data":{
                "work":{"kind":"Work","id":"work","version":9},
                "candidate":{"kind":"DeliveryCandidate","id":"candidate","version":4},
            }}),
        );
        capture_versions(
            &mut context,
            &json!({"status":"ok","data":{
                "kind":"DecisionRequest","id":"decision","version":2,
            }}),
        );
        assert_eq!(context.versions[&("Work".into(), "work".into())], 9);
        assert_eq!(
            context.versions[&("DeliveryCandidate".into(), "candidate".into())],
            4
        );
        assert_eq!(
            context.versions[&("DecisionRequest".into(), "decision".into())],
            2
        );
        capture_versions(
            &mut context,
            &json!({"data":{"changeProposals":[
            {"kind":"ChangeProposal","id":"proposal","version":3}]}}),
        );
        assert_eq!(
            context.versions[&("ChangeProposal".into(), "proposal".into())],
            3
        );
    }

    #[tokio::test]
    async fn input_json_file_uses_spaced_windows_path_and_freezes_content() {
        let _locale = crate::test_support::lock_locale();
        let directory = InputDirectory::new();
        let relative = directory.0.join("project settings.json");
        let source = std::env::current_dir()
            .unwrap()
            .join(&relative)
            .to_string_lossy()
            .into_owned();
        assert!(source.contains(' '));
        let payload = json!({"name":"file project","root":std::env::current_dir().unwrap().to_string_lossy(),
            "coordinatorCapabilityId":"fixture-agent","workerCapabilityId":"fixture-agent","checkCapabilityId":"native-check",
            "limits":{"concurrency":2,"executionAttempts":4,"evaluationAttempts":4,"coordinationTurns":4,
                "contextRounds":2,"executionSeconds":60,"coordinationSeconds":60}});
        tokio::fs::write(&relative, format!("\u{feff}{payload}"))
            .await
            .unwrap();
        let args = resolve_input_args(
            vec![
                "project".into(),
                "configure".into(),
                "--input-json".into(),
                source,
                "--confirm".into(),
            ],
            true,
        )
        .await
        .unwrap();
        assert_eq!(args[2], "--params-json");
        tokio::fs::write(&relative, "{}").await.unwrap();
        let Action::Operation(operation) = commands::compile(&args, &new_context(), false).unwrap()
        else {
            panic!("expected a project configuration");
        };
        assert_eq!(operation.params, payload);
    }

    #[tokio::test]
    async fn input_json_request_file_preserves_retry_identity_and_exact_guards() {
        let _locale = crate::test_support::lock_locale();
        let directory = InputDirectory::new();
        let path = directory.0.join("start request.json");
        let document = json!({"method":"work.start",
            "commandId":"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
            "ifMatch":[{"kind":"Work","id":"work-a","version":7}],
            "params":{"workId":"work-a","specRevision":2,"projectPolicyRevision":3,"grantProposalId":"grant-a"}});
        tokio::fs::write(&path, document.to_string()).await.unwrap();
        let args = vec![
            "work".into(),
            "start".into(),
            "work-a".into(),
            "--input-json".into(),
            std::env::current_dir()
                .unwrap()
                .join(path)
                .to_string_lossy()
                .into_owned(),
            "--json".into(),
        ];
        let mut operations = Vec::new();
        for _ in 0..2 {
            let resolved = resolve_input_args(args.clone(), true).await.unwrap();
            assert_eq!(resolved[3], "--request-json");
            let Action::Operation(operation) =
                commands::compile(&resolved, &new_context(), false).unwrap()
            else {
                panic!("expected work.start");
            };
            operations.push(operation);
        }

        assert_eq!(
            operations[0].command_id,
            document["commandId"].as_str().unwrap()
        );
        assert_eq!(operations[0].command_id, operations[1].command_id);
        assert_ne!(
            operations[0].envelope()["requestId"],
            operations[1].envelope()["requestId"]
        );
        assert_eq!(operations[0].params, document["params"]);
        assert_eq!(
            operations[0].if_match,
            document["ifMatch"].as_array().unwrap().to_vec()
        );
        let mut conflict = args;
        conflict[2] = "other-work".into();
        let resolved = resolve_input_args(conflict, true).await.unwrap();
        assert!(commands::compile(&resolved, &new_context(), false).is_err());
    }

    #[tokio::test]
    async fn transfer_file_is_read_once_and_preserves_exact_summary_through_compilation() {
        let _locale = crate::test_support::lock_locale();
        let directory = InputDirectory::new();
        let path = directory.0.join("handback request.json");
        let document = json!({"method":"workspace.handback",
            "commandId":"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
            "ifMatch":[{"kind":"Workspace","id":"file-workspace","version":8}],
            "params":{"workspaceId":"file-workspace","summary":"  改动\r\n\tquoted \"input\"  ","resumeAffected":true}});
        let source = format!(
            "\u{feff}  {}\r\n",
            serde_json::to_string_pretty(&document).unwrap()
        );
        tokio::fs::write(&path, source.as_bytes()).await.unwrap();
        let args = resolve_input_args(
            vec![
                "workspace".into(),
                "handback".into(),
                "--input-json".into(),
                path.to_string_lossy().into_owned(),
            ],
            false,
        )
        .await
        .unwrap();
        assert_eq!(tokio::fs::read(&path).await.unwrap(), source.as_bytes());
        tokio::fs::write(&path, "{}").await.unwrap();
        let context = CommandContext {
            work_id: Some("unrelated".into()),
            ..new_context()
        };
        for _ in 0..2 {
            let Action::Operation(operation) = commands::compile(&args, &context, true).unwrap()
            else {
                panic!("request files must bypass selected-work preparation");
            };
            assert_eq!(operation.params, document["params"]);
            assert_eq!(
                operation.if_match,
                document["ifMatch"].as_array().unwrap().to_vec()
            );
            assert_eq!(
                operation.command_id,
                document["commandId"].as_str().unwrap()
            );
            assert!(operation.confirmation);
        }
    }

    #[tokio::test]
    async fn guided_transfer_cli_resolves_into_preparation_with_explicit_authority() {
        let _locale = crate::test_support::lock_locale();
        let command_id = uuid::Uuid::new_v4().to_string();
        for handback in [false, true] {
            let mut args = vec![
                "workspace".into(),
                if handback { "handback" } else { "takeover" }.into(),
                "--work".into(),
                "work-a".into(),
                "--work-version".into(),
                "7".into(),
                "--version".into(),
                "8".into(),
                "--command-id".into(),
                command_id.clone(),
            ];
            if handback {
                args.extend([
                    "--summary".into(),
                    "  exact summary\r\n".into(),
                    "--resume-affected".into(),
                    "false".into(),
                ]);
            }
            let mut resolved = resolve_input_args(args, true).await.unwrap();
            assert!(commands::compile(&resolved, &new_context(), false).is_err());
            resolved.push("--confirm".into());
            let Action::PrepareTransfer(target) =
                commands::compile(&resolved, &new_context(), false).unwrap()
            else {
                panic!("expected CLI transfer preparation");
            };
            assert_eq!(target.work_id, "work-a");
            assert_eq!(target.work_version, Some(7));
            assert_eq!(target.workspace_version, Some(8));
            assert_eq!(target.command_id, command_id);
            assert_eq!(
                target.handback,
                handback.then(|| ("  exact summary\r\n".into(), false))
            );
        }
    }

    #[tokio::test]
    async fn input_json_work_revision_remains_the_explicit_typed_operation() {
        let _locale = crate::test_support::lock_locale();
        let directory = InputDirectory::new();
        let path = directory.0.join("revision request.json");
        let payload = json!({"workId":"work-a","replacementSpec":{"revision":3,"goal":"Caller-authored goal"},
            "affectedTaskIds":[],"reason":"Caller-authored reason"});
        let command_id = uuid::Uuid::new_v4().to_string();
        for document in [
            payload.clone(),
            json!({"method":"work.propose_change","params":payload,
                "commandId":command_id,"ifMatch":[{"kind":"Work","id":"work-a","version":7}]}),
        ] {
            tokio::fs::write(&path, document.to_string()).await.unwrap();
            let args = resolve_input_args(
                vec![
                    "work".into(),
                    "revise".into(),
                    "--work".into(),
                    "work-a".into(),
                    "--version".into(),
                    "7".into(),
                    "--input-json".into(),
                    path.to_string_lossy().into_owned(),
                ],
                true,
            )
            .await
            .unwrap();
            let Action::Operation(operation) =
                commands::compile(&args, &new_context(), false).unwrap()
            else {
                panic!("explicit JSON must not become conversation intake");
            };
            assert_eq!(operation.method, "work.propose_change");
            assert_eq!(operation.params, payload);
            assert_eq!(
                operation.if_match,
                vec![json!({"kind":"Work","id":"work-a","version":7})]
            );
            if document.get("commandId").is_some() {
                assert_eq!(operation.command_id, command_id);
            }
        }
    }

    #[test]
    fn input_json_dash_reads_actual_process_stdin() {
        let _locale = crate::test_support::lock_locale();
        const CHILD: &str = "WTA_AGENT_CENTER_STDIN_READER_TEST";
        if std::env::var_os(CHILD).is_some() {
            let args = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(resolve_input_args(
                    vec![
                        "work".into(),
                        "start".into(),
                        "work-a".into(),
                        "--input-json".into(),
                        "-".into(),
                        "--json".into(),
                    ],
                    true,
                ))
                .unwrap();
            let Action::Operation(operation) =
                commands::compile(&args, &new_context(), false).unwrap()
            else {
                panic!("expected work.start from stdin");
            };
            assert_eq!(operation.command_id, "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa");
            assert_eq!(operation.params["workId"], "work-a");
            assert_eq!(operation.if_match[0]["version"], 7);
            println!("STDIN_PAYLOAD_OK");
            return;
        }
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "agent_center::client::tests::input_json_dash_reads_actual_process_stdin",
                "--exact",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        {
            let mut stdin = child.stdin.take().unwrap();
            stdin.write_all(br#"{"method":"work.start","commandId":"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa","ifMatch":[{"kind":"Work","id":"work-a","version":7}],"params":{"workId":"work-a","specRevision":2,"projectPolicyRevision":3,"grantProposalId":"grant-a"}}"#).unwrap();
        }
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("STDIN_PAYLOAD_OK"));
    }

    #[tokio::test]
    async fn input_sources_are_explicit_and_console_does_not_read_keyboard_stdin() {
        let _locale = crate::test_support::lock_locale();
        let inline = r#"{"name":"inline"}"#;
        let args = vec![
            "project".into(),
            "configure".into(),
            "--params-json".into(),
            inline.into(),
        ];
        assert_eq!(resolve_input_args(args.clone(), false).await.unwrap(), args);
        assert!(resolve_input_args(
            vec![
                "project".into(),
                "configure".into(),
                "--input-json".into(),
                inline.into()
            ],
            true
        )
        .await
        .is_err());
        let error = resolve_input_args(
            vec![
                "project".into(),
                "configure".into(),
                "--input-json".into(),
                "-".into(),
            ],
            false,
        )
        .await
        .unwrap_err();
        assert!(format!("{error:#}").contains("stdin"));
    }
}
