// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use super::*;

fn bound_read(f: &mut Fixture, invocation: &Value, method: &str, params: Value) -> Response {
    f.engine.handle(
        &Principal::Invocation {
            invocation_id: text(invocation, "id").into(),
        },
        Request::new(method, params),
    )
}

fn evidence_tree(f: &mut Fixture, work: &Value) -> Value {
    let relative = id();
    let directory = f.root.join(&relative);
    std::fs::create_dir_all(&directory).unwrap();
    let content = b"{\"stderr\":\"expected 1, actual 0\",\"exitCode\":7}";
    std::fs::write(directory.join("process.json"), content).unwrap();
    std::fs::write(directory.join("binary.bin"), [0xff, 0x00]).unwrap();
    let mut captured = crate::agent_center::runtime::artifacts::capture(
        &f.root,
        &f.root,
        &json!({"kind":"Tree","relativePath":relative}),
    )
    .unwrap();
    captured["artifactKind"] = json!("Tree");
    captured["workId"] = work["id"].clone();
    f.engine.create("Artifact", captured)
}

#[test]
fn handoff_report_body_and_captured_diagnostics_are_bound_readable() {
    let mut f = Fixture::new();
    let work = f.draft("Read actual worker findings and captured check diagnostics");
    f.start(&work);
    let initial = f.coordinator(&work);
    let worker = f.plan(&work, &initial, false);
    f.start_invocation(&worker);
    f.acknowledge(&worker, None);
    let evidence = evidence_tree(&mut f, &work);
    let response = f.command(
        Principal::Invocation { invocation_id: text(&worker, "id").into() },
        "task.report_progress",
        json!({"dispatchId":worker["dispatch"]["id"],"taskRevision":1,
            "activity":"Check found incorrect output","findings":["expected 1, actual 0"],
            "artifacts":[{"artifactId":evidence["id"],"digest":evidence["digest"]}],
            "nextStep":"Revise only the incorrect value",
            "coordinationRequest":{"reason":"Actual check failed","affectedTaskIds":[worker["dispatch"]["taskId"]],"proposedAction":"Revise output"}}),
        vec![],
    );
    assert_eq!(response.status, "ok", "{response:?}");
    let report_id = response.data.unwrap()["reportId"].clone();
    let coordinator = f.coordinator(&work);
    for name in ["progress_get", "artifact_get", "artifact_read"] {
        assert!(values(&coordinator, "availableToolNames").contains(&json!(name)));
    }
    assert!(coordinator["coordinationInput"]["triggerEvents"]
        .as_array()
        .unwrap()
        .iter()
        .any(|trigger| trigger["subject"]["id"] == report_id));
    let report = bound_read(
        &mut f,
        &coordinator,
        "progress.get",
        json!({"reportId":report_id}),
    );
    assert_eq!(report.status, "ok", "{report:?}");
    let report = report.data.unwrap();
    assert_eq!(report["findings"][0], "expected 1, actual 0");
    assert_eq!(report["nextStep"], "Revise only the incorrect value");
    assert_eq!(
        report["coordinationRequest"]["reason"],
        "Actual check failed"
    );
    let list = bound_read(
        &mut f,
        &coordinator,
        "artifact.read",
        json!({"artifactId":evidence["id"]}),
    );
    assert_eq!(list.status, "ok", "{list:?}");
    assert_eq!(list.data.unwrap()["kind"], "Tree");
    let list = bound_read(
        &mut f,
        &coordinator,
        "artifact.read",
        json!({"artifactId":evidence["id"],"limit":160}),
    )
    .data
    .unwrap();
    assert_eq!(list["entries"].as_array().unwrap().len(), 1);
    assert_eq!(list["nextOffset"], 1);
    assert_eq!(list["eof"], false);
    let mut offset = 0;
    let mut diagnostic = String::new();
    loop {
        let page = bound_read(
            &mut f,
            &coordinator,
            "artifact.read",
            json!({
                "artifactId":evidence["id"],"relativePath":"process.json","offset":offset,"limit":7
            }),
        );
        assert_eq!(page.status, "ok", "{page:?}");
        let page = page.data.unwrap();
        let part = page["text"].as_str().unwrap();
        assert!(part.len() <= 7);
        diagnostic.push_str(part);
        if page["eof"] == true {
            break;
        }
        offset = page["nextOffset"].as_u64().unwrap();
    }
    assert_eq!(
        serde_json::from_str::<Value>(&diagnostic).unwrap()["stderr"],
        "expected 1, actual 0"
    );
    let binary = bound_read(
        &mut f,
        &coordinator,
        "artifact.read",
        json!({
            "artifactId":evidence["id"],"relativePath":"binary.bin"
        }),
    )
    .data
    .unwrap();
    assert_eq!(binary["encoding"], "unsupported");
    assert_eq!(binary["kind"], "Binary");
    assert!(binary.get("text").is_none());
    for params in [
        json!({"artifactId":evidence["id"],"relativePath":"..\\outside"}),
        json!({"artifactId":evidence["id"],"relativePath":"not-captured.txt"}),
        json!({"artifactId":evidence["id"],"relativePath":"process.json","offset":10000}),
        json!({"artifactId":evidence["id"],"limit":32769}),
    ] {
        assert_ne!(
            bound_read(&mut f, &coordinator, "artifact.read", params).status,
            "ok"
        );
    }
    let diagnostic_path = Path::new(text(&evidence, "localPath")).join("process.json");
    let mut permissions = std::fs::metadata(&diagnostic_path).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    permissions.set_readonly(false);
    std::fs::set_permissions(&diagnostic_path, permissions).unwrap();
    std::fs::write(diagnostic_path, b"mutated").unwrap();
    assert_ne!(
        bound_read(
            &mut f,
            &coordinator,
            "artifact.read",
            json!({
                "artifactId":evidence["id"],"relativePath":"process.json"
            })
        )
        .status,
        "ok"
    );
}

#[test]
fn handoff_report_and_evidence_reject_cross_work_access() {
    let mut f = Fixture::new();
    let first = f.draft("First work");
    let second = f.draft("Second work");
    f.start(&first);
    let coordinator = f.coordinator(&first);
    let worker = f.plan(&first, &coordinator, false);
    f.start_invocation(&worker);
    f.acknowledge(&worker, None);
    let foreign = evidence_tree(&mut f, &second);
    let report = f.engine.create(
        "ProgressReport",
        json!({"workId":second["id"],"findings":["private"]}),
    );
    for (method, params) in [
        ("progress.get", json!({"reportId":report["id"]})),
        ("artifact.get", json!({"artifactId":foreign["id"]})),
        (
            "artifact.read",
            json!({"artifactId":foreign["id"],"relativePath":"process.json"}),
        ),
    ] {
        let response = bound_read(&mut f, &worker, method, params);
        assert_eq!(response.failure.unwrap().code, "FORBIDDEN");
    }
    let response = f.command(
        Principal::Invocation { invocation_id: text(&worker, "id").into() },
        "task.report_progress",
        json!({"dispatchId":worker["dispatch"]["id"],"taskRevision":1,"activity":"Foreign evidence",
            "findings":[],"artifacts":[{"artifactId":foreign["id"],"digest":foreign["digest"]}],"nextStep":"Reject"}),
        vec![],
    );
    assert_eq!(response.failure.unwrap().code, "FORBIDDEN");
}

#[test]
fn handoff_file_pages_preserve_utf8_and_progress_body_is_bounded() {
    let mut f = Fixture::new();
    let work = f.draft("Bounded report and UTF-8 evidence");
    f.start(&work);
    let coordinator = f.coordinator(&work);
    let worker = f.plan(&work, &coordinator, false);
    f.start_invocation(&worker);
    f.acknowledge(&worker, None);
    let relative = id();
    let directory = f.root.join(&relative);
    std::fs::create_dir_all(&directory).unwrap();
    let content = "aéz";
    let file = directory.join("report.txt");
    std::fs::write(&file, content).unwrap();
    let mut captured = crate::agent_center::runtime::artifacts::capture(
        &f.root,
        &f.root,
        &json!({"kind":"File","relativePath":format!("{relative}\\report.txt")}),
    )
    .unwrap();
    captured["artifactKind"] = json!("File");
    captured["workId"] = work["id"].clone();
    let artifact = f.engine.create("Artifact", captured);
    let page = bound_read(
        &mut f,
        &worker,
        "artifact.read",
        json!({"artifactId":artifact["id"],"limit":2}),
    )
    .data
    .unwrap();
    assert_eq!(page["text"], "a");
    assert_eq!(page["nextOffset"], 1);
    let page = bound_read(
        &mut f,
        &worker,
        "artifact.read",
        json!({"artifactId":artifact["id"],"offset":1,"limit":2}),
    )
    .data
    .unwrap();
    assert_eq!(page["text"], "é");
    assert_eq!(page["nextOffset"], 3);
    assert_ne!(
        bound_read(
            &mut f,
            &worker,
            "artifact.read",
            json!({"artifactId":artifact["id"],"offset":2})
        )
        .status,
        "ok"
    );
    let response = f.command(
        Principal::Invocation { invocation_id: text(&worker,"id").into() },
        "task.report_progress",
        json!({"dispatchId":worker["dispatch"]["id"],"taskRevision":1,
            "activity":"x".repeat(65_537),"findings":[],"artifacts":[],"nextStep":"Capture instead"}),
        vec![],
    );
    assert_eq!(response.status, "error", "{response:?}");
    assert_eq!(response.failure.unwrap().code, "INVALID_ARGUMENT");
    assert!(f
        .engine
        .related("ProgressReport", "workId", text(&work, "id"))
        .is_empty());
}

#[test]
fn handoff_decline_retains_reason_deduplicates_and_releases_as_failed() {
    let mut f = Fixture::new();
    let work = f.draft("Decline must reach the coordinator with its reason");
    f.start(&work);
    let coordinator = f.coordinator(&work);
    let worker = f.plan(&work, &coordinator, false);
    f.start_invocation(&worker);
    let principal = Principal::Invocation {
        invocation_id: text(&worker, "id").into(),
    };
    let mut decline = Request::new(
        "task.acknowledge",
        json!({
            "dispatchId":worker["dispatch"]["id"],"taskRevision":1,"disposition":"Declined",
            "reason":"The assigned contract requires unavailable input evidence"
        }),
    );
    decline.command_id = Some(id());
    let first = f.engine.handle(&principal, decline.clone());
    assert_eq!(first.status, "ok", "{first:?}");
    let retry = f.engine.handle(&principal, decline.clone());
    assert_eq!(first.data, retry.data);
    decline.params["reason"] = json!("Changed intention");
    assert_eq!(
        f.engine.handle(&principal, decline).failure.unwrap().code,
        "COMMAND_ID_REUSED"
    );
    let acknowledgment = f
        .engine
        .record(
            text(first.data.as_ref().unwrap(), "acknowledgmentId"),
            "Acknowledgment",
        )
        .unwrap();
    assert_eq!(
        acknowledgment["reason"],
        "The assigned contract requires unavailable input evidence"
    );
    f.report(
        &worker,
        "TurnEnded",
        json!({"turnNumber":1,"finish":"Cancelled","quiescent":false}),
    );
    let attempt_id = text(&worker["subject"], "id");
    assert_eq!(
        f.engine.record(attempt_id, "Attempt").unwrap()["reservationHeld"],
        true
    );
    assert!(!f
        .engine
        .all("Operation")
        .iter()
        .any(
            |operation| operation["effect"]["method"] == "runtime.release"
                && operation["effect"]["params"]["invocationId"] == worker["id"]
        ));
    let current = f.engine.record(text(&worker, "id"), "Invocation").unwrap();
    f.report(
        &worker,
        "Settled",
        json!({"quiescent":true,"executionIdentity":current["executionIdentity"],
        "completedOperationIds":[current["stopOperationId"]]}),
    );
    let attempt = f.engine.record(attempt_id, "Attempt").unwrap();
    assert_eq!(attempt["state"], "Failed");
    assert_eq!(attempt["endReason"], "ContractDeclined");
    assert_eq!(attempt["declineReason"], acknowledgment["reason"]);
    for field in ["resultId", "evaluationUnitId", "evaluationRound"] {
        assert!(attempt["failureDetails"].get(field).is_none());
    }
    let coordinator = f.coordinator(&work);
    assert_eq!(
        coordinator["coordinationInput"]["snapshot"]["declinedAttempts"][0]["declineReason"],
        acknowledgment["reason"]
    );
    f.report(
        &worker,
        "Settled",
        json!({"quiescent":true,"executionIdentity":current["executionIdentity"],
        "completedOperationIds":[current["stopOperationId"]]}),
    );
    let releases: Vec<_> = f
        .engine
        .all("Operation")
        .into_iter()
        .filter(|operation| {
            operation["effect"]["method"] == "runtime.release"
                && operation["effect"]["params"]["invocationId"] == worker["id"]
        })
        .collect();
    assert_eq!(releases.len(), 1);
    assert_eq!(
        releases[0]["effect"]["params"]["terminalDisposition"],
        "Failed"
    );
    assert_eq!(
        f.engine
            .related("CoordinationTrigger", "workId", text(&work, "id"))
            .iter()
            .filter(|trigger| trigger["reason"] == "ContractDeclined")
            .count(),
        1
    );
    assert!(!f
        .engine
        .related("CoordinationTrigger", "workId", text(&work, "id"))
        .iter()
        .any(|trigger| trigger["reason"] == "MissingSubmission"));
    f.engine
        .complete_effect(
            text(&releases[0], "id"),
            Response::ok(id(), json!({"released":true})),
        )
        .unwrap();
    assert_eq!(
        f.engine.record(attempt_id, "Attempt").unwrap()["reservationHeld"],
        false
    );
}

#[test]
fn handoff_human_cancellation_remains_cancelled() {
    let mut f = Fixture::new();
    let work = f.draft("Human cancellation is not a contract decline");
    f.start(&work);
    let coordinator = f.coordinator(&work);
    let worker = f.plan(&work, &coordinator, false);
    f.start_invocation(&worker);
    f.acknowledge(&worker, None);
    let current = f.engine.record(text(&worker, "id"), "Invocation").unwrap();
    f.engine.stop_invocation(&current, "Cancel").unwrap();
    f.report(
        &worker,
        "TurnEnded",
        json!({"turnNumber":1,"finish":"Cancelled","quiescent":true}),
    );
    assert_eq!(
        f.engine
            .record(text(&worker["subject"], "id"), "Attempt")
            .unwrap()["state"],
        "Cancelled"
    );
    assert_eq!(
        f.engine.record(text(&worker, "id"), "Invocation").unwrap()["terminalDisposition"],
        "Cancelled"
    );
}

#[test]
fn handoff_coordinator_waits_for_its_workspace_before_dispatch() {
    let mut f = Fixture::new();
    let work = f.draft("Coordinator must start in the provisioned work workspace");
    let preview = f.command(
        Principal::Human,
        "grant.preview",
        json!({"workId":work["id"],"specRevision":1,"policyRevision":1}),
        vec![work.clone()],
    );
    assert_eq!(preview.status, "ok", "{preview:?}");
    let started = f.command(
        Principal::Human,
        "work.start",
        json!({"workId":work["id"],"specRevision":1,"projectPolicyRevision":1,
            "grantProposalId":preview.data.unwrap()["grantProposalId"]}),
        vec![work.clone()],
    );
    assert_eq!(started.status, "pending", "{started:?}");
    assert!(f
        .engine
        .related("Invocation", "workId", text(&work, "id"))
        .is_empty());
    f.complete_nonruntime();
    let coordinator = f.coordinator(&work);
    assert_eq!(
        coordinator["coordinationInput"]["snapshot"]["work"]["workspaceId"],
        work["workspaceId"]
    );
    assert_eq!(
        f.engine
            .record(text(&work, "workspaceId"), "Workspace")
            .unwrap()["status"],
        "Ready"
    );
}

const CHECK_ROOT_FAILURE: &str = "command gate requires a pinned Tree or GitCommit input; mutable workspace execution is not evidence";

fn pending_check(f: &mut Fixture) -> (Value, Value, Value) {
    let work = f.draft("Keep actual evaluation preparation diagnostics");
    f.start(&work);
    let coordinator = f.coordinator(&work);
    let worker = f.plan(&work, &coordinator, true);
    f.start_invocation(&worker);
    f.acknowledge(&worker, None);
    let artifact = f.artifact(&work);
    let result = f.submit(&worker, &artifact, None);
    f.finish_release(&worker);
    let check = f
        .engine
        .related("Invocation", "workId", text(&work, "id"))
        .into_iter()
        .find(|invocation| {
            text(&invocation["dispatch"], "kind") == "EvaluateGate"
                && text(invocation, "state") == "Dispatching"
        })
        .unwrap();
    (work, result, check)
}

fn release_observed_failure(f: &mut Fixture, invocation: &Value) {
    let operation = f
        .engine
        .all("Operation")
        .into_iter()
        .find(|operation| {
            text(&operation["effect"], "method") == "runtime.release"
                && operation["effect"]["params"]["invocationId"] == invocation["id"]
        })
        .unwrap();
    f.engine
        .complete_effect(
            text(&operation, "id"),
            Response::ok(id(), json!({"released":true})),
        )
        .unwrap();
}

#[test]
fn evaluation_preparation_failure_reaches_task_result_rework_and_coordinator() {
    let mut f = Fixture::new();
    let (work, result, check) = pending_check(&mut f);
    f.report(
        &check,
        "TurnEnded",
        json!({"turnNumber":1,"finish":"Error","quiescent":true,
        "errorText":CHECK_ROOT_FAILURE}),
    );
    release_observed_failure(&mut f, &check);
    let unit = f
        .engine
        .record(
            text(&check["dispatch"], "evaluationUnitId"),
            "EvaluationUnit",
        )
        .unwrap();
    assert_eq!(unit["failure"], "EXECUTION_FAILED");
    assert_eq!(unit["failureDetails"]["phase"], "BeforeStarted");
    assert_eq!(unit["failureDetails"]["diagnostic"], CHECK_ROOT_FAILURE);
    assert_eq!(unit["failureDetails"]["resultId"], result["id"]);
    assert_eq!(unit["failureDetails"]["evaluationUnitId"], unit["id"]);
    assert_eq!(unit["failureDetails"]["processOutcomeRecorded"], false);
    assert!(f
        .engine
        .related("GateResult", "resultId", text(&result, "id"))
        .is_empty());
    let coordinator = f.coordinator(&work);
    let task = bound_read(
        &mut f,
        &coordinator,
        "task.get",
        json!({"taskId":result["taskId"]}),
    )
    .data
    .unwrap();
    assert_eq!(
        task["evaluations"][0]["failureDetails"]["diagnostic"],
        CHECK_ROOT_FAILURE
    );
    let result_view = bound_read(
        &mut f,
        &coordinator,
        "result.get",
        json!({"resultId":result["id"]}),
    )
    .data
    .unwrap();
    assert_eq!(
        result_view["result"]["evaluationFailures"][0]["diagnostic"],
        CHECK_ROOT_FAILURE
    );
    let rework = &result_view["rework"][0];
    assert_eq!(rework["instruction"]["action"], "ReviseOutput");
    assert_eq!(rework["recovery"]["kind"], "RepairEvaluation");
    assert_eq!(rework["recovery"]["automaticRetry"], false);
    assert!(
        text(&rework["instruction"]["findings"][0], "requestedChange").contains(CHECK_ROOT_FAILURE)
    );
    assert!(
        text(&rework["instruction"]["findings"][0], "requestedChange").contains(text(&unit, "id"))
    );
    assert_eq!(
        coordinator["coordinationInput"]["snapshot"]["evaluationFailures"][0]["diagnostic"],
        CHECK_ROOT_FAILURE
    );
    let current_task = f.engine.record(text(&result, "taskId"), "Task").unwrap();
    let current_result = f.engine.record(text(&result, "id"), "TaskResult").unwrap();
    assert_eq!(
        f.engine
            .related("EvaluationUnit", "resultId", text(&result, "id"))
            .len(),
        1
    );
    let response = f.command(
        Principal::Invocation {
            invocation_id: text(&coordinator, "id").into(),
        },
        "task.rework",
        json!({"taskId":current_task["id"],"resultId":result["id"],
            "reworkId":rework["id"],"action":"ReviseOutput"}),
        vec![current_task.clone(), current_result],
    );
    assert_eq!(response.status, "ok", "{response:?}");
    let replacement = f
        .engine
        .related("Invocation", "workId", text(&work, "id"))
        .into_iter()
        .find(|invocation| invocation["dispatch"]["rework"]["resultId"] == result["id"])
        .unwrap();
    assert_eq!(
        replacement["dispatch"]["taskRevision"],
        check["dispatch"]["taskRevision"]
    );
    assert_eq!(
        replacement["dispatch"]["gateDefinitions"],
        check["dispatch"]["gateDefinitions"]
    );
    assert_eq!(
        replacement["dispatch"]["outputs"],
        check["dispatch"]["outputs"]
    );
    assert_eq!(replacement["dispatch"]["rework"]["action"], "ReviseOutput");
    assert_eq!(
        f.engine.record(text(&result, "id"), "TaskResult").unwrap()["disposition"],
        "ChangesRequested"
    );
    assert_eq!(current_task["gateDefinitions"][0]["required"], true);
    assert!(f.engine.all("DeliveryCandidate").is_empty());
}

#[test]
fn evaluation_explicit_collect_after_runtime_repair_is_legal_and_repeated_errors_are_bounded() {
    let mut f = Fixture::new();
    let (work, result, check) = pending_check(&mut f);
    f.report(
        &check,
        "TurnEnded",
        json!({"turnNumber":1,"finish":"Error","quiescent":true,"errorText":CHECK_ROOT_FAILURE}),
    );
    release_observed_failure(&mut f, &check);
    let coordinator = f.coordinator(&work);
    let view = bound_read(
        &mut f,
        &coordinator,
        "result.get",
        json!({"resultId":result["id"]}),
    )
    .data
    .unwrap();
    assert_eq!(
        view["result"]["evaluationFailures"][0]["diagnostic"],
        CHECK_ROOT_FAILURE
    );
    let rework = &view["rework"][0];
    assert!(values(&rework["recovery"], "allowedActions").contains(&json!("CollectEvidence")));
    let task = f.engine.record(text(&result, "taskId"), "Task").unwrap();
    let current = f.engine.record(text(&result, "id"), "TaskResult").unwrap();
    let response = f.command(
        Principal::Invocation {
            invocation_id: text(&coordinator, "id").into(),
        },
        "task.rework",
        json!({"taskId":task["id"],"resultId":result["id"],
            "reworkId":rework["id"],"action":"CollectEvidence"}),
        vec![task, current],
    );
    assert_eq!(response.status, "ok", "{response:?}");
    let second = f
        .engine
        .related("Invocation", "workId", text(&work, "id"))
        .into_iter()
        .find(|invocation| {
            invocation["dispatch"]["subjectResultId"] == result["id"]
                && invocation["dispatch"]["evaluationRound"] == 2
        })
        .unwrap();
    assert_eq!(second["dispatch"]["inputs"], check["dispatch"]["inputs"]);
    assert_eq!(second["dispatch"]["outputs"], check["dispatch"]["outputs"]);
    assert_eq!(
        second["dispatch"]["gateDefinitions"],
        check["dispatch"]["gateDefinitions"]
    );
    assert_eq!(
        f.engine
            .record(text(rework, "id"), "ReworkInstruction")
            .unwrap()["appliedAction"],
        "CollectEvidence"
    );
    f.report(
        &second,
        "TurnEnded",
        json!({"turnNumber":1,"finish":"Error","quiescent":true,"errorText":CHECK_ROOT_FAILURE}),
    );
    release_observed_failure(&mut f, &second);
    let current = f.engine.record(text(&result, "id"), "TaskResult").unwrap();
    let latest = f
        .engine
        .record(text(&current, "reworkId"), "ReworkInstruction")
        .unwrap();
    assert_eq!(latest["signature"], rework["signature"]);
    assert!(f
        .engine
        .related("AttentionItem", "subjectId", text(&result, "id"))
        .iter()
        .any(|item| item["reason"] == "RepeatedRework" && item["status"] == "Open"));
    let task = f.engine.record(text(&result, "taskId"), "Task").unwrap();
    let blocked = f.command(
        Principal::Invocation {
            invocation_id: text(&coordinator, "id").into(),
        },
        "task.rework",
        json!({"taskId":task["id"],"resultId":result["id"],
            "reworkId":latest["id"],"action":"CollectEvidence"}),
        vec![task, current],
    );
    assert_eq!(blocked.failure.unwrap().code, "BAD_STATE");
    assert_eq!(
        f.engine
            .related("EvaluationUnit", "resultId", text(&result, "id"))
            .len(),
        2
    );
    assert!(f.engine.all("GateResult").is_empty());
}

fn two_required_checks(f: &mut Fixture) -> (Value, Value) {
    let work = f.draft("Deduplicate two independent preparation failures");
    f.start(&work);
    let mut contract = f.contract(&work, "report", true);
    let mut second = contract["gateDefinitions"][0].clone();
    second["id"] = json!("check-b");
    contract["gateDefinitions"]
        .as_array_mut()
        .unwrap()
        .push(second);
    contract["criteria"][0]["requiredEvidence"]
        .as_array_mut()
        .unwrap()
        .push(json!("check-b"));
    let current = f.engine.record(text(&work, "id"), "Work").unwrap();
    let proposed = f.command(
        Principal::Service,
        "plan.propose",
        json!({
        "workId":work["id"],"tasks":[contract],"edges":[],"integrationTaskKey":"report",
        "reason":"Both declared checks must complete"}),
        vec![current],
    );
    assert_eq!(proposed.status, "ok", "{proposed:?}");
    let proposal = f
        .engine
        .record(
            text(proposed.data.as_ref().unwrap(), "proposalId"),
            "PlanProposal",
        )
        .unwrap();
    let current = f.engine.record(text(&work, "id"), "Work").unwrap();
    let applied = f.command(
        Principal::Service,
        "plan.apply",
        json!({"proposalId":proposal["id"]}),
        vec![current, proposal],
    );
    assert_eq!(applied.status, "ok", "{applied:?}");
    f.finish_idle_coordinator(&work);
    let worker = f
        .engine
        .related("Invocation", "workId", text(&work, "id"))
        .into_iter()
        .find(|invocation| invocation["dispatch"]["kind"] == "ProduceResult")
        .unwrap();
    f.start_invocation(&worker);
    f.acknowledge(&worker, None);
    let artifact = f.artifact(&work);
    let result = f.submit(&worker, &artifact, None);
    f.finish_release(&worker);
    (work, result)
}

fn deterministic_check_failures(
    f: &mut Fixture,
    result: &Value,
    round: u64,
    changed: bool,
    evidence_mode: bool,
) {
    let units: Vec<_> = f
        .engine
        .related("EvaluationUnit", "resultId", text(result, "id"))
        .into_iter()
        .filter(|unit| unit["evaluationRound"] == round)
        .collect();
    assert_eq!(units.len(), 2);
    for unit in units {
        let gate = text(&unit, "gateDefinitionId").to_owned();
        let rank = if (gate == "check") == (round == 1) {
            1
        } else {
            2
        };
        let fixed = format!("00000000-0000-4000-8000-{:012}", round * 10 + rank);
        // Assign deterministic generated identities before any runtime observation.
        let old = text(&unit, "id").to_owned();
        f.engine.records.remove(&old);
        f.engine
            .db
            .execute("DELETE FROM records WHERE id=?1", [&old])
            .unwrap();
        let mut replacement = unit.clone();
        replacement["id"] = json!(fixed);
        f.engine.put(replacement);
        let Some(attempt_id) = unit.get("attemptId").and_then(Value::as_str) else {
            assert_eq!(unit["state"], "Queued");
            continue;
        };
        let attempt = f.engine.record(attempt_id, "Attempt").unwrap();
        let mut invocation = f
            .engine
            .record(text(&attempt, "invocationId"), "Invocation")
            .unwrap();
        invocation["dispatch"]["evaluationUnitId"] = json!(fixed);
        f.engine.put(invocation);
        let mut dispatch = f
            .engine
            .record(text(&attempt, "dispatchId"), "TaskDispatch")
            .unwrap();
        dispatch["evaluationUnitId"] = json!(fixed);
        f.engine.put(dispatch);
        for mut operation in f.engine.all("Operation") {
            if operation["effect"]["method"] == "runtime.invoke"
                && operation["effect"]["params"]["invocation"]["dispatch"]["evaluationUnitId"]
                    == old
            {
                operation["effect"]["params"]["invocation"]["dispatch"]["evaluationUnitId"] =
                    json!(fixed);
                f.engine
                    .db
                    .execute(
                        "UPDATE effects SET body=?1 WHERE id=?2 AND state='Pending'",
                        rusqlite::params![
                            serde_json::to_string(&operation["effect"]["params"]).unwrap(),
                            text(&operation, "id")
                        ],
                    )
                    .unwrap();
                f.engine.put(operation);
            }
        }
    }
    let ordered: Vec<_> = f
        .engine
        .related("EvaluationUnit", "resultId", text(result, "id"))
        .into_iter()
        .filter(|unit| unit["evaluationRound"] == round)
        .collect();
    assert_eq!(
        ordered[0]["gateDefinitionId"],
        if round == 1 { "check" } else { "check-b" }
    );
    for _ in 0..ordered.len() {
        let unit = f
            .engine
            .related("EvaluationUnit", "resultId", text(result, "id"))
            .into_iter()
            .find(|unit| unit["evaluationRound"] == round && unit["state"] == "Running")
            .expect("settling the admitted check must allow the scheduler to admit its successor");
        let attempt = f
            .engine
            .record(text(&unit, "attemptId"), "Attempt")
            .unwrap();
        let invocation = f
            .engine
            .record(text(&attempt, "invocationId"), "Invocation")
            .unwrap();
        let diagnostic = if changed && !evidence_mode && unit["gateDefinitionId"] == "check" {
            "New preparation finding: the repaired snapshot has a different missing dependency"
        } else if unit["gateDefinitionId"] == "check" {
            "First check cannot prepare the pinned root"
        } else {
            "Second check cannot read the captured test script"
        };
        if evidence_mode {
            f.start_invocation(&invocation);
            f.acknowledge(&invocation, None);
            let work = f.engine.record(text(result, "workId"), "Work").unwrap();
            let evidence = if changed && unit["gateDefinitionId"] == "check" {
                let artifact = evidence_tree(f, &work);
                json!({"artifactId":artifact["id"],"digest":artifact["digest"]})
            } else {
                f.artifact(&work)
            };
            let dispatch = &invocation["dispatch"];
            let submitted = f.command(
                Principal::Invocation { invocation_id:text(&invocation,"id").into() },
                "gate.submit",json!({"evaluationUnitId":unit["id"],"dispatchId":dispatch["id"],
                    "taskRevision":dispatch["taskRevision"],"subjectResultId":result["id"],
                    "evaluationRound":round,"gateDefinitionId":unit["gateDefinitionId"],
                    "gateDefinitionRevision":1,"inputManifestDigest":dispatch["inputManifestDigest"],
                    "outcome":"Inconclusive","evidence":[evidence],"explanation":diagnostic}),vec![]);
            assert_eq!(submitted.status, "ok", "{submitted:?}");
            f.finish_release(&invocation);
        } else {
            f.report(
                &invocation,
                "TurnEnded",
                json!({"turnNumber":1,"finish":"Error","quiescent":true,"errorText":diagnostic}),
            );
            release_observed_failure(f, &invocation);
        }
    }
}

#[test]
fn evaluation_two_gate_rework_ignores_uuid_order_but_keeps_new_diagnostics_and_evidence() {
    for (changed, evidence_mode) in [(false, false), (true, false), (false, true), (true, true)] {
        let mut f = Fixture::new();
        let (_, result) = two_required_checks(&mut f);
        deterministic_check_failures(&mut f, &result, 1, false, evidence_mode);
        let current = f.engine.record(text(&result, "id"), "TaskResult").unwrap();
        let first = f
            .engine
            .record(text(&current, "reworkId"), "ReworkInstruction")
            .unwrap();
        let task = f.engine.record(text(&result, "taskId"), "Task").unwrap();
        let retry = f.command(Principal::Service,"task.rework",json!({
            "taskId":task["id"],"resultId":result["id"],"reworkId":first["id"],"action":"CollectEvidence"
        }),vec![task,current]);
        assert_eq!(retry.status, "ok", "{retry:?}");
        deterministic_check_failures(&mut f, &result, 2, changed, evidence_mode);
        let current = f.engine.record(text(&result, "id"), "TaskResult").unwrap();
        let second = f
            .engine
            .record(text(&current, "reworkId"), "ReworkInstruction")
            .unwrap();
        assert_eq!(first["signature"] != second["signature"], changed);
        assert!(
            text(&first["instruction"]["findings"][0], "requestedChange").contains("First check")
        );
        assert!(
            text(&second["instruction"]["findings"][0], "requestedChange").contains("Second check")
        );
        let repeated = f
            .engine
            .related("AttentionItem", "subjectId", text(&result, "id"))
            .iter()
            .any(|item| item["reason"] == "RepeatedRework" && item["status"] == "Open");
        assert_eq!(repeated, !changed);
        let task = f.engine.record(text(&result, "taskId"), "Task").unwrap();
        let next = f.command(Principal::Service,"task.rework",json!({
            "taskId":task["id"],"resultId":result["id"],"reworkId":second["id"],"action":"CollectEvidence"
        }),vec![task,current]);
        if changed {
            assert_eq!(next.status, "ok", "{next:?}");
        } else {
            assert_eq!(next.failure.unwrap().code, "BAD_STATE");
        }
    }
}

#[test]
fn evaluation_runtime_error_is_bounded_and_not_a_process_check_failure() {
    let mut f = Fixture::new();
    let (_, _, check) = pending_check(&mut f);
    f.start_invocation(&check);
    f.acknowledge(&check, None);
    let diagnostic = format!("Capture failed after startup: {}", "é".repeat(3000));
    f.report(
        &check,
        "TurnEnded",
        json!({"turnNumber":1,"finish":"Error","quiescent":true,"errorText":diagnostic}),
    );
    release_observed_failure(&mut f, &check);
    let unit = f
        .engine
        .record(
            text(&check["dispatch"], "evaluationUnitId"),
            "EvaluationUnit",
        )
        .unwrap();
    assert_eq!(unit["failure"], "EXECUTION_FAILED");
    assert_eq!(unit["failureDetails"]["phase"], "AfterStarted");
    assert_eq!(unit["failureDetails"]["diagnosticTruncated"], true);
    assert!(text(&unit["failureDetails"], "diagnostic").len() <= 4096);
    assert!(
        text(&unit["failureDetails"], "diagnostic").starts_with("Capture failed after startup: ")
    );
    assert!(f.engine.all("GateResult").is_empty());
}

#[test]
fn evaluation_cancelled_and_missing_submission_keep_distinct_diagnostics() {
    for (finish, code, action) in [
        ("Cancelled", "CANCELLED", "Replan"),
        ("Normal", "PROTOCOL_INCOMPLETE", "CollectEvidence"),
    ] {
        let mut f = Fixture::new();
        let (_, result, check) = pending_check(&mut f);
        f.start_invocation(&check);
        f.acknowledge(&check, None);
        if finish == "Cancelled" {
            let current = f.engine.record(text(&check, "id"), "Invocation").unwrap();
            f.engine.stop_invocation(&current, "Cancel").unwrap();
        }
        f.report(
            &check,
            "TurnEnded",
            json!({"turnNumber":1,"finish":finish,"quiescent":true}),
        );
        release_observed_failure(&mut f, &check);
        let unit = f
            .engine
            .record(
                text(&check["dispatch"], "evaluationUnitId"),
                "EvaluationUnit",
            )
            .unwrap();
        assert_eq!(unit["failure"], code);
        assert_eq!(unit["failureDetails"]["diagnostic"], "");
        if finish == "Cancelled" {
            assert_eq!(unit["failureDetails"]["stopReason"], "Cancel");
            assert_eq!(
                f.engine
                    .record(text(&check["subject"], "id"), "Attempt")
                    .unwrap()["state"],
                "Cancelled"
            );
        }
        let rework = f
            .engine
            .related("ReworkInstruction", "resultId", text(&result, "id"))
            .pop()
            .unwrap();
        assert_eq!(rework["instruction"]["action"], action);
        assert!(f.engine.all("GateResult").is_empty());
    }
}

#[test]
fn evaluation_actual_nonzero_gate_retains_process_evidence_and_revise_output() {
    let mut f = Fixture::new();
    let (work, result, _) = pending_check(&mut f);
    f.gate(&work, false);
    let gates = f
        .engine
        .related("GateResult", "resultId", text(&result, "id"));
    assert_eq!(gates.len(), 1);
    assert_eq!(gates[0]["outcome"], "Failed");
    assert!(!values(&gates[0], "evidence").is_empty());
    let rework = f
        .engine
        .related("ReworkInstruction", "resultId", text(&result, "id"))
        .pop()
        .unwrap();
    assert_eq!(rework["instruction"]["action"], "ReviseOutput");
    assert_eq!(
        rework["instruction"]["gateResultIds"],
        json!([gates[0]["id"]])
    );
    assert!(rework.get("evaluationFailures").is_none());
    assert!(rework.get("recovery").is_none());
}

fn readonly_integration(f: &mut Fixture) -> (Value, Value, Value, Value) {
    let mut work = f.draft("Verify pinned upstream code and deliver its unchanged snapshot");
    work["spec"]["delivery"]["kind"] = json!("LocalCode");
    let work = f.engine.put(work);
    f.start(&work);
    let mut contribution = f.contract(&work, "code", true);
    contribution["role"] = json!("Contribution");
    contribution["outputs"][0]["kind"] = json!("Tree");
    let mut integration = f.contract(&work, "integration", true);
    integration["resourceRequirements"]["mode"] = json!("ReadOnly");
    integration["inputSlots"] = json!([{"slot":"report","source":{"kind":"Dependency","sourceTaskKey":"code","outputSlot":"report"}}]);
    let current = f.engine.record(text(&work, "id"), "Work").unwrap();
    let response = f.command(Principal::Service,"plan.propose",json!({
        "workId":work["id"],"tasks":[contribution,integration],
        "edges":[{"sourceTaskKey":"code","outputSlot":"report","consumerTaskKey":"integration","condition":"GatePassed","requiredGateIds":["check"]}],
        "integrationTaskKey":"integration","reason":"Read-only verification of the accepted contribution"
    }),vec![current]);
    assert_eq!(response.status, "ok", "{response:?}");
    let proposal = f
        .engine
        .record(
            text(response.data.as_ref().unwrap(), "proposalId"),
            "PlanProposal",
        )
        .unwrap();
    let current = f.engine.record(text(&work, "id"), "Work").unwrap();
    let applied = f.command(
        Principal::Service,
        "plan.apply",
        json!({"proposalId":proposal["id"]}),
        vec![current, proposal],
    );
    assert_eq!(applied.status, "ok", "{applied:?}");
    f.finish_idle_coordinator(&work);
    let worker = f
        .engine
        .related("Invocation", "workId", text(&work, "id"))
        .into_iter()
        .find(|invocation| invocation["dispatch"]["role"] == "Contribution")
        .unwrap();
    f.start_invocation(&worker);
    f.acknowledge(&worker, None);
    let code = evidence_tree(f, &work);
    let reference = json!({"artifactId":code["id"],"digest":code["digest"]});
    let upstream = f.submit(&worker, &reference, None);
    f.finish_release(&worker);
    f.gate(&work, true);
    assert_eq!(
        f.engine
            .record(text(&upstream, "id"), "TaskResult")
            .unwrap()["disposition"],
        "Accepted"
    );
    let consumer = f
        .engine
        .related("Invocation", "workId", text(&work, "id"))
        .into_iter()
        .find(|invocation| invocation["dispatch"]["role"] == "Integration")
        .unwrap();
    (work, upstream, consumer, reference)
}

#[test]
fn evaluation_inputs_keep_original_producer_pins_and_overlapping_output_slots() {
    let mut f = Fixture::new();
    let (work, upstream, consumer, code) = readonly_integration(&mut f);
    let original_inputs = consumer["dispatch"]["inputs"].clone();
    f.start_invocation(&consumer);
    f.acknowledge(&consumer, None);
    let report = f.artifact(&work);
    let result = f.submit(&consumer, &report, None);
    let source_task = f.engine.record(text(&upstream, "taskId"), "Task").unwrap();
    let mut changed_source = source_task.clone();
    changed_source["currentResultId"] = json!(id());
    f.engine.put(changed_source);
    f.finish_release(&consumer);
    let unit = f
        .engine
        .related("EvaluationUnit", "resultId", text(&result, "id"))
        .pop()
        .unwrap();
    let inputs = values(&unit, "inputs");
    assert_eq!(inputs.len(), 2);
    assert!(inputs.contains(&original_inputs[0]));
    assert!(inputs.contains(&json!({"slot":"report","artifact":report,
        "sourceResultId":result["id"],"sourceGateIds":[]})));
    assert_eq!(
        inputs
            .iter()
            .filter(|input| input["slot"] == "report")
            .count(),
        2
    );
    assert_eq!(original_inputs[0]["artifact"], code);
    assert_eq!(original_inputs[0]["sourceResultId"], upstream["id"]);
    assert_eq!(
        unit["inputManifestDigest"],
        digest(&unit["inputs"]).unwrap()
    );
    let evaluation = f
        .engine
        .related("Invocation", "workId", text(&work, "id"))
        .into_iter()
        .find(|invocation| invocation["dispatch"]["subjectResultId"] == result["id"])
        .unwrap();
    assert_eq!(evaluation["dispatch"]["inputs"], unit["inputs"]);
    assert_eq!(
        evaluation["dispatch"]["outputs"],
        consumer["dispatch"]["outputs"]
    );
    assert_eq!(
        evaluation["dispatch"]["inputManifestDigest"],
        unit["inputManifestDigest"]
    );
    f.engine.put(source_task);
}

#[test]
fn readonly_report_integration_delivers_exact_accepted_upstream_code() {
    let mut f = Fixture::new();
    let (work, upstream, consumer, code) = readonly_integration(&mut f);
    f.start_invocation(&consumer);
    f.acknowledge(&consumer, None);
    let report = f.artifact(&work);
    let result = f.submit(&consumer, &report, None);
    f.finish_release(&consumer);
    f.gate(&work, true);
    let current = f.engine.record(text(&work, "id"), "Work").unwrap();
    let candidate = f
        .engine
        .record(text(&current, "currentCandidateId"), "DeliveryCandidate")
        .unwrap();
    assert_eq!(candidate["integrationResultId"], result["id"]);
    assert_eq!(candidate["destination"]["artifact"], code);
    assert_eq!(candidate["destination"]["sourceResultId"], upstream["id"]);
    assert_eq!(
        candidate["destination"]["sourceInput"],
        consumer["dispatch"]["inputs"][0]
    );
    assert_eq!(values(&candidate, "artifacts"), vec![report, code]);
    assert!(candidate["destination"].get("commitId").is_none());
    assert_eq!(
        f.engine.record(text(&result, "id"), "TaskResult").unwrap()["disposition"],
        "Accepted"
    );
    let response = f.command(
        Principal::Human,
        "delivery.accept",
        json!({"candidateId":candidate["id"]}),
        vec![current, candidate],
    );
    assert_eq!(response.status, "ok", "{response:?}");
}

#[test]
fn readonly_delivery_rejects_changed_upstream_provenance_and_mutated_code() {
    let mut f = Fixture::new();
    let (work, upstream, consumer, code) = readonly_integration(&mut f);
    f.start_invocation(&consumer);
    f.acknowledge(&consumer, None);
    let report = f.artifact(&work);
    f.submit(&consumer, &report, None);
    f.finish_release(&consumer);
    f.gate(&work, true);
    let current = f.engine.record(text(&work, "id"), "Work").unwrap();
    let candidate = f
        .engine
        .record(text(&current, "currentCandidateId"), "DeliveryCandidate")
        .unwrap();
    let source_task = f.engine.record(text(&upstream, "taskId"), "Task").unwrap();
    let mut changed_source = source_task.clone();
    changed_source["acceptedResultId"] = json!(id());
    f.engine.put(changed_source);
    let response = f.command(
        Principal::Human,
        "delivery.accept",
        json!({"candidateId":candidate["id"]}),
        vec![current.clone(), candidate.clone()],
    );
    assert_eq!(response.failure.unwrap().code, "STALE_EVALUATION");
    f.engine.put(source_task);
    let record = f
        .engine
        .record(text(&code, "artifactId"), "Artifact")
        .unwrap();
    let path = Path::new(text(&record, "localPath")).join("process.json");
    let mut permissions = std::fs::metadata(&path).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    permissions.set_readonly(false);
    std::fs::set_permissions(&path, permissions).unwrap();
    std::fs::write(path, b"changed after candidate creation").unwrap();
    let response = f.command(
        Principal::Human,
        "delivery.accept",
        json!({"candidateId":candidate["id"]}),
        vec![current, candidate],
    );
    assert_eq!(response.failure.unwrap().code, "ARTIFACT_UNAVAILABLE");
    assert!(f.engine.all("Acceptance").is_empty());
}
