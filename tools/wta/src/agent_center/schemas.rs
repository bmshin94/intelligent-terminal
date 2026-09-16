// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use schemars::JsonSchema;
use serde_json::{json, Value};

use super::wire::*;

const METHODS: &[&str] = &[
    "console.open",
    "project.get",
    "project.list",
    "project.configure",
    "conversation.submit",
    "conversation.resolve_intents",
    "conversation.request_input",
    "conversation.answer_input",
    "work.create_draft",
    "work.propose_change",
    "work.apply_change",
    "grant.preview",
    "work.start",
    "plan.propose",
    "plan.apply",
    "work.get",
    "work.list",
    "work.control",
    "task.get",
    "task.list",
    "task.acknowledge",
    "task.report_progress",
    "progress.get",
    "task.request_context",
    "task.answer_context",
    "task.rework",
    "result.get",
    "result.submit",
    "result.accept",
    "result.request_changes",
    "result.reject",
    "artifact.get",
    "artifact.read",
    "artifact.capture",
    "gate.submit",
    "review.submit",
    "workspace.get",
    "workspace.inspect",
    "workspace.takeover",
    "workspace.handback",
    "operation.get",
    "coordination.finish",
    "decision.request",
    "decision.answer",
    "decision.get",
    "decision.list",
    "inbox.list",
    "delivery.prepare",
    "delivery.accept",
    "delivery.request_changes",
    "delivery.get",
    "delivery.list",
];

pub(super) fn canonical(method: &str) -> Option<&'static str> {
    METHODS
        .iter()
        .copied()
        .find(|name| *name == method || name.replace('.', "_") == method)
}

pub(super) fn is_read(method: &str) -> bool {
    matches!(
        canonical(method),
        Some(
            "project.get"
                | "project.list"
                | "work.get"
                | "work.list"
                | "task.get"
                | "task.list"
                | "result.get"
                | "progress.get"
                | "artifact.get"
                | "artifact.read"
                | "workspace.get"
                | "workspace.inspect"
                | "operation.get"
                | "decision.get"
                | "decision.list"
                | "inbox.list"
                | "delivery.get"
                | "delivery.list"
        )
    )
}

fn typed<T: JsonSchema>() -> Value {
    let mut schema = schemars::schema_for!(T).to_value();
    nonnull_options(&mut schema);
    schema
}

// Option means omitted on this wire, not an explicit JSON null. Keep the
// generated required-field list and remove nullable alternatives.
fn nonnull_options(schema: &mut Value) {
    match schema {
        Value::Array(values) => values.iter_mut().for_each(nonnull_options),
        Value::Object(object) => {
            object.remove("default");
            if let Some(Value::Array(types)) = object.get_mut("type") {
                types.retain(|value| value != "null");
                let single = (types.len() == 1).then(|| types[0].clone());
                if let Some(single) = single {
                    object.insert("type".into(), single);
                }
            }
            if let Some(Value::Array(choices)) = object.get_mut("anyOf") {
                choices.retain(|value| value["type"] != "null");
                if choices.len() == 1 {
                    let only = choices[0].clone();
                    object.remove("anyOf");
                    if let Some(fields) = only.as_object() {
                        object.extend(fields.clone());
                    }
                }
            }
            for value in object.values_mut() {
                nonnull_options(value);
            }
        }
        _ => {}
    }
}

fn object(properties: Value, required: &[&str]) -> Value {
    json!({"type":"object","additionalProperties":false,"properties":properties,"required":required})
}

fn identifier(field: &str) -> Value {
    object(json!({field:{"type":"string","format":"uuid"}}), &[field])
}

fn list(work: bool) -> Value {
    let mut properties = json!({
        "afterId":{"type":"string","format":"uuid"},
        "limit":{"type":"integer","minimum":1,"maximum":100}
    });
    let required = if work {
        properties["workId"] = json!({"type":"string","format":"uuid"});
        vec!["workId", "limit"]
    } else {
        vec!["limit"]
    };
    object(properties, &required)
}

fn set_enum(schema: &mut Value, key: &str, values: &[&str]) {
    if let Some(property) = schema
        .get_mut("properties")
        .and_then(|props| props.get_mut(key))
    {
        property["enum"] = json!(values);
    }
}

pub(super) fn params(method: &str) -> Option<Value> {
    let method = canonical(method)?;
    let mut schema = match method {
        "console.open" => typed::<ConsoleOpen>(),
        "project.configure" => typed::<ProjectConfigure>(),
        "conversation.submit" => typed::<ConversationSubmit>(),
        "conversation.resolve_intents" => typed::<ResolveIntents>(),
        "conversation.request_input" => typed::<IntakeInput>(),
        "conversation.answer_input" => typed::<AnswerIntake>(),
        "work.create_draft" => typed::<CreateDraft>(),
        "work.propose_change" => typed::<WorkProposeChange>(),
        "work.apply_change" => typed::<WorkApplyChange>(),
        "grant.preview" => typed::<GrantPreview>(),
        "work.start" => typed::<WorkStart>(),
        "plan.propose" => typed::<PlanPropose>(),
        "plan.apply" => typed::<PlanApply>(),
        "work.control" => typed::<WorkControl>(),
        "task.acknowledge" => typed::<Acknowledge>(),
        "task.report_progress" => typed::<Progress>(),
        "task.request_context" => typed::<RequestContext>(),
        "task.answer_context" => typed::<AnswerContext>(),
        "task.rework" => typed::<Rework>(),
        "result.submit" => typed::<TaskResultBody>(),
        "result.accept" => typed::<ResultVerdict>(),
        "result.request_changes" | "result.reject" => typed::<ResultChanges>(),
        "artifact.capture" => typed::<Capture>(),
        "gate.submit" => typed::<GateSubmission>(),
        "review.submit" => typed::<ReviewSubmission>(),
        "workspace.takeover" => typed::<WorkspaceTakeover>(),
        "workspace.handback" => typed::<WorkspaceHandback>(),
        "coordination.finish" => typed::<CoordinationFinish>(),
        "decision.request" => typed::<DecisionRequestBody>(),
        "decision.answer" => typed::<DecisionAnswer>(),
        "delivery.prepare" => typed::<DeliveryPrepare>(),
        "delivery.accept" => typed::<DeliveryAccept>(),
        "delivery.request_changes" => typed::<DeliveryChanges>(),
        "project.get" => identifier("projectId"),
        "work.get" => identifier("workId"),
        "task.get" => identifier("taskId"),
        "result.get" => identifier("resultId"),
        "progress.get" => identifier("reportId"),
        "artifact.get" => identifier("artifactId"),
        "artifact.read" => object(
            json!({
                "artifactId":{"type":"string","format":"uuid"},
                "relativePath":{"type":"string"},
                "offset":{"type":"integer","minimum":0},
                "limit":{"type":"integer","minimum":1,"maximum":32768}
            }),
            &["artifactId"],
        ),
        "workspace.get" => identifier("workspaceId"),
        "operation.get" => identifier("operationId"),
        "decision.get" => identifier("decisionId"),
        "delivery.get" => identifier("candidateId"),
        "project.list" | "work.list" => list(false),
        "task.list" => list(true),
        "delivery.list" | "decision.list" | "inbox.list" => {
            let mut schema = list(false);
            schema["properties"]["workId"] = json!({"type":"string","format":"uuid"});
            schema
        }
        "workspace.inspect" => object(
            json!({
                "workId":{"type":"string","format":"uuid"},"candidateId":{"type":"string","format":"uuid"}
            }),
            &["workId"],
        ),
        _ => return None,
    };
    match method {
        "conversation.submit" => set_enum(
            &mut schema,
            "declaredIntent",
            &[
                "ImmediateQuestion",
                "WorkDiscussion",
                "NewWork",
                "ContextAddition",
                "ChangeProposal",
            ],
        ),
        "task.acknowledge" => set_enum(&mut schema, "disposition", &["Accepted", "Declined"]),
        "task.answer_context" => set_enum(
            &mut schema,
            "compatibility",
            &["ExistingInputs", "RequiresRevision"],
        ),
        "task.rework" => set_enum(
            &mut schema,
            "action",
            &["CollectEvidence", "ReviseOutput", "Replan"],
        ),
        "artifact.capture" => set_enum(
            &mut schema,
            "purpose",
            &["Output", "Evidence", "ManualInput"],
        ),
        "gate.submit" => set_enum(
            &mut schema,
            "outcome",
            &["Passed", "Failed", "Inconclusive"],
        ),
        "review.submit" => set_enum(
            &mut schema,
            "recommendation",
            &["Accept", "NeedsEvidence", "ChangesRequested", "Reject"],
        ),
        "coordination.finish" => set_enum(
            &mut schema,
            "outcome",
            &[
                "Answered",
                "ActionsRecorded",
                "WaitingOnRecordedSubject",
                "NoActionNeeded",
            ],
        ),
        "conversation.answer_input" => set_enum(&mut schema, "action", &["Answer", "Cancel"]),
        "work.control" => set_enum(&mut schema, "action", &["Hold", "Resume", "Cancel"]),
        "decision.request" => set_enum(&mut schema, "purpose", &["TaskInput"]),
        _ => {}
    }
    if let Some(defs) = schema.get_mut("$defs") {
        for (name, key, values) in [
            ("TaskContract", "role", &["Contribution", "Integration"][..]),
            ("OutputContract", "kind", OutputContract::KINDS),
            ("GateDefinition", "kind", &["Command"][..]),
            (
                "ResolvedIntent",
                "kind",
                &[
                    "ImmediateQuestion",
                    "WorkDiscussion",
                    "NewWork",
                    "ContextAddition",
                    "ChangeProposal",
                ][..],
            ),
            ("ReviewPolicy", "rule", &["AllRequiredGatesThenReview"][..]),
            ("CaptureSource", "kind", &["File", "Tree", "GitCommit"][..]),
            ("ContextTarget", "kind", &["Coordinator", "Task"][..]),
            ("Resources", "mode", &["ReadOnly", "ExclusiveWrite"][..]),
            ("Delivery", "kind", &["LocalCode", "Report"][..]),
            (
                "DependencyContract",
                "condition",
                &["ArtifactAvailable", "GatePassed"][..],
            ),
            (
                "ReworkInstruction",
                "reason",
                &[
                    "NeedsEvidence",
                    "ContractViolation",
                    "Misunderstood",
                    "UserRevision",
                ][..],
            ),
            (
                "ReworkInstruction",
                "action",
                &["CollectEvidence", "ReviseOutput", "Replan"][..],
            ),
        ] {
            if let Some(definition) = defs.get_mut(name) {
                set_enum(definition, key, values);
            }
        }
        if let Some(intent) = defs.get_mut("ResolvedIntent") {
            intent["allOf"] = json!([
                {"if":{"properties":{"kind":{"const":"NewWork"}}},"then":{"required":["draftGoal"]}},
                {"if":{"properties":{"kind":{"enum":["WorkDiscussion","ContextAddition","ChangeProposal"]}}},"then":{"required":["workId"]}},
                {"if":{"properties":{"kind":{"const":"ChangeProposal"}}},"then":{"required":["changeProposalId"]}}
            ]);
        }
        if let Some(criterion) = defs.get_mut("Criterion") {
            criterion["properties"]["evidenceRule"]["description"] = json!(
                "For an approved prose evidence rule, copy the exact work.spec.criteria[].evidenceRule here; preserve its description too. Never substitute a summary. Machine rules (command:<gate>, artifact:<slot>, legacy bare ASCII gate IDs) still require their exact binding."
            );
            criterion["properties"]["requiredEvidence"]["description"] = json!(
                "Nonempty concrete evidence bindings: required Command gate IDs for this criterion or artifact:<required-output-slot>. These are executable/verifiable references, not prose; evidenceRule preserves the approved prose separately."
            );
        }
    }
    Some(schema)
}

fn mutable_subjects(method: &str) -> &'static [&'static str] {
    match method {
        "grant.preview"
        | "work.start"
        | "plan.propose"
        | "work.control"
        | "work.propose_change" => &["Work"],
        "work.apply_change" => &["Work", "ChangeProposal"],
        "plan.apply" => &["Work", "PlanProposal"],
        "task.answer_context" => &["ContextRequest"],
        "decision.answer" => &["DecisionRequest"],
        "conversation.answer_input" => &["IntakeRequest"],
        "task.rework" => &["Task", "TaskResult"],
        "result.accept" | "result.request_changes" | "result.reject" => &["TaskResult"],
        "workspace.takeover" | "workspace.handback" => &["Workspace"],
        "delivery.prepare" => &["Work", "TaskResult"],
        "delivery.accept" | "delivery.request_changes" => &["Work", "DeliveryCandidate"],
        "decision.request" => &["Work", "TaskResult", "ChangeProposal"],
        _ => &[],
    }
}

pub(super) fn tool(method: &str) -> Option<Value> {
    let method = canonical(method)?;
    let mut parameters = params(method)?;
    let definitions = parameters.as_object_mut()?.remove("$defs");
    parameters.as_object_mut()?.remove("$schema");
    let subjects = mutable_subjects(method);
    let mut properties = json!({"params":parameters});
    let mut required = vec!["params"];
    if !is_read(method) {
        properties["commandId"] = json!({
            "type":"string","format":"uuid",
            "description":"New UUID for a new intention. Reuse only for an identical retry."
        });
        required.push("commandId");
    }
    if !subjects.is_empty() {
        let mut reference = typed::<EntityRef>();
        reference.as_object_mut()?.remove("$schema");
        reference["properties"]["kind"]["enum"] = json!(subjects);
        reference["properties"]["version"]["minimum"] = json!(1);
        properties["ifMatch"] = json!({
            "type":"array","minItems":1,"items":reference,
            "description":"Exact current mutable subject references from the service view. Never guess versions."
        });
        required.push("ifMatch");
    }
    let mut input = object(properties, &required);
    if let Some(definitions) = definitions {
        input["$defs"] = definitions;
    }
    let meaning = match method {
        "plan.propose" => "Propose a plan for approved work, then plan.apply the returned proposal. Copy task scope entries from work.spec.scope and retain every exact work.spec.exclusions entry. Preserve approved criterion descriptions/evidence. Output kind must be one of the advertised OutputContract.kind values (case-sensitive); Code, Tree and GitCommit require a concrete required Command check. Refresh work.get on version conflict; correct fieldErrors with a new commandId.",
        "conversation.resolve_intents" => "Resolve messages only in the supplied intake coordination turn (scope has no workId). Approved-work coordination must use work.get and work planning tools, not re-resolve its source messages.",
        "task.acknowledge" => "First work action: accept/decline the exact dispatch or continuation.",
        "task.request_context" => "Record an internal question. Blocking requests require yielding the provider turn, not polling.",
        "artifact.capture" => "Capture immutable output/evidence. This bridge waits for the recorded capture operation.",
        "artifact.read" => "Read verified immutable evidence in bounded pages. Use nextOffset until complete; Tree without relativePath lists captured entries. Binary content is explicitly unsupported.",
        "progress.get" => "Read the exact recorded activity, findings and artifacts for a coordination trigger's reportId.",
        "result.submit" => "Submit one immutable producing result. Receipt is Submitted, not accepted or completed work.",
        "gate.submit" => "Submit an evaluation for the exact result and current evaluation round.",
        "review.submit" => "Submit an internal review; never human final delivery acceptance.",
        "coordination.finish" => "Required coordinator terminal record. Prose alone is not an applied action.",
        _ => "Use exact protocol fields and reference versions. Response status is authoritative.",
    };
    Some(json!({
        "name":method.replace('.',"_" ),
        "description":format!("Agent Center v1 {method}. {meaning}"),
        "inputSchema":input
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tools_advertise_only_implemented_gates_decisions_and_intents() {
        assert_eq!(
            params("decision.request").unwrap()["properties"]["purpose"]["enum"],
            json!(["TaskInput"])
        );
        assert_eq!(
            params("plan.propose").unwrap()["$defs"]["GateDefinition"]["properties"]["kind"]
                ["enum"],
            json!(["Command"])
        );
        assert_eq!(
            params("conversation.submit").unwrap()["properties"]["declaredIntent"]["enum"],
            json!([
                "ImmediateQuestion",
                "WorkDiscussion",
                "NewWork",
                "ContextAddition",
                "ChangeProposal"
            ])
        );
        let intents = params("conversation.resolve_intents").unwrap();
        assert_eq!(
            intents["$defs"]["ResolvedIntent"]["allOf"][2]["then"]["required"],
            json!(["changeProposalId"])
        );
    }

    #[test]
    fn plan_tools_advertise_the_enforced_output_vocabulary_and_recovery() {
        let expected = json!(["File", "Tree", "GitCommit", "Report", "Code", "Evidence"]);
        assert_eq!(json!(OutputContract::KINDS), expected);
        for method in ["plan.propose", "plan_propose"] {
            let parameters = params(method).unwrap();
            assert_eq!(
                parameters["$defs"]["OutputContract"]["properties"]["kind"]["enum"],
                expected
            );
            let advertised = tool(method).unwrap();
            assert_eq!(
                advertised["inputSchema"]["$defs"]["OutputContract"]["properties"]["kind"]["enum"],
                expected
            );
            let description = advertised["description"].as_str().unwrap();
            assert!(description.contains("work.spec.scope"));
            assert!(description.contains("fieldErrors"));
            assert!(description.contains("required Command check"));
            let criterion = &advertised["inputSchema"]["$defs"]["Criterion"];
            assert_eq!(criterion["properties"]["evidenceRule"]["type"], "string");
            assert!(!criterion["required"]
                .as_array()
                .unwrap()
                .contains(&json!("evidenceRule")));
            assert!(criterion["properties"]["requiredEvidence"]["description"]
                .as_str()
                .unwrap()
                .contains("not prose"));
        }
    }

    #[test]
    fn lists_require_bounded_pagination_and_expose_the_correct_work_scope() {
        for method in [
            "project.list",
            "work.list",
            "task.list",
            "delivery.list",
            "decision.list",
            "inbox.list",
        ] {
            let schema = params(method).unwrap();
            assert!(schema["required"]
                .as_array()
                .unwrap()
                .contains(&json!("limit")));
            assert_eq!(schema["properties"]["limit"]["minimum"], 1);
            assert_eq!(schema["properties"]["limit"]["maximum"], 100);
            assert!(schema["properties"].get("afterId").is_some());
            assert_eq!(
                schema["required"]
                    .as_array()
                    .unwrap()
                    .contains(&json!("workId")),
                method == "task.list"
            );
        }
        assert!(params("project.list").unwrap()["properties"]
            .get("workId")
            .is_none());
        assert!(params("decision.list").unwrap()["properties"]
            .get("workId")
            .is_some());
    }

    #[test]
    fn reads_do_not_request_mutation_authority() {
        let read = tool("task_get").unwrap();
        assert!(read["inputSchema"]["properties"].get("commandId").is_none());
        assert_eq!(
            read["inputSchema"]["properties"]["params"]["required"],
            json!(["taskId"])
        );
        let write = tool("result_submit").unwrap();
        assert!(write["inputSchema"]["required"]
            .as_array()
            .unwrap()
            .contains(&json!("commandId")));
    }

    #[test]
    fn evidence_and_progress_reads_are_bounded_read_only_tools() {
        for (method, field) in [
            ("progress_get", "reportId"),
            ("artifact_read", "artifactId"),
        ] {
            assert!(is_read(method));
            let schema = tool(method).unwrap();
            let input = &schema["inputSchema"];
            assert!(input["properties"].get("commandId").is_none());
            assert!(input["properties"].get("ifMatch").is_none());
            assert_eq!(input["properties"]["params"]["required"], json!([field]));
        }
        let schema = params("artifact.read").unwrap();
        assert_eq!(schema["properties"]["limit"]["maximum"], 32768);
        assert_eq!(schema["properties"]["offset"]["minimum"], 0);
    }

    #[test]
    fn submission_schema_exposes_contract_and_root_definitions() {
        let tool = tool("result.submit").unwrap();
        let input = &tool["inputSchema"];
        assert_eq!(input["properties"]["params"]["additionalProperties"], false);
        assert!(input["properties"]["params"]["required"]
            .as_array()
            .unwrap()
            .contains(&json!("inputManifestDigest")));
        assert!(input["$defs"]["ArtifactRef"].is_object());
        assert!(input["properties"]["params"].get("$defs").is_none());
        assert_eq!(
            input["properties"]["params"]["properties"]["supersedesResultId"]["type"],
            "string"
        );
    }

    #[test]
    fn all_advertised_methods_have_closed_parameter_schemas() {
        for method in METHODS {
            let tool = tool(method).unwrap_or_else(|| panic!("missing schema for {method}"));
            assert_eq!(
                tool["inputSchema"]["additionalProperties"], false,
                "{method}"
            );
            assert_eq!(
                tool["inputSchema"]["properties"]["params"]["additionalProperties"], false,
                "{method}"
            );
        }
        assert_eq!(canonical("work_create_draft"), Some("work.create_draft"));
        assert!(tool("invented_method").is_none());
    }
}
