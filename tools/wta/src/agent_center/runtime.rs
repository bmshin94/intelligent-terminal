//! Headless execution and immutable filesystem effects for Agent Center.
//!
//! Human-installed `adapters.json` entries require `adapter.approvedModelDestination`,
//! a nonempty description of the approved provider/account or endpoint. This is
//! explicit approval metadata, never inferred from inherited environment variables;
//! it is not a network-isolation or provider-endpoint verification guarantee.

mod acp;
pub(super) mod artifacts;
mod bridge;
#[cfg(test)]
mod conformance;
mod process;
mod workspace;

use super::engine::Effect;
use super::wire::{Principal, Request, Response};
use super::ServiceHandle;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Mutex};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// Read only verified captured members, never arbitrary provider or workspace paths.
pub(super) fn read_artifact(
    record: &Value,
    relative_path: Option<&str>,
    offset: u64,
    limit: u64,
) -> Result<Value> {
    anyhow::ensure!(
        (1..=32_768).contains(&limit),
        "read limit must be 1..32768 bytes"
    );
    anyhow::ensure!(record["availability"] == "Ready", "artifact is not ready");
    let root = artifacts::verify(record)?;
    let entries = record["manifest"]["entries"]
        .as_array()
        .context("captured manifest entries missing")?;
    if record["manifest"]["kind"] == "Tree" && relative_path.is_none() {
        let start = usize::try_from(offset).context("manifest offset out of range")?;
        anyhow::ensure!(start <= entries.len(), "manifest offset out of range");
        let mut page = Vec::new();
        let mut bytes = 2;
        for entry in &entries[start..] {
            let length = serde_json::to_vec(entry)?.len() + 1;
            if bytes + length > limit as usize {
                anyhow::ensure!(
                    !page.is_empty(),
                    "limit is too small for one manifest entry"
                );
                break;
            }
            bytes += length;
            page.push(entry.clone());
        }
        let next = start + page.len();
        return Ok(json!({"artifactId":record["id"],"digest":record["digest"],
            "kind":"Tree","offsetUnit":"entries","offset":offset,"entries":page,
            "nextOffset":next,"eof":next == entries.len()}));
    }
    let entry = if let Some(relative) = relative_path {
        // Manifest membership is required in addition to path containment.
        entries
            .iter()
            .find(|entry| entry["path"].as_str() == Some(relative))
            .context("relativePath is not a captured member")?
    } else {
        anyhow::ensure!(
            record["manifest"]["kind"] == "File" && entries.len() == 1,
            "a captured file member must be selected explicitly"
        );
        &entries[0]
    };
    anyhow::ensure!(
        entry["kind"] != "Directory",
        "selected member is a directory"
    );
    let relative = text(entry, "path")?;
    let content = std::fs::read(artifacts::resolve(&root, relative)?)?;
    anyhow::ensure!(
        artifacts::digest(&content) == entry["digest"]
            && Some(content.len() as u64) == entry["size"].as_u64(),
        "captured member changed while reading"
    );
    let start = usize::try_from(offset).context("byte offset out of range")?;
    anyhow::ensure!(start <= content.len(), "byte offset out of range");
    let mut result = json!({"artifactId":record["id"],"digest":record["digest"],
        "relativePath":relative,"sizeBytes":content.len(),"offsetUnit":"bytes","offset":offset});
    let text = match std::str::from_utf8(&content) {
        Ok(text) if !text.contains('\0') => text,
        _ => {
            result["kind"] = json!("Binary");
            result["encoding"] = json!("unsupported");
            result["reason"] =
                json!("Binary evidence is retained but cannot be interpreted by this text reader");
            return Ok(result);
        }
    };
    anyhow::ensure!(
        text.is_char_boundary(start),
        "offset must be a UTF-8 character boundary"
    );
    let mut end = start.saturating_add(limit as usize).min(content.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    anyhow::ensure!(
        end > start || start == content.len(),
        "limit is too small for one UTF-8 character"
    );
    result["kind"] = json!("Text");
    result["encoding"] = json!("utf-8");
    result["text"] = json!(&text[start..end]);
    result["nextOffset"] = json!(end);
    result["eof"] = json!(end == content.len());
    Ok(result)
}

#[derive(Clone)]
pub struct Runtime {
    inner: Arc<Inner>,
}

struct Inner {
    handle: ServiceHandle,
    root: PathBuf,
    invocations: Mutex<HashMap<String, Arc<Invocation>>>,
    effects: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    adapters: BTreeMap<String, Value>,
    runtime_id: String,
    shutting_down: std::sync::atomic::AtomicBool,
}

struct Invocation {
    input: Value,
    state: Mutex<InvocationState>,
    report_lock: Mutex<()>,
    cancel: CancellationToken,
    commands: mpsc::Sender<Control>,
}

enum Control {
    Continue(Value),
    Release,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InvocationState {
    state: String,
    sequence: u64,
    turn: u64,
    terminal_record_ids: Vec<String>,
    terminal_observation: Option<Value>,
    continuation_ids: BTreeMap<String, Value>,
    current_continuation_id: Option<String>,
    stop_operations: Vec<String>,
    acknowledged: bool,
    waiting: bool,
    released: bool,
    settled: bool,
    execution_identity: String,
}

impl Runtime {
    pub fn new(handle: ServiceHandle, root: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&root).context("create Agent Center runtime root")?;
        std::fs::create_dir_all(root.join("invocations"))?;
        std::fs::create_dir_all(root.join("effects"))?;
        let configuration = root.join("adapters.json");
        let adapters = if configuration.exists() {
            read_adapters(&std::fs::read(configuration)?)?
        } else {
            BTreeMap::new()
        };
        Ok(Self {
            inner: Arc::new(Inner {
                handle,
                root,
                invocations: Mutex::new(HashMap::new()),
                effects: Mutex::new(HashMap::new()),
                adapters,
                runtime_id: Uuid::new_v4().to_string(),
                shutting_down: std::sync::atomic::AtomicBool::new(false),
            }),
        })
    }

    /// Advertise only configured ACP adapters; loading configuration never launches a model.
    pub async fn register(&self) -> Result<()> {
        let mut capabilities = vec![
            json!({"id":"native-check","kinds":["EvaluateGate"],"supportsContinuation":false,"supportsScopedStop":true}),
        ];
        capabilities.extend(self.inner.adapters.keys().map(|id| {
            json!({
                "id":id,"kinds":["ProduceResult","ReviewResult","Coordinate"],
                "supportsContinuation":true,"supportsScopedStop":true
            })
        }));
        success(self.request(Principal::Runtime {runtime_id:self.inner.runtime_id.clone()}, "runtime.register", json!({
            "runtimeInstanceId":self.inner.runtime_id,"protocolVersions":[1],"capabilities":capabilities
        })).await)?;
        Ok(())
    }

    /// Revoke invocation bindings and wait for their owned process trees to settle.
    pub async fn shutdown(&self) -> Result<()> {
        self.inner
            .shutting_down
            .store(true, std::sync::atomic::Ordering::Release);
        let invocations: Vec<_> = self
            .inner
            .invocations
            .lock()
            .await
            .values()
            .cloned()
            .collect();
        for invocation in &invocations {
            invocation.cancel.cancel();
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        loop {
            let mut unsettled = Vec::new();
            for invocation in &invocations {
                if !invocation.state.lock().await.settled {
                    unsettled.push(text(&invocation.input, "id")?.to_owned());
                }
            }
            if unsettled.is_empty() {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                bail!(
                    "runtime shutdown could not prove settlement for invocations: {}",
                    unsettled.join(", ")
                );
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    pub async fn execute(&self, effect: Effect) -> Result<()> {
        Uuid::parse_str(&effect.id).context("effect ID must be UUID")?;
        let serial = self
            .inner
            .effects
            .lock()
            .await
            .entry(effect.id.clone())
            .or_default()
            .clone();
        let _serial = serial.lock().await;
        let intent = self
            .inner
            .root
            .join("effects")
            .join(format!("{}.intent", effect.id));
        let fingerprint = artifacts::digest(&serde_json::to_vec(
            &json!({"method":effect.method,"params":effect.params}),
        )?);
        if intent.exists() && tokio::fs::read_to_string(&intent).await? != fingerprint {
            bail!("effect ID reused with different content");
        }
        let receipt = self
            .inner
            .root
            .join("effects")
            .join(format!("{}.response.json", effect.id));
        if receipt.exists() {
            let response = serde_json::from_slice(&tokio::fs::read(receipt).await?)?;
            return self
                .inner
                .handle
                .complete_effect(&effect.id, response)
                .await;
        }
        if intent.exists() && !effect.method.starts_with("runtime.") {
            let response = Response::fail(&effect.id, "OUTCOME_UNKNOWN", "filesystem effect was previously started without a completion receipt; reconcile the same operation");
            return self
                .inner
                .handle
                .complete_effect(&effect.id, response)
                .await;
        }
        tokio::fs::write(intent, fingerprint).await?;
        let result = self.effect(&effect.method, &effect.params).await;
        let mut response = match result {
            Ok(response) => response,
            Err(error) => Response::fail(&effect.id, "EXECUTION_FAILED", format!("{error:#}")),
        };
        response.request_id = effect.id.clone();
        tokio::fs::write(receipt, serde_json::to_vec(&response)?)
            .await
            .context("persist effect completion receipt")?;
        self.inner
            .handle
            .complete_effect(&effect.id, response)
            .await
    }

    async fn effect(&self, method: &str, params: &Value) -> Result<Response> {
        match method {
            "runtime.invoke" => self.invoke(params["invocation"].clone()).await,
            "runtime.continue" => {
                let invocation = self.lookup(params).await?;
                let continuation = &params["continuation"];
                let id = text(continuation, "id")?;
                let mut state = invocation.state.lock().await;
                if let Some(original) = state.continuation_ids.get(id) {
                    if original != continuation {
                        bail!("continuation ID reused with different content");
                    }
                    return Ok(Response::ok(
                        "",
                        json!({"continuationId":id,"disposition":"AlreadyRecorded"}),
                    ));
                }
                if state.released || state.state == "Ended" || invocation.cancel.is_cancelled() {
                    return Ok(Response::fail(
                        "",
                        "BAD_STATE",
                        "invocation cannot be continued",
                    ));
                }
                let dispatch = &invocation.input["dispatch"];
                for key in ["dispatchId", "taskRevision", "bindingGeneration"] {
                    let expected = match key {
                        "dispatchId" => &dispatch["id"],
                        "taskRevision" => &dispatch["taskRevision"],
                        _ => &invocation.input["bindingGeneration"],
                    };
                    if &continuation[key] != expected {
                        bail!("continuation binding mismatch: {key}");
                    }
                    if continuation["compatibleInputManifestDigest"]
                        != dispatch["inputManifestDigest"]
                    {
                        bail!("continuation cannot change the pinned input manifest");
                    }
                }
                let limit = invocation.input["limits"]["remainingContextRounds"]
                    .as_u64()
                    .unwrap_or(0);
                if state.continuation_ids.len() as u64 >= limit {
                    bail!("context allowance exhausted");
                }
                state
                    .continuation_ids
                    .insert(id.into(), continuation.clone());
                self.persist(&invocation, &state)?;
                invocation
                    .commands
                    .try_send(Control::Continue(continuation.clone()))
                    .context("continuation queue full")?;
                Ok(Response::ok(
                    "",
                    json!({"continuationId":id,"disposition":"Recorded"}),
                ))
            }
            "runtime.stop" => {
                let invocation = self.lookup(params).await?;
                let operation = text(params, "operationId")?;
                {
                    let mut state = invocation.state.lock().await;
                    if !state.stop_operations.iter().any(|id| id == operation) {
                        state.stop_operations.push(operation.into());
                    }
                    if state.settled || state.released {
                        drop(state);
                        self.settled(&invocation).await?;
                        return Ok(Response::ok(
                            "",
                            json!({"quiescent":true,"operationId":operation}),
                        ));
                    }
                    state.state = "Settling".into();
                    self.persist(&invocation, &state)?;
                }
                invocation.cancel.cancel();
                Ok(Response::pending(
                    "",
                    operation.into(),
                    json!({"invocationId":params["invocationId"]}),
                ))
            }
            "runtime.release" => {
                let invocation = self.lookup(params).await?;
                let mut state = invocation.state.lock().await;
                if !state.settled || !matches!(state.state.as_str(), "Idle" | "Ended") {
                    return Ok(Response::fail(
                        "",
                        "BAD_STATE",
                        "release requires proven quiescence",
                    ));
                }
                if !state.released {
                    if let Err(error) = invocation.commands.try_send(Control::Release) {
                        tracing::debug!(target:"agent_center", %error, "settled invocation control channel is closed");
                    }
                    state.released = true;
                    state.state = "Ended".into();
                    invocation.cancel.cancel();
                    self.persist(&invocation, &state)?;
                }
                Ok(Response::ok("", json!({"released":true})))
            }
            "runtime.probe" => {
                let id = text(params, "invocationId")?;
                let entries = self.inner.invocations.lock().await;
                let Some(invocation) = entries.get(id) else {
                    let state = if self.ledger(id)?.exists() {
                        "Unknown"
                    } else {
                        "NotStarted"
                    };
                    return Ok(Response::ok(
                        "",
                        json!({"state":state,"lastSequence":0,"terminalRecordIds":[]}),
                    ));
                };
                let state = invocation.state.lock().await;
                let mut data = json!({"state":state.state,"lastSequence":state.sequence,"terminalRecordIds":state.terminal_record_ids});
                if let Some(observation) = &state.terminal_observation {
                    data["terminalObservation"] = observation.clone();
                }
                Ok(Response::ok("", data))
            }
            "artifact.capture" => {
                let path = self.workspace(text(params, "workspaceId")?).await?;
                let sources = params["sources"]
                    .as_array()
                    .context("capture sources required")?;
                if sources.is_empty() || sources.len() > 32 {
                    bail!("capture requires 1..32 sources");
                }
                let mut captured = Vec::new();
                for source in sources {
                    captured.push(self.capture(&path, source).await?);
                }
                Ok(Response::ok("", json!({"artifacts":captured})))
            }
            "workspace.create" | "workspace.provision" => Ok(Response::ok(
                "",
                workspace::create(&self.inner.root, params).await?,
            )),
            "workspace.prepare_delivery" | "workspace.commit" => {
                let path = self.workspace(text(params, "workspaceId")?).await?;
                let data =
                    workspace::commit(&self.inner.root, &path, "Agent Center fixed local delivery")
                        .await?;
                Ok(Response::ok("", data))
            }
            "workspace.handback" => {
                let path = self.workspace(text(params, "workspaceId")?).await?;
                workspace::managed(&self.inner.root, &path)?;
                let artifact = self
                    .capture(&path, &json!({"kind":"Tree","relativePath":""}))
                    .await?;
                Ok(Response::ok(
                    "",
                    json!({"artifacts":[artifact],"workspaceId":params["workspaceId"],"summary":params["summary"]}),
                ))
            }
            "workspace.takeover" => {
                // The service holds admission and settles writers before dispatching this effect.
                let path = self.workspace(text(params, "workspaceId")?).await?;
                workspace::managed(&self.inner.root, &path)?;
                Ok(Response::ok(
                    "",
                    json!({"workspaceId":params["workspaceId"],"localPath":path,"ready":true}),
                ))
            }
            _ => Ok(Response::fail(
                "",
                "METHOD_UNSUPPORTED",
                format!("unsupported runtime effect {method}"),
            )),
        }
    }

    fn ledger(&self, id: &str) -> Result<PathBuf> {
        Uuid::parse_str(id).context("invocation ID must be a UUID")?;
        Ok(self
            .inner
            .root
            .join("invocations")
            .join(format!("{id}.json")))
    }

    fn persist(&self, invocation: &Invocation, state: &InvocationState) -> Result<()> {
        let path = self.ledger(text(&invocation.input, "id")?)?;
        // The ledger intentionally stores no provider configuration or MCP bearer.
        std::fs::write(path, serde_json::to_vec(&json!({"inputDigest":artifacts::digest(&serde_json::to_vec(&invocation.input)?),"state":state}))?)
            .context("persist invocation ledger")
    }

    async fn invoke(&self, mut input: Value) -> Result<Response> {
        if input["dispatch"]["kind"] != "EvaluateGate" {
            let capability = text(&input, "capabilityId")?;
            let adapter = self
                .inner
                .adapters
                .get(capability)
                .context("ACP capability is not configured in adapters.json")?;
            input["adapter"] = adapter.clone();
        }
        let id = text(&input, "id")?.to_owned();
        let mut invocations = self.inner.invocations.lock().await;
        if self
            .inner
            .shutting_down
            .load(std::sync::atomic::Ordering::Acquire)
        {
            bail!("runtime is shutting down; invocation admission is closed");
        }
        if let Some(existing) = invocations.get(&id) {
            return Ok(duplicate_receipt(&existing.input, &input));
        }
        if self.ledger(&id)?.exists() {
            return Ok(Response::fail(
                "",
                "OUTCOME_UNKNOWN",
                "recorded invocation needs reconciliation; it will not be prompted twice",
            ));
        }
        if invocations
            .values()
            .filter(|entry| !entry.cancel.is_cancelled())
            .count()
            >= 64
        {
            bail!("runtime invocation limit reached");
        }
        deadline(&input)?;
        let (commands, receiver) = mpsc::channel(32);
        let invocation = Arc::new(Invocation {
            input,
            state: Mutex::new(InvocationState {
                state: "NotStarted".into(),
                settled: true,
                ..Default::default()
            }),
            report_lock: Mutex::new(()),
            cancel: CancellationToken::new(),
            commands,
        });
        self.persist(&invocation, &*invocation.state.lock().await)?;
        invocations.insert(id.clone(), invocation.clone());
        let runtime = self.clone();
        std::thread::Builder::new().name(format!("center-{id}")).spawn(move || {
            let result = tokio::runtime::Builder::new_current_thread().enable_all().build();
            match result {
                Ok(executor) => {
                    tokio::task::LocalSet::new().block_on(&executor, async {
                        if let Err(error) = runtime.run(invocation.clone(), receiver).await {
                            tracing::warn!(target:"agent_center", %error, "invocation failed");
                            let quiescent = invocation.state.lock().await.settled;
                            let finish = if invocation.cancel.is_cancelled() { "Cancelled" } else { "Error" };
                            if let Err(report_error) = runtime.end(&invocation, finish, Some(format!("{error:#}")), quiescent).await {
                                tracing::error!(target:"agent_center", %report_error, "failed to report invocation failure");
                            }
                            if quiescent && !invocation.state.lock().await.execution_identity.is_empty() {
                                if let Err(report_error) = runtime.settled(&invocation).await {
                                    tracing::error!(target:"agent_center", %report_error, "failed to report invocation settlement");
                                }
                            }
                        }
                    });
                }
                Err(error) => tracing::error!(target:"agent_center", %error, "cannot initialize invocation executor"),
            }
        }).context("spawn invocation executor thread")?;
        Ok(Response::ok(
            "",
            json!({"invocationId":id,"disposition":"Recorded"}),
        ))
    }

    async fn lookup(&self, params: &Value) -> Result<Arc<Invocation>> {
        self.inner
            .invocations
            .lock()
            .await
            .get(text(params, "invocationId")?)
            .cloned()
            .context("invocation not recorded in this runtime")
    }

    async fn request(&self, principal: Principal, method: &str, params: Value) -> Response {
        let mut request = Request::new(method, params);
        if !super::schemas::is_read(method) {
            request.command_id = Some(Uuid::new_v4().to_string());
        }
        self.inner.handle.request(principal, request).await
    }

    async fn bound(&self, invocation: &Invocation, method: &str, params: Value) -> Response {
        self.request(
            Principal::Invocation {
                invocation_id: invocation.input["id"].as_str().unwrap_or_default().into(),
            },
            method,
            params,
        )
        .await
    }

    async fn report(&self, invocation: &Invocation, kind: &str, data: Value) -> Result<()> {
        let _serial = invocation.report_lock.lock().await;
        let mut state = invocation.state.lock().await;
        let sequence = state.sequence + 1;
        let params = json!({
            "observationId":Uuid::new_v4().to_string(),"invocationId":invocation.input["id"],
            "bindingGeneration":invocation.input["bindingGeneration"],"sequence":sequence,"kind":kind,"data":data
        });
        let runtime_id = text(&invocation.input, "runtimeId")?.into();
        let response = self
            .request(Principal::Runtime { runtime_id }, "runtime.report", params)
            .await;
        success(response)?;
        state.sequence = sequence;
        if kind == "TurnEnded" {
            state.terminal_observation = Some(json!({"sequence":sequence,"data":data}));
        }
        self.persist(invocation, &state)
    }

    async fn end(
        &self,
        invocation: &Invocation,
        finish: &str,
        error: Option<String>,
        quiescent: bool,
    ) -> Result<()> {
        let turn = {
            let mut state = invocation.state.lock().await;
            state.state = if quiescent { "Idle" } else { "Settling" }.into();
            state.turn = state.turn.max(1);
            state.turn
        };
        let mut data = json!({"turnNumber":turn,"finish":finish,"quiescent":quiescent});
        if let Some(error) = error {
            data["errorText"] = json!(error);
        }
        self.report(invocation, "TurnEnded", data).await
    }

    async fn settled(&self, invocation: &Invocation) -> Result<()> {
        let data = {
            let state = invocation.state.lock().await;
            json!({"quiescent":true,"executionIdentity":state.execution_identity,"completedOperationIds":state.stop_operations})
        };
        self.report(invocation, "Settled", data).await
    }

    async fn workspace(&self, id: &str) -> Result<PathBuf> {
        let data = success(
            self.request(
                Principal::Service,
                "workspace.get",
                json!({"workspaceId":id}),
            )
            .await,
        )?;
        let record = data.get("workspace").unwrap_or(&data);
        let path = record["localPath"]
            .as_str()
            .or_else(|| record["root"].as_str())
            .or_else(|| record["localRoot"].as_str())
            .context("workspace has no local root")?;
        PathBuf::from(path)
            .canonicalize()
            .context("workspace root unavailable")
    }

    async fn capture(&self, path: &Path, source: &Value) -> Result<Value> {
        if source["kind"] == "GitCommit" {
            workspace::capture_commit(&self.inner.root, path, source).await
        } else {
            let (root, path, source) = (self.inner.root.clone(), path.to_owned(), source.clone());
            tokio::task::spawn_blocking(move || artifacts::capture(&root, &path, &source)).await?
        }
    }

    async fn run(
        &self,
        invocation: Arc<Invocation>,
        controls: mpsc::Receiver<Control>,
    ) -> Result<()> {
        if invocation.cancel.is_cancelled() {
            bail!("invocation cancelled before execution");
        }
        let dispatch = &invocation.input["dispatch"];
        let cwd = if let Some(workspace) = dispatch["workspaceId"].as_str() {
            self.workspace(workspace).await?
        } else if let Some(workspace) =
            invocation.input["coordinationInput"]["snapshot"]["work"]["workspaceId"].as_str()
        {
            self.workspace(workspace).await?
        } else {
            let path = self
                .inner
                .root
                .join("coordination")
                .join(text(&invocation.input, "id")?);
            tokio::fs::create_dir_all(&path).await?;
            path
        };
        let mut inputs = Vec::new();
        let mut check_records = Vec::new();
        if let Some(refs) = dispatch["inputs"].as_array() {
            for reference in refs {
                let artifact = &reference["artifact"];
                let data = success(
                    self.bound(
                        &invocation,
                        "artifact.get",
                        json!({"artifactId":artifact["artifactId"]}),
                    )
                    .await,
                )?;
                let record = data.get("artifact").unwrap_or(&data).clone();
                if record["digest"] != artifact["digest"] {
                    bail!("pinned input digest differs from captured artifact");
                }
                if dispatch["kind"] == "EvaluateGate" {
                    check_records.push((reference.clone(), record.clone()));
                }
                let destination = self
                    .inner
                    .root
                    .join("inputs")
                    .join(text(&invocation.input, "id")?)
                    .join(text(artifact, "artifactId")?);
                let target = destination.clone();
                tokio::task::spawn_blocking(move || {
                    if target.exists() {
                        artifacts::verify_materialized(&record, &target)
                    } else {
                        artifacts::materialize(&record, &target)
                    }
                })
                .await??;
                inputs.push(
                    json!({"slot":reference["slot"],"localPath":destination,"artifact":artifact}),
                );
            }
        }
        if dispatch["kind"] == "EvaluateGate" {
            let check_cwd = self
                .inner
                .root
                .join("checks")
                .join(text(&invocation.input, "id")?);
            let target = check_cwd.clone();
            let check_dispatch = dispatch.clone();
            tokio::task::spawn_blocking(move || {
                artifacts::check_working_copy(&check_dispatch, &check_records, &target)
            })
            .await??;
            self.run_check(&invocation, &cwd, &check_cwd).await?;
        } else {
            acp::run(self.clone(), invocation, cwd, inputs, controls).await?;
        }
        Ok(())
    }

    async fn run_check(&self, invocation: &Invocation, cwd: &Path, check_cwd: &Path) -> Result<()> {
        let dispatch = &invocation.input["dispatch"];
        let gate_id = text(dispatch, "gateDefinitionId")?;
        let gate = dispatch["gateDefinitions"]
            .as_array()
            .and_then(|gates| gates.iter().find(|gate| gate["id"] == gate_id))
            .context("declared gate definition unavailable")?;
        let recipe: process::Recipe = serde_json::from_value(gate["recipe"].clone())?;
        if recipe.evidence_parser_id != "process-exit-v1" {
            bail!("unregistered evidence parser");
        }
        if !matches!(
            recipe.environment_ref.as_str(),
            "clean" | "clean-v1" | "default" | "local-default"
        ) {
            bail!("unregistered command environment");
        }
        {
            let mut state = invocation.state.lock().await;
            state.execution_identity = format!("command:{}", text(&invocation.input, "id")?);
            state.state = "Running".into();
            state.turn = 1;
        }
        self.report(invocation, "Started", json!({"adapterKind":"Command","executionIdentity":format!("command:{}", text(&invocation.input,"id")?)})).await?;
        success(self.bound(invocation, "task.acknowledge", json!({"dispatchId":dispatch["id"],"taskRevision":dispatch["taskRevision"],"disposition":"Accepted"})).await)?;
        let mut recipe = recipe;
        recipe.timeout_seconds = recipe
            .timeout_seconds
            .min(deadline(&invocation.input)?.as_secs().max(1));
        invocation.state.lock().await.settled = false;
        let outcome = process::run(&recipe, check_cwd, invocation.cancel.clone()).await?;
        invocation.state.lock().await.settled = true;
        let evidence_dir = cwd
            .join(".agent-center-evidence")
            .join(text(&invocation.input, "id")?);
        tokio::fs::create_dir_all(&evidence_dir).await?;
        tokio::fs::write(evidence_dir.join("stdout.txt"), &outcome.stdout).await?;
        tokio::fs::write(evidence_dir.join("stderr.txt"), &outcome.stderr).await?;
        let evidence = json!({"recipe":recipe,"cwd":check_cwd,"inputManifestDigest":dispatch["inputManifestDigest"],"outcome":outcome});
        tokio::fs::write(
            evidence_dir.join("process.json"),
            serde_json::to_vec_pretty(&evidence)?,
        )
        .await?;
        let relative = evidence_dir.strip_prefix(cwd)?.to_string_lossy();
        let capture = self.bound(invocation, "artifact.capture", json!({"workspaceId":dispatch["workspaceId"],"sources":[{"kind":"Tree","relativePath":relative}],"purpose":"Evidence"})).await;
        let captured = bridge::await_operation(self, invocation, capture).await?;
        tokio::fs::remove_dir_all(&evidence_dir)
            .await
            .context("remove captured check staging")?;
        let refs = artifact_refs(&captured)?;
        let submission = json!({
            "evaluationUnitId":dispatch["evaluationUnitId"],"dispatchId":dispatch["id"],
            "taskRevision":dispatch["taskRevision"],"subjectResultId":dispatch["subjectResultId"],
            "evaluationRound":dispatch["evaluationRound"],"gateDefinitionId":gate_id,
            "gateDefinitionRevision":gate["revision"],"inputManifestDigest":dispatch["inputManifestDigest"],
            "outcome":outcome.disposition(),"evidence":refs,
            "explanation":format!("process-exit-v1: exit={:?}; timeout={}; cancelled={}; truncated={}",outcome.exit_code,outcome.timed_out,outcome.cancelled,outcome.output_truncated)
        });
        let submitted = success(self.bound(invocation, "gate.submit", submission).await)?;
        if let Some(id) = submitted["gateResultId"].as_str() {
            invocation
                .state
                .lock()
                .await
                .terminal_record_ids
                .push(id.into());
        }
        self.end(
            invocation,
            if outcome.cancelled {
                "Cancelled"
            } else {
                "Normal"
            },
            None,
            true,
        )
        .await?;
        self.settled(invocation).await
    }
}

fn artifact_refs(data: &Value) -> Result<Vec<Value>> {
    Ok(data["artifacts"]
        .as_array()
        .context("capture result has no artifacts")?
        .iter()
        .map(|artifact| json!({"artifactId":artifact["artifactId"],"digest":artifact["digest"]}))
        .collect())
}

fn duplicate_receipt(original: &Value, repeated: &Value) -> Response {
    if original != repeated {
        Response::fail(
            "",
            "COMMAND_ID_REUSED",
            "invocation ID reused with different content",
        )
    } else {
        Response::ok(
            "",
            json!({"invocationId":original["id"],"disposition":"AlreadyRecorded"}),
        )
    }
}

pub(super) fn validate_adapter_configuration(bytes: &[u8]) -> Result<()> {
    read_adapters(bytes).map(|_| ())
}

fn read_adapters(bytes: &[u8]) -> Result<BTreeMap<String, Value>> {
    if bytes.len() > 262_144 {
        bail!("adapter configuration exceeds 256 KiB");
    }
    let config: Value = serde_json::from_slice(bytes)?;
    let capabilities = config["capabilities"]
        .as_array()
        .context("adapters.json requires capabilities[]")?;
    if capabilities.len() > 16 {
        bail!("at most 16 ACP capabilities can be configured");
    }
    let mut adapters = BTreeMap::new();
    for capability in capabilities {
        let id = text(capability, "id")?;
        if id.is_empty() || id.len() > 128 || id == "native-check" {
            bail!("invalid or reserved ACP capability ID");
        }
        let adapter = &capability["adapter"];
        if adapter["kind"] != "ACP" || text(adapter, "executable")?.is_empty() {
            bail!("configured capability requires ACP kind and executable");
        }
        if text(adapter, "executable")?.contains('\0') {
            bail!("invalid executable");
        }
        let destination = text(adapter, "approvedModelDestination")
            .context("ACP configuration requires an explicitly human-approved model destination")?;
        if destination.trim().is_empty()
            || destination.len() > 1024
            || destination.chars().any(char::is_control)
        {
            bail!("approvedModelDestination must be a nonempty description of at most 1024 bytes without control characters");
        }
        if let Some(args) = adapter.get("args") {
            let args: Vec<String> =
                serde_json::from_value(args.clone()).context("adapter args must be strings")?;
            if args.len() > 128 || args.iter().any(|arg| arg.contains('\0')) {
                bail!("invalid adapter arguments");
            }
        }
        if let Some(model) = adapter.get("model") {
            if !model.is_string() {
                bail!("adapter model must be a string");
            }
        }
        if let Some(environment) = adapter.get("environment") {
            let environment: BTreeMap<String, String> = serde_json::from_value(environment.clone())
                .context("adapter environment must map names to strings")?;
            if environment.len() > 64
                || environment.iter().any(|(name, value)| {
                    name.is_empty() || name.contains(['\0', '=']) || value.contains('\0')
                })
            {
                bail!("invalid adapter environment");
            }
        }
        if adapters.insert(id.into(), adapter.clone()).is_some() {
            bail!("duplicate capability ID");
        }
    }
    Ok(adapters)
}

fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value[key]
        .as_str()
        .with_context(|| format!("{key} must be a string"))
}

fn success(response: Response) -> Result<Value> {
    if response.status != "ok" {
        bail!(
            "work service {}: {}",
            response.status,
            response
                .failure
                .map(|failure| format!("{}: {}", failure.code, failure.message))
                .unwrap_or_default()
        );
    }
    response.data.context("work service response data missing")
}

fn deadline(input: &Value) -> Result<Duration> {
    let deadline = time::OffsetDateTime::parse(
        text(&input["limits"], "deadlineUtc")?,
        &time::format_description::well_known::Rfc3339,
    )?;
    let remaining =
        deadline.unix_timestamp_nanos() - time::OffsetDateTime::now_utc().unix_timestamp_nanos();
    if remaining <= 0 {
        bail!("invocation deadline expired");
    }

    Ok(Duration::from_nanos(
        (remaining as u128).min(3_600_000_000_000) as u64,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invocation_dedup_preserves_identity_and_rejects_changed_contract() {
        let original = json!({"id":Uuid::new_v4().to_string(),"dispatch":{"id":"dispatch","contractDigest":"sha256:original"}});
        let response = duplicate_receipt(&original, &original);
        assert_eq!(response.status, "ok");
        assert_eq!(response.data.unwrap()["disposition"], "AlreadyRecorded");
        let mut changed = original.clone();
        changed["dispatch"]["contractDigest"] = json!("sha256:changed");
        let response = duplicate_receipt(&original, &changed);
        assert_eq!(response.failure.unwrap().code, "COMMAND_ID_REUSED");
    }

    #[test]
    fn expired_invocation_cannot_start_a_provider() {
        assert!(deadline(&json!({"limits":{"deadlineUtc":"2020-01-01T00:00:00Z"}})).is_err());
        assert!(deadline(&json!({"limits":{}})).is_err());
    }

    #[test]
    fn acp_configuration_requires_explicit_destination_approval() {
        let mut configuration = json!({"capabilities":[{"id":"approved-agent","adapter":{
            "kind":"ACP","executable":"controlled-agent.exe","args":[],"model":"approved-model",
            "environment":{"PROVIDER_BASE_URL":"https://example.invalid"}
        }}]});
        assert!(
            validate_adapter_configuration(&serde_json::to_vec(&configuration).unwrap()).is_err()
        );
        configuration["capabilities"][0]["adapter"]["approvedModelDestination"] = json!(" ");
        assert!(
            validate_adapter_configuration(&serde_json::to_vec(&configuration).unwrap()).is_err()
        );
        configuration["capabilities"][0]["adapter"]["approvedModelDestination"] =
            json!("Local deterministic fixture; no model service");
        validate_adapter_configuration(&serde_json::to_vec(&configuration).unwrap()).unwrap();
    }
}
