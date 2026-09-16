// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use super::{CommandContext, Operation};
use crate::agent_center::{
    client::{local_response, send, status_exit_code},
    transport::Client,
};
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::future::Future;

#[derive(Clone, Debug)]
pub struct ApplyTarget {
    pub work_id: String,
    pub proposal_id: String,
    pub work_version: Option<u64>,
    pub proposal_version: Option<u64>,
    pub command_id: String,
}

#[derive(Clone, Debug)]
pub struct TransferTarget {
    pub work_id: String,
    pub work_version: Option<u64>,
    pub workspace_version: Option<u64>,
    pub known_versions: std::collections::BTreeMap<(String, String), u64>,
    pub handback: Option<(String, bool)>,
    pub command_id: String,
}

pub async fn prepare_transfer(client: &Client, target: &TransferTarget) -> Result<Preparation> {
    prepare_transfer_with(
        target,
        |operation| async move { send(client, &operation).await },
    )
    .await
}

async fn prepare_transfer_with<F, Fut>(
    target: &TransferTarget,
    mut request: F,
) -> Result<Preparation>
where
    F: FnMut(Operation) -> Fut,
    Fut: Future<Output = Result<Value>>,
{
    let work_response = request(Operation::read(
        "work.get",
        json!({"workId":target.work_id}),
    ))
    .await?;
    if status_exit_code(&work_response) != 0 {
        return Ok(Preparation::Response(work_response));
    }
    let work = &work_response["data"]["work"];
    if work["id"] != target.work_id
        || work["lifecycle"] != "Active"
        || target
            .work_version
            .is_some_and(|version| work["version"] != version)
    {
        return Ok(conflict("Work.id/version/lifecycle"));
    }
    number(work, "version")?;
    let workspace_id = text(work, "workspaceId")?;
    let workspace_read = Operation::read("workspace.get", json!({"workspaceId":workspace_id}));
    let workspace_response = request(workspace_read.clone()).await?;
    if status_exit_code(&workspace_response) != 0 {
        return Ok(Preparation::Response(workspace_response));
    }
    let workspace = &workspace_response["data"];
    let version = number(workspace, "version")?;
    let expected_version = target.workspace_version.or_else(|| {
        target
            .known_versions
            .get(&("Workspace".into(), workspace_id.into()))
            .copied()
    });
    if workspace["id"] != workspace_id
        || workspace["workId"] != target.work_id
        || workspace["status"] != "Ready"
        || workspace["writer"]
            != if target.handback.is_some() {
                "Human"
            } else {
                "None"
            }
        || expected_version.is_some_and(|expected| version != expected)
    {
        return Ok(conflict("Workspace.id/workId/version/status/writer"));
    }
    // Read-only inspection carries the actual current refs, never a candidate
    // inferred from the selected tab or a fabricated workspace location.
    let inspection = request(Operation::read(
        "workspace.inspect",
        json!({"workId":target.work_id}),
    ))
    .await?;
    if status_exit_code(&inspection) != 0 {
        return Ok(Preparation::Response(inspection));
    }
    if inspection["data"]["workspaceId"] != workspace_id {
        return Ok(conflict("Workspace.id"));
    }
    if inspection["data"]["workspace"] != *workspace {
        return Ok(conflict("Workspace.version/writer"));
    }
    let affected_input_contracts = if target.handback.is_some() {
        work_response["data"]["taskSummaries"]
            .as_array()
            .with_context(|| {
                t!("agent_center.missing_field", field = "taskSummaries").into_owned()
            })?
            .iter()
            .filter(|task| task["resourceRequirements"]["workspaceId"] == workspace_id)
            .map(|task| {
                json!({"taskId":task["id"],"revision":task["revision"],
                "planRevision":task["planRevision"],"contract":task["contract"]})
            })
            .collect::<Vec<_>>()
    } else {
        vec![]
    };
    let latest_work = request(Operation::read(
        "work.get",
        json!({"workId":target.work_id}),
    ))
    .await?;
    if status_exit_code(&latest_work) != 0 {
        return Ok(Preparation::Response(latest_work));
    }
    if latest_work["data"] != work_response["data"] {
        return Ok(conflict("Work.version/contracts"));
    }
    let latest_workspace = request(workspace_read).await?;
    if status_exit_code(&latest_workspace) != 0 {
        return Ok(Preparation::Response(latest_workspace));
    }
    if latest_workspace["data"] != *workspace {
        return Ok(conflict("Workspace.version/writer"));
    }
    let mut params = json!({"workspaceId":workspace_id});
    if let Some((summary, resume_affected)) = &target.handback {
        params["summary"] = json!(summary);
        params["resumeAffected"] = json!(resume_affected);
    }
    let operation = Operation {
        method: if target.handback.is_some() {
            "workspace.handback"
        } else {
            "workspace.takeover"
        }
        .into(),
        params,
        if_match: vec![json!({"kind":"Workspace","id":workspace_id,"version":version})],
        mutation: true,
        confirmation: true,
        command_id: target.command_id.clone(),
    };
    let preview = json!({"work":work_response["data"],"workspace":workspace,"inspection":inspection["data"],
        "affectedInputContracts":affected_input_contracts,
        "request":{"method":operation.method,"params":operation.params,"ifMatch":operation.if_match,
            "commandId":operation.command_id}});
    Ok(Preparation::Ready { operation, preview })
}

#[derive(Clone, Debug)]
pub struct ProposalIntake {
    pub context: CommandContext,
    pub text: String,
    pub command_id: String,
    pub client_message_id: String,
}

pub(super) fn intake_identity(command_id: &str, kind: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(format!("wta.agent-center.intake:{kind}:{command_id}"));
    let mut bytes = [0; 16];
    bytes.copy_from_slice(&digest[..16]);
    uuid::Builder::from_custom_bytes(bytes)
        .into_uuid()
        .to_string()
}

pub async fn prepare_proposal(client: &Client, intake: &ProposalIntake) -> Result<Preparation> {
    let work_id = intake
        .context
        .work_id
        .as_ref()
        .with_context(|| t!("agent_center.work_required").into_owned())?;
    let response = send(
        client,
        &Operation::read("work.get", json!({"workId":work_id})),
    )
    .await?;
    proposal_from_work(intake, response)
}

fn proposal_from_work(intake: &ProposalIntake, response: Value) -> Result<Preparation> {
    if status_exit_code(&response) != 0 {
        return Ok(Preparation::Response(response));
    }
    let work = &response["data"]["work"];
    if work["id"].as_str() != intake.context.work_id.as_deref() {
        return Ok(conflict("Work.id"));
    }
    let project = text(work, "projectId")?;
    if intake
        .context
        .project_id
        .as_ref()
        .is_some_and(|captured| captured != project)
    {
        return Ok(conflict("Work.projectId"));
    }
    let mut context = intake.context.clone();
    context.project_id = Some(project.into());
    let mut operation = super::conversation(intake.text.clone(), &context, false)?;
    operation.command_id = intake.command_id.clone();
    operation.params["clientMessageId"] = json!(intake.client_message_id);
    operation.params["declaredIntent"] = json!("ChangeProposal");
    Ok(Preparation::Ready {
        operation,
        preview: response,
    })
}

#[derive(Debug)]
pub enum Preparation {
    Ready {
        operation: Operation,
        preview: Value,
    },
    Response(Value),
}

pub async fn prepare(
    client: &Client,
    work_id: &str,
    apply: Option<&ApplyTarget>,
) -> Result<Preparation> {
    prepare_with(work_id, apply, |operation| async move {
        send(client, &operation).await
    })
    .await
}

async fn prepare_with<F, Fut>(
    work_id: &str,
    apply: Option<&ApplyTarget>,
    mut request: F,
) -> Result<Preparation>
where
    F: FnMut(Operation) -> Fut,
    Fut: Future<Output = Result<Value>>,
{
    let work_response = request(Operation::read("work.get", json!({"workId":work_id}))).await?;
    if status_exit_code(&work_response) != 0 {
        return Ok(Preparation::Response(work_response));
    }
    let data = &work_response["data"];
    let work = &data["work"];
    if work["id"] != work_id {
        return Ok(conflict("Work.id"));
    }
    let version = number(work, "version")?;
    let spec_revision = number(work, "currentSpecRevision")?;
    let project_id = text(work, "projectId")?;
    let proposal = if let Some(target) = apply {
        if target.work_id != work_id
            || target
                .work_version
                .is_some_and(|expected| expected != version)
        {
            return Ok(conflict("Work.version"));
        }
        let proposals = data["changeProposals"].as_array().with_context(|| {
            t!("agent_center.missing_field", field = "changeProposals").into_owned()
        })?;
        let matching: Vec<_> = proposals
            .iter()
            .filter(|proposal| proposal["id"] == target.proposal_id)
            .collect();
        if matching.len() != 1 {
            return Ok(conflict("ChangeProposal.id"));
        }
        let proposal = matching[0];
        if let Some(field) = proposal_conflict(work, proposal, target) {
            return Ok(conflict(field));
        }
        Some(proposal)
    } else {
        None
    };
    let project_response = request(Operation::read(
        "project.get",
        json!({"projectId":project_id}),
    ))
    .await?;
    if status_exit_code(&project_response) != 0 {
        return Ok(Preparation::Response(project_response));
    }
    let project_data = &project_response["data"];
    let project = if project_data["project"].is_object() {
        &project_data["project"]
    } else {
        project_data
    };
    if project["id"] != project_id {
        return Ok(conflict("Project.id"));
    }
    let policy = number(project, "policyRevision")?;
    let guard = json!({"kind":"Work","id":work_id,"version":version});
    let preview = request(Operation {
        method: "grant.preview".into(),
        params: json!({"workId":work_id,"specRevision":spec_revision,"policyRevision":policy}),
        if_match: vec![guard.clone()],
        mutation: true,
        confirmation: false,
        command_id: uuid::Uuid::new_v4().to_string(),
    })
    .await?;
    if status_exit_code(&preview) != 0 {
        return Ok(Preparation::Response(preview));
    }
    let grant_id = text(&preview["data"], "grantProposalId")?;
    let mut guards = vec![guard];
    let (method, params, command_id) = if let (Some(target), Some(proposal)) = (apply, proposal) {
        let grant = &preview["data"]["proposal"];
        for (field, expected) in [
            ("id", json!(grant_id)),
            ("workId", json!(work_id)),
            ("specRevision", json!(spec_revision)),
            ("policyRevision", json!(policy)),
            ("basedOnGrantId", work["currentGrantId"].clone()),
            ("status", json!("Proposed")),
        ] {
            if grant[field] != expected {
                return Ok(conflict(&format!("GrantProposal.{field}")));
            }
        }
        // A proposal may change while policy/grant reads are in flight. Never
        // silently replace either guard with a newer revision at confirmation.
        let latest = request(Operation::read("work.get", json!({"workId":work_id}))).await?;
        if status_exit_code(&latest) != 0 {
            return Ok(Preparation::Response(latest));
        }
        let latest_work = &latest["data"]["work"];
        if latest_work["id"] != work_id || latest_work["version"] != version {
            return Ok(conflict("Work.version"));
        }
        let current_proposals =
            latest["data"]["changeProposals"]
                .as_array()
                .with_context(|| {
                    t!("agent_center.missing_field", field = "changeProposals").into_owned()
                })?;
        let current: Vec<_> = current_proposals
            .iter()
            .filter(|current| current["id"] == target.proposal_id)
            .collect();
        if current.len() != 1 || current[0] != proposal {
            return Ok(conflict("ChangeProposal.version"));
        }
        if let Some(field) = proposal_conflict(latest_work, current[0], target) {
            return Ok(conflict(field));
        }
        guards.push(json!({"kind":"ChangeProposal","id":target.proposal_id,"version":number(proposal,"version")?}));
        (
            "work.apply_change",
            json!({"proposalId":target.proposal_id,"grantProposalId":grant_id}),
            target.command_id.clone(),
        )
    } else {
        (
            "work.start",
            json!({"workId":work_id,"specRevision":spec_revision,
            "projectPolicyRevision":policy,"grantProposalId":grant_id}),
            uuid::Uuid::new_v4().to_string(),
        )
    };
    let operation = Operation {
        method: method.into(),
        params,
        if_match: guards,
        mutation: true,
        confirmation: true,
        command_id,
    };
    let preview = json!({"work":data,"project":project_data,"changeProposal":proposal,
        "grant":preview,"request":{"method":operation.method,"params":operation.params,
            "ifMatch":operation.if_match,"commandId":operation.command_id}});
    Ok(Preparation::Ready { operation, preview })
}

fn proposal_conflict(work: &Value, proposal: &Value, target: &ApplyTarget) -> Option<&'static str> {
    if work["lifecycle"] != "Active" || work["requiresReplan"] == true {
        Some("Work.lifecycle/requiresReplan")
    } else if proposal["workId"] != target.work_id {
        Some("ChangeProposal.workId")
    } else if proposal["status"] != "Proposed" {
        Some("ChangeProposal.status")
    } else if !proposal["version"].as_u64().is_some_and(|version| {
        version > 0
            && target
                .proposal_version
                .is_none_or(|expected| version == expected)
    }) {
        Some("ChangeProposal.version")
    } else if proposal["basedOnSpecRevision"] != work["currentSpecRevision"]
        || proposal["replacementSpec"]["revision"] != work["currentSpecRevision"]
    {
        Some("ChangeProposal.basedOnSpecRevision")
    } else if !work["currentPlanRevision"].is_u64()
        || proposal["basedOnPlanRevision"] != work["currentPlanRevision"]
    {
        Some("ChangeProposal.basedOnPlanRevision")
    } else if !work["currentGrantId"]
        .as_str()
        .is_some_and(|id| !id.is_empty())
        || proposal["grantId"] != work["currentGrantId"]
    {
        Some("ChangeProposal.grantId")
    } else if proposal["replacementSpec"]["projectId"] != work["projectId"] {
        Some("ChangeProposal.replacementSpec.projectId")
    } else {
        None
    }
}

fn conflict(field: &str) -> Preparation {
    Preparation::Response(local_response(
        "conflict",
        "STALE_VERSION",
        format!("{}: {field}", t!("agent_center.status_conflict")),
    ))
}

fn number(value: &Value, field: &str) -> Result<u64> {
    value[field]
        .as_u64()
        .filter(|number| *number > 0)
        .with_context(|| t!("agent_center.missing_field", field = field).into_owned())
}

fn text<'a>(value: &'a Value, field: &str) -> Result<&'a str> {
    value[field]
        .as_str()
        .filter(|text| !text.is_empty())
        .with_context(|| t!("agent_center.missing_field", field = field).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    fn transfer_fixture(handback: bool) -> (TransferTarget, Value, Value, Value) {
        let target = TransferTarget {
            work_id: "work-a".into(),
            work_version: Some(7),
            workspace_version: Some(8),
            known_versions: Default::default(),
            handback: handback.then(|| ("  updated input\r\n".into(), false)),
            command_id: uuid::Uuid::new_v4().to_string(),
        };
        let work = json!({"status":"ok","data":{"work":{"id":"work-a","version":7,"workspaceId":"workspace-a",
            "lifecycle":"Active","desiredAdvancement":"Hold","currentPlanRevision":4},
            "taskSummaries":[
                {"id":"affected","revision":3,"planRevision":4,"resourceRequirements":{"workspaceId":"workspace-a"},
                    "contract":{"inputSlots":[{"slot":"source","source":{"kind":"Artifact","artifact":{"artifactId":"input-v2","digest":"fixed"}}}]}},
                {"id":"unaffected","revision":2,"resourceRequirements":{},
                    "contract":{"inputSlots":[]}}]}});
        let workspace = json!({"status":"ok","data":{"kind":"Workspace","id":"workspace-a","version":8,
            "workId":"work-a","status":"Ready","writer":if handback {"Human"} else {"None"},
            "localRoot":"C:\\managed\\work-a","headGeneration":2}});
        let inspection = json!({"status":"ok","data":{"workspaceId":"workspace-a","workspace":workspace["data"],
            "artifacts":[{"artifactId":"output-v2","digest":"fixed"}],"destination":{"localRoot":"C:\\managed\\work-a"}}});
        (target, work, workspace, inspection)
    }

    #[tokio::test]
    async fn selected_work_transfer_previews_exact_refs_and_affected_contracts_without_resuming_hold(
    ) {
        let _locale = crate::test_support::lock_locale();
        for handback in [false, true] {
            let (target, work, workspace, inspection) = transfer_fixture(handback);
            let mut responses = VecDeque::from([
                work.clone(),
                workspace.clone(),
                inspection.clone(),
                work,
                workspace,
            ]);
            let mut requests = vec![];
            let result = prepare_transfer_with(&target, |operation| {
                requests.push(operation);
                std::future::ready(Ok(responses.pop_front().unwrap()))
            })
            .await
            .unwrap();
            let Preparation::Ready { operation, preview } = result else {
                panic!("expected confirmation")
            };
            assert!(requests.iter().all(|request| !request.mutation));
            assert_eq!(
                requests
                    .iter()
                    .map(|request| request.method.as_str())
                    .collect::<Vec<_>>(),
                [
                    "work.get",
                    "workspace.get",
                    "workspace.inspect",
                    "work.get",
                    "workspace.get"
                ]
            );
            assert_eq!(operation.command_id, target.command_id);
            assert_eq!(operation.params["workspaceId"], "workspace-a");
            assert_eq!(
                operation.if_match,
                [json!({"kind":"Workspace","id":"workspace-a","version":8})]
            );
            assert!(operation.confirmation);
            assert_eq!(preview["inspection"], inspection["data"]);
            assert_eq!(preview["work"]["work"]["desiredAdvancement"], "Hold");
            assert!(operation.params.get("action").is_none());
            if handback {
                assert_eq!(operation.method, "workspace.handback");
                assert_eq!(operation.params["summary"], "  updated input\r\n");
                assert_eq!(operation.params["resumeAffected"], false);
                assert_eq!(
                    preview["affectedInputContracts"].as_array().unwrap().len(),
                    1
                );
                assert_eq!(preview["affectedInputContracts"][0]["taskId"], "affected");
                assert_eq!(
                    preview["affectedInputContracts"][0]["contract"]["inputSlots"][0]["source"]
                        ["artifact"]["artifactId"],
                    "input-v2"
                );
            } else {
                assert_eq!(operation.method, "workspace.takeover");
                assert_eq!(operation.params, json!({"workspaceId":"workspace-a"}));
            }
        }
    }

    #[tokio::test]
    async fn transfer_rejects_stale_identity_version_writer_and_preparation_races() {
        let _locale = crate::test_support::lock_locale();
        for (index, path, value) in [
            (0, "/data/work/id", json!("work-b")),
            (0, "/data/work/version", json!(9)),
            (0, "/data/work/workspaceId", json!("workspace-b")),
            (1, "/data/workId", json!("work-b")),
            (1, "/data/version", json!(9)),
            (1, "/data/status", json!("Capturing")),
            (1, "/data/writer", json!("None")),
            (2, "/data/workspaceId", json!("workspace-b")),
            (2, "/data/workspace/writer", json!("HandingBack")),
            (3, "/data/work/version", json!(9)),
            (
                3,
                "/data/taskSummaries/0/contract/inputSlots/0/source/artifact/digest",
                json!("changed"),
            ),
            (4, "/data/version", json!(9)),
            (4, "/data/writer", json!("HandingBack")),
        ] {
            let (target, work, workspace, inspection) = transfer_fixture(true);
            let mut responses =
                VecDeque::from([work.clone(), workspace.clone(), inspection, work, workspace]);
            *responses[index].pointer_mut(path).unwrap() = value;
            let result = prepare_transfer_with(&target, |request| {
                assert!(!request.mutation);
                std::future::ready(Ok(responses.pop_front().unwrap()))
            })
            .await
            .unwrap();
            assert!(
                matches!(result, Preparation::Response(response) if response["status"] == "conflict"),
                "{index} {path}"
            );
        }
    }

    fn intake_fixture() -> ProposalIntake {
        ProposalIntake {
            context: CommandContext {
                work_id: Some("work-a".into()),
                project_id: None,
                console_session_id: uuid::Uuid::new_v4().to_string(),
                conversation_id: uuid::Uuid::new_v4().to_string(),
                context_version: 6,
                ..Default::default()
            },
            text: "Keep the current output but narrow the requested scope".into(),
            command_id: uuid::Uuid::new_v4().to_string(),
            client_message_id: uuid::Uuid::new_v4().to_string(),
        }
    }

    #[test]
    fn natural_revision_only_submits_change_proposal_intake_with_captured_identities() {
        let _locale = crate::test_support::lock_locale();
        let intake = intake_fixture();
        let response =
            json!({"status":"ok","data":{"work":{"id":"work-a","projectId":"project-a"}}});
        let Preparation::Ready { operation, .. } = proposal_from_work(&intake, response).unwrap()
        else {
            panic!("expected proposal intake");
        };
        assert_eq!(operation.method, "conversation.submit");
        assert_eq!(operation.params["declaredIntent"], "ChangeProposal");
        assert_eq!(operation.params["text"], intake.text);
        assert_eq!(operation.params["context"]["selectedWorkId"], "work-a");
        assert_eq!(operation.params["context"]["projectId"], "project-a");
        assert_eq!(
            operation.params["context"]["consoleSessionId"],
            intake.context.console_session_id
        );
        assert_eq!(operation.params["context"]["contextVersion"], 6);
        assert_eq!(
            operation.params["conversationId"],
            intake.context.conversation_id
        );
        assert_eq!(
            operation.params["clientMessageId"],
            intake.client_message_id
        );
        assert_eq!(operation.command_id, intake.command_id);
        assert!(operation.if_match.is_empty());
        assert!(!operation.confirmation);
        for field in [
            "replacementSpec",
            "criteria",
            "grantProposalId",
            "proposalId",
            "advance",
        ] {
            assert!(operation.params.get(field).is_none(), "{field}");
        }
        assert_eq!(intake.context.project_id, None);
    }

    #[test]
    fn proposal_intake_refuses_conflicting_work_or_captured_project() {
        let _locale = crate::test_support::lock_locale();
        let mut intake = intake_fixture();
        intake.context.project_id = Some("project-a".into());
        for work in [
            json!({"id":"other","projectId":"project-a"}),
            json!({"id":"work-a","projectId":"other"}),
        ] {
            let result =
                proposal_from_work(&intake, json!({"status":"ok","data":{"work":work}})).unwrap();
            assert!(
                matches!(result, Preparation::Response(response) if response["status"] == "conflict")
            );
        }
    }

    fn fixture() -> (ApplyTarget, Value, Value, Value) {
        let target = ApplyTarget {
            work_id: "work-a".into(),
            proposal_id: "change-a".into(),
            work_version: Some(7),
            proposal_version: Some(2),
            command_id: uuid::Uuid::new_v4().to_string(),
        };
        let work = json!({"status":"ok","data":{"work":{"id":"work-a","version":7,"projectId":"project-a",
            "currentSpecRevision":3,"currentPlanRevision":4,"currentGrantId":"grant-current","lifecycle":"Active"},
            "changeProposals":[{"id":"change-a","version":2,"workId":"work-a","status":"Proposed",
                "basedOnSpecRevision":3,"basedOnPlanRevision":4,"grantId":"grant-current",
                "replacementSpec":{"revision":3,"projectId":"project-a","goal":"Requested replacement"},
                "impact":{"allowanceReset":false,"authorityExpansion":false}}]}});
        let project = json!({"status":"ok","data":{"id":"project-a","policyRevision":5}});
        let grant = json!({"status":"ok","data":{"grantProposalId":"grant-preview","proposal":{
            "id":"grant-preview","workId":"work-a","specRevision":3,"policyRevision":5,
            "basedOnGrantId":"grant-current","status":"Proposed","remainingUsage":{"executionAttempts":2}}}});
        (target, work, project, grant)
    }

    #[tokio::test]
    async fn ordinary_apply_previews_current_authority_and_freezes_exact_request() {
        let _locale = crate::test_support::lock_locale();
        let (target, work, project, grant) = fixture();
        let mut responses = VecDeque::from([work.clone(), project, grant.clone(), work]);
        let mut requests = Vec::new();
        let ready = prepare_with(&target.work_id, Some(&target), |operation| {
            requests.push(operation);
            std::future::ready(Ok(responses.pop_front().unwrap()))
        })
        .await
        .unwrap();
        let Preparation::Ready { operation, preview } = ready else {
            panic!("expected confirmation")
        };
        assert_eq!(
            requests
                .iter()
                .map(|request| request.method.as_str())
                .collect::<Vec<_>>(),
            ["work.get", "project.get", "grant.preview", "work.get"]
        );
        assert_eq!(
            requests[2].params,
            json!({"workId":"work-a","specRevision":3,"policyRevision":5})
        );
        assert_eq!(
            requests[2].if_match,
            vec![json!({"kind":"Work","id":"work-a","version":7})]
        );
        assert_eq!(operation.method, "work.apply_change");
        assert_eq!(
            operation.params,
            json!({"proposalId":"change-a","grantProposalId":"grant-preview"})
        );
        assert_eq!(
            operation.if_match,
            vec![
                json!({"kind":"Work","id":"work-a","version":7}),
                json!({"kind":"ChangeProposal","id":"change-a","version":2})
            ]
        );
        assert_eq!(operation.command_id, target.command_id);
        assert!(operation.confirmation);
        assert_eq!(preview["grant"], grant);
        assert_eq!(preview["changeProposal"]["impact"]["allowanceReset"], false);
        let captured = operation.envelope();
        assert_eq!(captured["commandId"], operation.envelope()["commandId"]);
        assert_eq!(preview["request"]["ifMatch"], captured["ifMatch"]);
        assert!(responses.is_empty());
    }

    #[tokio::test]
    async fn stale_or_mismatched_proposals_never_request_a_grant() {
        let _locale = crate::test_support::lock_locale();
        for (path, value) in [
            ("/data/work/id", json!("other")),
            ("/data/work/version", json!(8)),
            ("/data/work/requiresReplan", json!(true)),
            ("/data/changeProposals/0/workId", json!("other")),
            ("/data/changeProposals/0/version", json!(3)),
            ("/data/changeProposals/0/status", json!("Applied")),
            ("/data/changeProposals/0/basedOnSpecRevision", json!(2)),
            ("/data/changeProposals/0/replacementSpec/revision", json!(4)),
            ("/data/changeProposals/0/basedOnPlanRevision", json!(3)),
            ("/data/changeProposals/0/grantId", json!("obsolete")),
            (
                "/data/changeProposals/0/replacementSpec/projectId",
                json!("other"),
            ),
        ] {
            let (target, mut work, _, _) = fixture();
            work["data"]["work"]["requiresReplan"] = json!(false);
            *work.pointer_mut(path).unwrap() = value;
            let mut count = 0;
            let outcome = prepare_with(&target.work_id, Some(&target), |_| {
                count += 1;
                std::future::ready(Ok(work.clone()))
            })
            .await
            .unwrap();
            assert_eq!(count, 1, "{path}");
            assert!(
                matches!(outcome, Preparation::Response(response) if response["status"] == "conflict"),
                "{path}"
            );
        }
    }

    #[tokio::test]
    async fn changes_during_grant_preparation_are_not_silently_refreshed() {
        let _locale = crate::test_support::lock_locale();
        for path in ["/data/work/version", "/data/changeProposals/0/version"] {
            let (target, work, project, grant) = fixture();
            let mut latest = work.clone();
            *latest.pointer_mut(path).unwrap() = json!(99);
            let mut responses = VecDeque::from([work, project, grant, latest]);
            let outcome = prepare_with(&target.work_id, Some(&target), |_| {
                std::future::ready(Ok(responses.pop_front().unwrap()))
            })
            .await
            .unwrap();
            assert!(
                matches!(outcome, Preparation::Response(response) if response["status"] == "conflict")
            );
        }
    }

    #[tokio::test]
    async fn start_uses_the_same_current_policy_preview_without_change_guards() {
        let _locale = crate::test_support::lock_locale();
        let (_, work, project, grant) = fixture();
        let mut responses = VecDeque::from([work, project, grant]);
        let outcome = prepare_with("work-a", None, |_| {
            std::future::ready(Ok(responses.pop_front().unwrap()))
        })
        .await
        .unwrap();
        let Preparation::Ready { operation, .. } = outcome else {
            panic!("expected start preview")
        };
        assert_eq!(operation.method, "work.start");
        assert_eq!(operation.params["specRevision"], 3);
        assert_eq!(operation.params["projectPolicyRevision"], 5);
        assert_eq!(operation.if_match.len(), 1);
    }
}
