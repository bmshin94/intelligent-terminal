// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use super::*;

fn inspect(f: &mut Fixture, principal: Principal, params: Value) -> Response {
    f.engine
        .handle(&principal, Request::new("workspace.inspect", params))
}

#[test]
fn inspection_before_a_candidate_exposes_workspace_without_transferring_authority() {
    let mut f = Fixture::new();
    let work = f.draft("Inspect before the first submission");
    f.start(&work);
    let current = f.engine.record(text(&work, "id"), "Work").unwrap();
    let workspace = f
        .engine
        .record(text(&current, "workspaceId"), "Workspace")
        .unwrap();
    let cursor = f.engine.cursor();
    let response = inspect(&mut f, Principal::Human, json!({"workId":work["id"]}));
    assert_eq!(response.status, "ok", "{response:?}");
    let data = response.data.unwrap();
    assert_eq!(data["workspace"], workspace);
    assert_eq!(data["destination"]["localRoot"], workspace["localRoot"]);
    assert_eq!(data["artifacts"], json!([]));
    assert_eq!(data["results"], json!([]));
    assert!(data.get("candidateId").is_none());
    assert_eq!(f.engine.cursor(), cursor);
    assert_eq!(
        f.engine
            .record(text(&workspace, "id"), "Workspace")
            .unwrap(),
        workspace
    );
    assert_eq!(f.engine.record(text(&work, "id"), "Work").unwrap(), current);
}

#[test]
fn inspection_shows_exact_submitted_output_before_delivery_and_keeps_candidate_path() {
    let mut f = Fixture::new();
    let work = f.draft("Inspect intermediate report");
    f.start(&work);
    let coordinator = f.coordinator(&work);
    let worker = f.plan(&work, &coordinator, false);
    f.start_invocation(&worker);
    f.acknowledge(&worker, None);
    let artifact = f.artifact(&work);
    let result = f.submit(&worker, &artifact, None);

    let response = inspect(&mut f, Principal::Human, json!({"workId":work["id"]}));
    assert_eq!(response.status, "ok", "{response:?}");
    let data = response.data.unwrap();
    assert!(data.get("candidateId").is_none());
    assert_eq!(data["artifacts"], json!([artifact]));
    assert_eq!(data["results"][0]["resultId"], result["id"]);
    assert_eq!(data["results"][0]["disposition"], "Submitted");

    f.finish_release(&worker);
    let current = f.engine.record(text(&work, "id"), "Work").unwrap();
    let candidate = f
        .engine
        .record(text(&current, "currentCandidateId"), "DeliveryCandidate")
        .unwrap();
    let response = inspect(
        &mut f,
        Principal::Human,
        json!({"workId":work["id"],"candidateId":candidate["id"]}),
    );
    assert_eq!(response.status, "ok", "{response:?}");
    let data = response.data.unwrap();
    assert_eq!(data["candidateId"], candidate["id"]);
    assert_eq!(data["artifacts"], candidate["artifacts"]);
    assert_eq!(data["destination"], candidate["destination"]);

    let other = f.draft("Another candidate owner");
    f.start(&other);
    let response = inspect(
        &mut f,
        Principal::Human,
        json!({"workId":other["id"],"candidateId":candidate["id"]}),
    );
    assert_eq!(response.failure.unwrap().code, "INVALID_REFERENCE");
}

#[test]
fn inspection_rejects_missing_workspace_explicit_bad_candidate_and_cross_work_binding() {
    let mut f = Fixture::new();
    let first = f.draft("First work");
    let second = f.draft("Second work");
    let response = inspect(&mut f, Principal::Human, json!({"workId":first["id"]}));
    assert_eq!(response.failure.unwrap().code, "BAD_STATE");
    f.start(&first);
    f.start(&second);
    let coordinator = f.coordinator(&first);
    let response = inspect(
        &mut f,
        Principal::Invocation {
            invocation_id: text(&coordinator, "id").into(),
        },
        json!({"workId":second["id"]}),
    );
    assert_eq!(response.failure.unwrap().code, "FORBIDDEN");
    let response = inspect(
        &mut f,
        Principal::Human,
        json!({"workId":first["id"],"candidateId":id()}),
    );
    assert_ne!(response.status, "ok");
    let response = inspect(
        &mut f,
        Principal::Human,
        json!({"workId":first["id"],"candidateId":""}),
    );
    assert_ne!(response.status, "ok");
}
