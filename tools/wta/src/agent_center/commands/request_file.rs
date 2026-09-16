// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use super::{Arguments, Definition, Operation};
use crate::agent_center::wire::Request;
use anyhow::{bail, ensure, Context, Result};
use serde_json::{json, Value};

/// A complete request supplies its own targets and guards. CLI arguments are
/// assertions against that request, never overrides or selected-work defaults.
pub(super) fn compile(
    definition: &Definition,
    mut arguments: Arguments,
    source: &str,
    explicit_command_id: Option<String>,
    confirmed: bool,
    interactive: bool,
) -> Result<Operation> {
    let method = definition
        .method
        .context("This command does not accept a service request file")?;
    let mut document: Value = serde_json::from_str(source).context("Parsing request document")?;
    let object = document
        .as_object_mut()
        .context("A request document must be an object")?;
    for key in object.keys() {
        ensure!(
            matches!(
                key.as_str(),
                "type" | "requestId" | "commandId" | "method" | "ifMatch" | "params"
            ),
            "Unknown request field {key}"
        );
    }
    object
        .entry("requestId")
        .or_insert_with(|| json!(uuid::Uuid::new_v4().to_string()));
    let request: Request = serde_json::from_value(document).context("Invalid request document")?;
    ensure!(
        request.message_type == "request",
        "The document type must be request"
    );
    uuid::Uuid::parse_str(&request.request_id).context("Invalid requestId")?;
    ensure!(
        request.method == method,
        "Request method {} does not match {}",
        request.method,
        definition.name
    );
    ensure!(
        request.params.is_object(),
        "Request params must be an object"
    );
    let command_id = if definition.mutation {
        let id = request
            .command_id
            .as_ref()
            .context("Mutation request files require a stable commandId")?;
        uuid::Uuid::parse_str(id).context("Invalid commandId")?;
        if let Some(explicit) = explicit_command_id {
            ensure!(
                explicit == *id,
                "CLI --command-id conflicts with request commandId"
            );
        }
        id.clone()
    } else {
        ensure!(
            request.command_id.is_none() && explicit_command_id.is_none(),
            "Read requests must not contain commandId"
        );
        String::new()
    };
    let params = &request.params;
    let expected: Vec<(&str, Option<&str>)> = match method {
        "work.start" | "work.control" | "work.propose_change" | "grant.preview" => {
            vec![("Work", Some(id(params, "workId")?))]
        }
        "workspace.takeover" | "workspace.handback" => {
            vec![("Workspace", Some(id(params, "workspaceId")?))]
        }
        "decision.answer" => vec![("DecisionRequest", Some(id(params, "decisionId")?))],
        "conversation.answer_input" => vec![("IntakeRequest", Some(id(params, "requestId")?))],
        "delivery.accept" | "delivery.request_changes" => vec![
            ("Work", None),
            ("DeliveryCandidate", Some(id(params, "candidateId")?)),
        ],
        "work.apply_change" => vec![
            ("Work", None),
            ("ChangeProposal", Some(id(params, "proposalId")?)),
        ],
        _ => vec![],
    };
    ensure!(
        request.if_match.len() == expected.len(),
        "Request ifMatch must contain the exact required entity guards"
    );
    for (kind, target) in &expected {
        let matching: Vec<_> = request
            .if_match
            .iter()
            .filter(|guard| guard.kind == *kind)
            .collect();
        ensure!(
            matching.len() == 1,
            "Request ifMatch requires exactly one {kind} guard"
        );
        let guard = matching[0];
        ensure!(
            !guard.id.is_empty() && guard.version > 0,
            "Invalid {kind} guard identity or version"
        );
        ensure!(
            target.is_none_or(|id| id == guard.id),
            "Request {kind} guard conflicts with params target"
        );
    }
    let fixed = match definition.name {
        "work pause" => Some(("action", "Hold")),
        "work resume" => Some(("action", "Resume")),
        "work cancel" | "intake cancel" => Some(("action", "Cancel")),
        "intake answer" => Some(("action", "Answer")),
        "work new" => Some(("declaredIntent", "NewWork")),
        "plan revise" => Some(("declaredIntent", "WorkDiscussion")),
        _ => None,
    };
    if let Some((field, expected)) = fixed {
        compare(definition.name, params.get(field), &json!(expected))?;
    }
    if definition.name == "work events" {
        compare(
            "work events scope",
            params.pointer("/scope/kind"),
            &json!("Work"),
        )?;
    }
    if definition.name == "work new" {
        ensure!(
            params
                .pointer("/context/selectedWorkId")
                .is_none_or(Value::is_null),
            "NewWork must not target an existing work"
        );
    }
    let work = params
        .get("workId")
        .or_else(|| params.pointer("/context/selectedWorkId"))
        .or_else(|| {
            if definition.name == "work events" {
                params.pointer("/scope/id")
            } else {
                None
            }
        })
        .cloned()
        .or_else(|| {
            request
                .if_match
                .iter()
                .find(|guard| guard.kind == "Work")
                .map(|guard| json!(guard.id))
        });
    let project = params
        .get("projectId")
        .or_else(|| params.pointer("/context/projectId"));
    let target = match definition.name {
        "work new" | "plan revise" => params.get("text"),
        "work show" | "plan show" | "work events" | "work start" | "work pause" | "work resume"
        | "work cancel" | "work revise" | "grant preview" | "workspace show" => work.as_ref(),
        "work apply" => params.get("proposalId"),
        "task show" => params.get("taskId"),
        "result show" => params.get("resultId"),
        "artifact show" | "evidence show" => params.get("artifactId"),
        "decision show" | "decision answer" => params.get("decisionId"),
        "intake answer" | "intake cancel" => params.get("requestId"),
        "review show" | "review accept" | "review revise" => params.get("candidateId"),
        "workspace get" | "workspace takeover" | "workspace handback" => params.get("workspaceId"),
        "project show" => project,
        "operation show" => params.get("operationId"),
        _ => None,
    };
    if !arguments.positional.is_empty() {
        compare("positional target", target, &json!(arguments.id()?))?;
    }
    for (flag, field) in [
        ("grant", "grantProposalId"),
        ("conversation", "conversationId"),
        ("message-id", "clientMessageId"),
    ] {
        if let Some(value) = arguments.take(flag) {
            compare(flag, params.get(field), &json!(value))?;
        }
    }
    for (flag, value) in [("work", work.as_ref()), ("project", project)] {
        if let Some(explicit) = arguments.take(flag) {
            compare(flag, value, &json!(explicit))?;
        }
    }
    if let Some(after) = arguments.take("after") {
        compare(
            "after",
            params.get(if method == "events.subscribe" {
                "afterCursor"
            } else {
                "afterId"
            }),
            &json!(after),
        )?;
    }
    for (flag, field) in [
        ("spec-revision", "specRevision"),
        (
            "policy-revision",
            if method == "work.start" {
                "projectPolicyRevision"
            } else {
                "policyRevision"
            },
        ),
        ("limit", "limit"),
    ] {
        if let Some(value) = arguments.take(flag) {
            compare(
                flag,
                params.get(field),
                &json!(value
                    .parse::<u64>()
                    .with_context(|| format!("Invalid --{flag}"))?),
            )?;
        }
    }
    for (flag, field) in [
        ("value", "value"),
        ("findings", "findings"),
        ("preserve", "preserveArtifacts"),
    ] {
        if let Some(value) = arguments.take(flag) {
            compare(
                flag,
                params.get(field),
                &serde_json::from_str::<Value>(&value)?,
            )?;
        }
    }
    if arguments.take("advance").is_some() {
        compare("advance", params.get("advance"), &json!(true))?;
    }
    let primary = expected.last().map(|(kind, _)| *kind);
    for (flag, kind) in [
        ("version", primary),
        ("work-version", Some("Work")),
        ("proposal-version", Some("ChangeProposal")),
    ] {
        if let Some(value) = arguments.take(flag) {
            let version = value
                .parse::<u64>()
                .with_context(|| format!("Invalid --{flag}"))?;
            let guard = request
                .if_match
                .iter()
                .find(|guard| Some(guard.kind.as_str()) == kind);
            ensure!(
                guard.is_some_and(|guard| guard.version == version),
                "CLI --{flag} conflicts with request ifMatch"
            );
        }
    }
    arguments.finish()?;
    if method == "work.start" {
        id(params, "grantProposalId")?;
        ensure!(
            params["specRevision"]
                .as_u64()
                .is_some_and(|revision| revision > 0),
            "work.start requires specRevision"
        );
        ensure!(
            params["projectPolicyRevision"]
                .as_u64()
                .is_some_and(|revision| revision > 0),
            "work.start requires projectPolicyRevision"
        );
    }
    if method == "delivery.request_changes" {
        ensure!(
            params["findings"]
                .as_array()
                .is_some_and(|findings| !findings.is_empty()),
            "Revision requests require findings"
        );
    }
    // A structured CLI request is an explicit submission of its captured
    // approval references. Interactive clients still preview consequential acts.
    Ok(Operation {
        method: request.method,
        params: request.params,
        if_match: request
            .if_match
            .into_iter()
            .map(serde_json::to_value)
            .collect::<std::result::Result<_, _>>()?,
        mutation: definition.mutation,
        confirmation: definition.confirmation && interactive && !confirmed,
        command_id,
    })
}

fn id<'a>(params: &'a Value, field: &str) -> Result<&'a str> {
    params[field]
        .as_str()
        .filter(|id| !id.is_empty())
        .with_context(|| format!("Request requires {field}"))
}

fn compare(argument: &str, actual: Option<&Value>, expected: &Value) -> Result<()> {
    if actual != Some(expected) {
        bail!("CLI argument {argument} conflicts with the request document");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_center::commands::{self, Action, CommandContext};

    #[test]
    fn transfer_documents_bypass_guidance_and_preserve_fixed_payload_and_guards() {
        let _locale = crate::test_support::lock_locale();
        for command in ["takeover", "handback"] {
            let mut document = json!({"method":format!("workspace.{command}"),
                "commandId":"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
                "ifMatch":[{"kind":"Workspace","id":"file-workspace","version":12}],
                "params":{"workspaceId":"file-workspace"}});
            if command == "handback" {
                document["params"]["summary"] = json!("  exact Unicode: 改动\r\n\t\"quoted\"  ");
                document["params"]["resumeAffected"] = json!(true);
            }
            let original = document.to_string().into_bytes();
            let operation =
                compile_document(&["workspace", command], &document, &[], true).unwrap();
            let retry = compile_document(&["workspace", command], &document, &[], true).unwrap();
            assert_eq!(document.to_string().into_bytes(), original);
            assert_eq!(operation.params, document["params"]);
            assert_eq!(
                operation.if_match,
                document["ifMatch"].as_array().unwrap().to_vec()
            );
            assert_eq!(
                operation.command_id,
                document["commandId"].as_str().unwrap()
            );
            assert_eq!(operation.params, retry.params);
            assert_eq!(operation.command_id, retry.command_id);
            assert!(operation.confirmation);
            assert!(compile_document(
                &["workspace", command],
                &document,
                &["--version", "13"],
                true
            )
            .is_err());
            assert!(compile_document(
                &["workspace", command, "selected-workspace"],
                &document,
                &[],
                true
            )
            .is_err());
        }
    }

    fn start() -> Value {
        json!({"method":"work.start","commandId":"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
            "ifMatch":[{"kind":"Work","id":"work-a","version":7}],
            "params":{"workId":"work-a","specRevision":2,"projectPolicyRevision":3,"grantProposalId":"grant-a"}})
    }

    fn compile_document(
        command: &[&str],
        document: &Value,
        options: &[&str],
        interactive: bool,
    ) -> Result<Operation> {
        let mut args: Vec<String> = command.iter().map(|arg| (*arg).into()).collect();
        args.extend(["--request-json".into(), document.to_string()]);
        args.extend(options.iter().map(|arg| (*arg).into()));
        let context = CommandContext {
            work_id: Some("unrelated".into()),
            ..Default::default()
        };
        match commands::compile(&args, &context, interactive)? {
            Action::Operation(operation) => Ok(operation),
            _ => bail!("Expected operation"),
        }
    }

    #[test]
    fn request_document_preserves_guards_identity_and_explicit_cli_approval() {
        let _locale = crate::test_support::lock_locale();
        let document = start();
        let first =
            compile_document(&["work", "start", "work-a"], &document, &["--json"], false).unwrap();
        let retry = compile_document(&["work", "start", "work-a"], &document, &[], false).unwrap();
        assert_eq!(first.command_id, document["commandId"].as_str().unwrap());
        assert_eq!(first.command_id, retry.command_id);
        assert_ne!(first.envelope()["requestId"], retry.envelope()["requestId"]);
        assert_eq!(first.params, document["params"]);
        assert_eq!(
            first.if_match,
            document["ifMatch"].as_array().unwrap().to_vec()
        );
        assert!(!first.confirmation);
        assert!(
            compile_document(&["work", "start"], &document, &[], true)
                .unwrap()
                .confirmation
        );
    }

    #[test]
    fn request_document_rejects_explicit_argument_and_guard_conflicts() {
        let _locale = crate::test_support::lock_locale();
        for options in [
            vec!["--work", "other"],
            vec!["--version", "8"],
            vec!["--work-version", "8"],
            vec!["--spec-revision", "4"],
            vec!["--policy-revision", "4"],
            vec!["--grant", "other"],
            vec!["--command-id", "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb"],
        ] {
            assert!(
                compile_document(&["work", "start"], &start(), &options, false).is_err(),
                "{options:?}"
            );
        }
        assert!(compile_document(&["work", "start", "other"], &start(), &[], false).is_err());
        assert!(compile_document(
            &["work", "start", "work-a"],
            &start(),
            &["--version", "7", "--work", "work-a", "--grant", "grant-a"],
            false
        )
        .is_ok());
        let mut document = start();
        document["ifMatch"][0]["id"] = json!("other");
        assert!(compile_document(&["work", "start"], &document, &[], false).is_err());
        document["ifMatch"] = json!([]);
        assert!(compile_document(&["work", "start"], &document, &[], false).is_err());
    }

    #[test]
    fn request_document_rejects_method_subaction_and_authority_substitution() {
        let _locale = crate::test_support::lock_locale();
        assert!(compile_document(&["work", "pause"], &start(), &[], false).is_err());
        let mut document = start();
        document["method"] = json!("work.control");
        document["params"] = json!({"workId":"work-a","action":"Cancel"});
        assert!(compile_document(&["work", "pause"], &document, &[], false).is_err());
        assert!(compile_document(&["work", "cancel"], &document, &[], false).is_ok());
        document["actor"] = json!("human");
        assert!(compile_document(&["work", "cancel"], &document, &[], false).is_err());
        document.as_object_mut().unwrap().remove("actor");
        document.as_object_mut().unwrap().remove("commandId");
        assert!(compile_document(&["work", "cancel"], &document, &[], false).is_err());
    }

    #[test]
    fn request_document_supports_decision_and_exact_candidate_review() {
        let _locale = crate::test_support::lock_locale();
        let mut document = start();
        document["method"] = json!("decision.answer");
        document["params"] = json!({"decisionId":"decision-a","value":{"choice":"yes"}});
        document["ifMatch"] = json!([{"kind":"DecisionRequest","id":"decision-a","version":2}]);
        assert!(
            compile_document(&["decision", "answer", "decision-a"], &document, &[], false).is_ok()
        );
        assert!(compile_document(
            &["decision", "answer", "decision-a"],
            &document,
            &["--value", r#"{"choice":"no"}"#],
            false
        )
        .is_err());
        document["method"] = json!("delivery.accept");
        document["params"] = json!({"candidateId":"candidate-a"});
        document["ifMatch"] = json!([{"kind":"Work","id":"work-a","version":7},{"kind":"DeliveryCandidate","id":"candidate-a","version":4}]);
        let op =
            compile_document(&["review", "accept", "candidate-a"], &document, &[], true).unwrap();
        assert!(op.confirmation);
        assert_eq!(op.if_match.len(), 2);
        assert!(compile_document(
            &["review", "accept", "candidate-a"],
            &document,
            &["--work", "other"],
            false
        )
        .is_err());
        document["ifMatch"][1]["id"] = json!("candidate-b");
        assert!(
            compile_document(&["review", "accept", "candidate-a"], &document, &[], false).is_err()
        );
    }
}
