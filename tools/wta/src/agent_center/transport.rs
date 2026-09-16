// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeServer};
use tokio::sync::{mpsc, oneshot, watch, OwnedSemaphorePermit, Semaphore};
use uuid::Uuid;

use super::engine::{Effect, Engine};
use super::wire::{Principal, Request, Response};
use crate::agent_tools::action_proposal::pipe_security;

pub(crate) const MAX_FRAME_BYTES: usize = 1_048_576;
const MAX_QUEUED_FRAMES: usize = 256;
const MAX_QUEUED_BYTES: usize = 8 * 1024 * 1024;

enum EngineMessage {
    Shutdown(oneshot::Sender<()>),
    Request(Principal, Request, oneshot::Sender<Response>),
    Complete(String, Response, oneshot::Sender<Result<()>>),
    Effects(oneshot::Sender<Result<Vec<Effect>>>),
    Snapshot(Value, oneshot::Sender<Result<(Value, String), Response>>),
    Events(
        String,
        Value,
        oneshot::Sender<Result<(Vec<Value>, String), Response>>,
    ),
}

#[derive(Clone)]
pub(crate) struct ServiceHandle {
    sender: mpsc::Sender<EngineMessage>,
    changes: watch::Receiver<u64>,
    store_id: String,
}

impl ServiceHandle {
    pub(super) fn watch_changes(&self) -> watch::Receiver<u64> {
        self.changes.clone()
    }

    pub(super) async fn shutdown(&self) -> Result<()> {
        let (sender, receiver) = oneshot::channel();
        self.sender
            .send(EngineMessage::Shutdown(sender))
            .await
            .map_err(|_| anyhow!("work service stopped before shutdown"))?;
        receiver.await.context("work store did not close cleanly")
    }

    pub(crate) async fn wait_operation(&self, principal: Principal, id: &str) -> Response {
        let mut changes = self.changes.clone();
        loop {
            // Subscribe before reading, so completion cannot strand the MCP call.
            changes.borrow_and_update();
            let response = self
                .request(
                    principal.clone(),
                    Request::new("operation.get", json!({"operationId":id})),
                )
                .await;
            if response.status != "ok" {
                return response;
            }
            let Some(view) = response.data.as_ref() else {
                return Response::fail(
                    response.request_id,
                    "PROTOCOL_INCOMPLETE",
                    "Operation view is missing",
                );
            };
            let operation = &view["operation"];
            match operation["status"].as_str() {
                Some("Succeeded") => {
                    let Some(result) = operation.get("result") else {
                        return Response::fail(
                            response.request_id,
                            "PROTOCOL_INCOMPLETE",
                            "Completed operation has no result receipt",
                        );
                    };
                    return Response::ok(response.request_id, result.clone());
                }
                Some("Failed" | "RepairRequired") => {
                    let mut error = Response::fail(
                        response.request_id,
                        if operation["status"] == "RepairRequired" {
                            "OUTCOME_UNKNOWN"
                        } else {
                            "EXECUTION_FAILED"
                        },
                        "The recorded operation did not complete; inspect its retained evidence",
                    );
                    error.operation_id = Some(id.to_owned());
                    if let Some(recorded) = view.get("failure").or_else(|| operation.get("failure"))
                    {
                        match serde_json::from_value(recorded.clone()) {
                            Ok(failure) => error.failure = Some(failure),
                            Err(decode) => {
                                tracing::error!(target:"agent_center", operation_id=id, %decode, "invalid stored operation failure")
                            }
                        }
                    }
                    return error;
                }
                Some("Pending" | "Running") => {}
                _ => {
                    return Response::fail(
                        response.request_id,
                        "PROTOCOL_INCOMPLETE",
                        "Operation has an invalid state",
                    )
                }
            }
            if changes.changed().await.is_err() {
                return service_stopped();
            }
        }
    }

    pub(crate) async fn request(&self, principal: Principal, request: Request) -> Response {
        let request_id = request.request_id.clone();
        let (sender, receiver) = oneshot::channel();
        if self
            .sender
            .send(EngineMessage::Request(principal, request, sender))
            .await
            .is_err()
        {
            return failure(
                &request_id,
                "error",
                "EXECUTION_FAILED",
                "The work service is unavailable",
            );
        }
        receiver.await.unwrap_or_else(|_| {
            failure(
                &request_id,
                "error",
                "EXECUTION_FAILED",
                "The work service stopped before replying",
            )
        })
    }

    pub(crate) async fn complete_effect(&self, id: &str, response: Response) -> Result<()> {
        let (sender, receiver) = oneshot::channel();
        self.sender
            .send(EngineMessage::Complete(id.to_owned(), response, sender))
            .await
            .map_err(|_| anyhow!("work service stopped while recording an effect"))?;
        receiver
            .await
            .context("work service lost effect completion")?
    }

    pub(super) async fn effects(&self) -> Result<Vec<Effect>> {
        let (sender, receiver) = oneshot::channel();
        self.sender
            .send(EngineMessage::Effects(sender))
            .await
            .map_err(|_| anyhow!("work service stopped while dispatching effects"))?;
        receiver
            .await
            .context("work service lost effect dispatch")?
    }

    async fn snapshot(&self, scope: Value) -> Result<(Value, String), Response> {
        let (sender, receiver) = oneshot::channel();
        self.sender
            .send(EngineMessage::Snapshot(scope, sender))
            .await
            .map_err(|_| service_stopped())?;
        receiver.await.map_err(|_| service_stopped())?
    }

    async fn events(&self, after: String, scope: Value) -> Result<(Vec<Value>, String), Response> {
        let (sender, receiver) = oneshot::channel();
        self.sender
            .send(EngineMessage::Events(after, scope, sender))
            .await
            .map_err(|_| service_stopped())?;
        receiver.await.map_err(|_| service_stopped())?
    }
}

fn service_stopped() -> Response {
    failure("", "error", "EXECUTION_FAILED", "The work service stopped")
}

fn failure(request_id: &str, status: &str, code: &str, message: &str) -> Response {
    let response = Response::fail(request_id, code, message);
    debug_assert_eq!(response.status, status);
    response
}

fn ok(request_id: &str, data: Value, cursor: Option<&str>) -> Result<Response> {
    let mut response =
        json!({"type":"response","requestId":request_id,"status":"ok","data":data,"subjects":[]});
    if let Some(cursor) = cursor {
        response["cursor"] = json!(cursor);
    }
    serde_json::from_value(response).context("constructing service response")
}

pub(crate) fn state_root() -> Result<PathBuf> {
    crate::runtime_paths::intelligent_terminal_root()
        .map(|root| root.join("agent-center"))
        .context("Agent Center requires an available application state directory")
}

pub(crate) async fn configure(input: &Path) -> Result<()> {
    configure_at(input, &state_root()?).await?;
    println!(
        "{}",
        json!({"type":"response","requestId":Uuid::new_v4().to_string(),
        "status":"ok","subjects":[],"data":{"configured":true,"effectiveOnNextServiceStart":true}})
    );
    Ok(())
}

async fn configure_at(input: &Path, root: &Path) -> Result<()> {
    let mut bytes = Vec::new();
    tokio::fs::File::open(input)
        .await
        .context("opening ACP adapter configuration")?
        .take(MAX_FRAME_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .await
        .context("reading ACP adapter configuration")?;
    if bytes.len() > MAX_FRAME_BYTES {
        bail!("ACP adapter configuration exceeds 1 MiB");
    }
    super::runtime::validate_adapter_configuration(&bytes)?;
    tokio::fs::create_dir_all(root).await?;
    let _authority = lock_authority(root)
        .context("Configure adapters before starting Agent Center; a running authority cannot be reconfigured implicitly")?;
    let staging = root.join(format!("adapters-{}.staging", Uuid::new_v4()));
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staging)
        .await?;
    let result: Result<()> = async {
        file.write_all(&bytes).await?;
        file.sync_all().await?;
        drop(file);
        tokio::fs::rename(&staging, root.join("adapters.json"))
            .await
            .context("installing ACP adapter configuration")?;
        Ok(())
    }
    .await;
    if result.is_err() {
        if let Err(error) = tokio::fs::remove_file(&staging).await {
            if error.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(target:"agent_center", %error, "adapter staging file needs cleanup");
            }
        }
    }
    result
}

fn pipe_name(root: &Path) -> Result<String> {
    let canonical = root
        .canonicalize()
        .context("resolving Agent Center state directory")?;
    let digest = Sha256::digest(
        canonical
            .as_os_str()
            .to_string_lossy()
            .to_lowercase()
            .as_bytes(),
    );
    Ok(format!(
        r"\\.\pipe\IntelligentTerminal-AgentCenter-{:x}",
        digest
    ))
}

fn create_pipe(name: &str, first: bool) -> Result<NamedPipeServer> {
    let security = pipe_security::build_required()?;
    pipe_security::create_server(name, first, Some(&security))
        .context("creating private Agent Center pipe")
}

fn lock_authority(root: &Path) -> Result<std::fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .share_mode(0)
        .open(root.join("authority.lock"))
        .context("Agent Center already has an authority, or its state directory cannot be locked")
}

pub(super) async fn start_engine(root: PathBuf) -> Result<ServiceHandle> {
    let engine = tokio::task::spawn_blocking(move || Engine::open(&root)).await??;
    let store_id = engine.store_id().to_owned();
    let (sender, mut receiver) = mpsc::channel::<EngineMessage>(128);
    let (changes_sender, changes) = watch::channel(0_u64);
    tokio::task::spawn_blocking(move || {
        let mut engine = engine;
        while let Some(message) = receiver.blocking_recv() {
            let before = engine.cursor();
            let effect_completion = matches!(&message, EngineMessage::Complete(..));
            match message {
                EngineMessage::Shutdown(reply) => {
                    drop(engine);
                    let _ = reply.send(());
                    break;
                }
                EngineMessage::Request(principal, request, reply) => {
                    // A disconnected caller does not undo an already committed command.
                    let _ = reply.send(engine.handle(&principal, request));
                }
                EngineMessage::Complete(id, response, reply) => {
                    let result = engine.complete_effect(&id, response);
                    if let Err(error) = &result {
                        tracing::error!(target:"agent_center", effect_id = id, %error, "effect receipt could not be recorded");
                    }
                    let _ = reply.send(result);
                }
                EngineMessage::Effects(reply) => {
                    let _ = reply.send(engine.take_effects());
                }
                EngineMessage::Snapshot(scope, reply) => {
                    let result = engine
                        .snapshot(&scope)
                        .map(|snapshot| (snapshot, engine.cursor()));
                    let _ = reply.send(result);
                }
                EngineMessage::Events(after, scope, reply) => {
                    let result = engine
                        .events_after(Some(&after), &scope)
                        .map(|events| (events, engine.cursor()));
                    let _ = reply.send(result);
                }
            }
            if effect_completion || before != engine.cursor() {
                changes_sender.send_modify(|generation| *generation = generation.wrapping_add(1));
            }
        }
    });
    Ok(ServiceHandle {
        sender,
        changes,
        store_id,
    })
}

pub(crate) async fn serve() -> Result<()> {
    let root = state_root()?;
    tokio::fs::create_dir_all(&root)
        .await
        .context("creating Agent Center state directory")?;
    let _authority = lock_authority(&root)?;
    let name = pipe_name(&root)?;
    let mut listener = create_pipe(&name, true)?;
    let handle = start_engine(root.clone()).await?;
    let runtime = super::runtime::Runtime::new(handle.clone(), root)?;
    runtime.register().await?;
    let dispatch_runtime = runtime.clone();
    let service_instance = Uuid::new_v4().to_string();
    let dispatch_handle = handle.clone();
    let mut dispatcher = tokio::spawn(async move {
        let mut changes = dispatch_handle.watch_changes();
        loop {
            for effect in dispatch_handle.effects().await? {
                let runtime = dispatch_runtime.clone();
                tokio::spawn(async move {
                    let id = effect.id.clone();
                    if let Err(error) = runtime.execute(effect).await {
                        tracing::error!(target:"agent_center", effect_id = id, %error, "effect requires reconciliation");
                    }
                });
            }
            changes
                .changed()
                .await
                .context("work engine event dispatcher stopped")?;
        }
        #[allow(unreachable_code)]
        Ok::<(), anyhow::Error>(())
    });
    let result = loop {
        tokio::select! {
            result = listener.connect() => {
                if let Err(error) = result { break Err(error.into()); }
                let connected = listener;
                listener = match create_pipe(&name, false) {
                    Ok(listener) => listener,
                    Err(error) => break Err(error),
                };
                let service = handle.clone();
                let instance = service_instance.clone();
                tokio::spawn(async move {
                    if let Err(error) = connection(connected, service, instance).await {
                        tracing::warn!(target:"agent_center", %error, "Console connection ended");
                    }
                });
            }
            result = &mut dispatcher => {
                break match result {
                    Ok(Err(error)) => Err(error),
                    Ok(Ok(())) => Err(anyhow!("Agent Center dispatcher stopped unexpectedly")),
                    Err(error) => Err(error.into()),
                };
            }
            result = tokio::signal::ctrl_c() => { break result.context("waiting for service interruption"); }
        }
    };
    dispatcher.abort();
    let runtime_shutdown = runtime.shutdown().await;
    handle.shutdown().await?;
    runtime_shutdown?;
    result
}

async fn read_frame<R: AsyncRead + Unpin>(reader: &mut R) -> Result<Value> {
    let length = reader.read_u32_le().await.context("reading frame length")? as usize;
    if length == 0 || length > MAX_FRAME_BYTES {
        bail!("INVALID_FRAME: payload length {length} is outside 1..={MAX_FRAME_BYTES}");
    }
    let mut bytes = vec![0; length];
    reader
        .read_exact(&mut bytes)
        .await
        .context("reading frame payload")?;
    serde_json::from_slice(&bytes)
        .context("INVALID_FRAME: payload is not a JSON object")
        .and_then(|value: Value| {
            if value.is_object() {
                Ok(value)
            } else {
                bail!("INVALID_FRAME: expected a JSON object")
            }
        })
}

async fn write_frame<W: AsyncWrite + Unpin>(writer: &mut W, value: &Value) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() > MAX_FRAME_BYTES {
        bail!("INVALID_FRAME: response exceeds the 1 MiB frame limit; use an artifact reference");
    }
    writer.write_u32_le(bytes.len() as u32).await?;
    writer.write_all(&bytes).await?;
    writer.flush().await?;
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Hello {
    #[serde(rename = "type")]
    kind: String,
    versions: Vec<u32>,
    client_instance_id: String,
    client_kind: String,
}

struct Outbound {
    bytes: Vec<u8>,
    _permit: OwnedSemaphorePermit,
}

fn queue(sender: &mpsc::Sender<Outbound>, budget: &Arc<Semaphore>, value: &Value) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() > MAX_FRAME_BYTES {
        bail!("response exceeds maximum frame size");
    }
    let permit = budget
        .clone()
        .try_acquire_many_owned(bytes.len() as u32)
        .context("RESYNC_REQUIRED: outbound byte budget exceeded")?;
    sender
        .try_send(Outbound {
            bytes,
            _permit: permit,
        })
        .map_err(|_| anyhow!("RESYNC_REQUIRED: outbound frame queue exceeded or closed"))
}

struct Subscription {
    scope: Value,
    cursor: String,
}

async fn negotiate<S: AsyncRead + AsyncWrite + Unpin>(
    pipe: &mut S,
    store_id: &str,
    instance: &str,
) -> Result<Option<Hello>> {
    let hello = match read_frame(pipe)
        .await
        .and_then(|v| serde_json::from_value::<Hello>(v).map_err(Into::into))
    {
        Ok(hello)
            if hello.kind == "hello"
                && Uuid::parse_str(&hello.client_instance_id).is_ok()
                && ["CLI", "Console", "Runtime"].contains(&hello.client_kind.as_str()) =>
        {
            hello
        }
        result => {
            write_frame(pipe, &json!({"type":"protocol_error","code":"INVALID_FRAME","message":"The first frame must be a valid hello"})).await?;
            return match result {
                Err(error) => Err(error),
                _ => Err(anyhow!("invalid hello")),
            };
        }
    };
    if !hello.versions.contains(&1) {
        write_frame(pipe, &json!({"type":"protocol_error","code":"INCOMPATIBLE_VERSION","message":"Agent Center requires protocol v1","supportedVersions":[1]})).await?;
        return Ok(None);
    }
    write_frame(
        pipe,
        &json!({
            "type":"welcome","version":1,"connectionId":Uuid::new_v4().to_string(),
            "serviceInstanceId":instance,"storeId":store_id,"maxFrameBytes":MAX_FRAME_BYTES
        }),
    )
    .await?;
    Ok(Some(hello))
}

async fn connection(
    mut pipe: NamedPipeServer,
    handle: ServiceHandle,
    instance: String,
) -> Result<()> {
    let Some(hello) = negotiate(&mut pipe, &handle.store_id, &instance).await? else {
        return Ok(());
    };
    let runtime_client = hello.client_kind == "Runtime";
    let (mut reader, mut writer) = tokio::io::split(pipe);
    let (sender, mut receiver) = mpsc::channel::<Outbound>(MAX_QUEUED_FRAMES);
    let budget = Arc::new(Semaphore::new(MAX_QUEUED_BYTES));
    let mut writer_task = tokio::spawn(async move {
        while let Some(frame) = receiver.recv().await {
            writer.write_u32_le(frame.bytes.len() as u32).await?;
            writer.write_all(&frame.bytes).await?;
            writer.flush().await?;
        }
        Ok::<(), anyhow::Error>(())
    });
    let mut changes = handle.changes.clone();
    let mut subscriptions = HashMap::<String, Subscription>::new();
    let result: Result<()> = async {
        loop {
            tokio::select! {
                frame = read_frame(&mut reader) => {
                    let frame = match frame {
                        Ok(frame) => frame,
                        Err(error) => {
                            queue(&sender, &budget, &json!({"type":"protocol_error","code":"INVALID_FRAME","message":error.to_string()}))?;
                            return Err(error);
                        }
                    };
                    if frame["type"] != "request" {
                        queue(&sender, &budget, &json!({"type":"protocol_error","code":"INVALID_FRAME","message":"Expected a request frame"}))?;
                        return Err(anyhow!("expected request frame"));
                    }
                    let request: Request = match serde_json::from_value(frame) {
                        Ok(request) => request,
                        Err(error) => {
                            queue(&sender, &budget, &json!({"type":"protocol_error","code":"INVALID_FRAME","message":error.to_string()}))?;
                            return Err(error.into());
                        }
                    };
                    let response = if runtime_client {
                        failure(&request.request_id, "unsupported", "CAPABILITY_UNAVAILABLE",
                            "External runtime connections are not enabled; invocation authority is issued by the owned runtime")
                    } else if request.method == "events.subscribe" {
                        let params = &request.params;
                        let valid = params.as_object().is_some_and(|object| object.keys().all(|key| ["scope","afterCursor"].contains(&key.as_str())))
                            && request.command_id.is_none() && request.if_match.is_empty();
                        if !valid {
                            failure(&request.request_id,"error","INVALID_ARGUMENT","events.subscribe expects scope and optional afterCursor, without mutation fields")
                        } else {
                            let scope = params.get("scope").cloned().unwrap_or(Value::Null);
                            let valid_scope = scope.as_object().is_some_and(|object| object.keys().all(|key| ["kind","id"].contains(&key.as_str())))
                                && match scope["kind"].as_str() {
                                    Some("WorkList") => scope.get("id").is_none(),
                                    Some("Work" | "Conversation" | "Operation") => scope["id"].as_str().is_some_and(|id| !id.is_empty()),
                                    _ => false,
                                };
                            if !valid_scope || params.get("afterCursor").is_some_and(|v| !v.is_string()) {
                                failure(&request.request_id,"error","INVALID_ARGUMENT","Invalid subscription scope or cursor")
                            } else {
                                let subscription_id = Uuid::new_v4().to_string();
                                let snapshot = if let Some(after) = params["afterCursor"].as_str() {
                                    handle.events(after.to_owned(),scope.clone()).await.map(|_| (None,after.to_owned()))
                                } else {
                                    handle.snapshot(scope.clone()).await.map(|(view,cursor)| (Some(view),cursor))
                                };
                                match snapshot {
                                    Ok((snapshot,cursor)) => {
                                        let mut data = json!({"subscriptionId":subscription_id,"cursor":cursor});
                                        if let Some(snapshot) = snapshot { data["snapshot"] = snapshot; }
                                        subscriptions.insert(subscription_id,Subscription{scope,cursor:cursor.clone()});
                                        ok(&request.request_id,data,Some(&cursor))?
                                    }
                                    Err(mut error) => { error.request_id = request.request_id.clone(); error }
                                }
                            }
                        }
                    } else if request.method == "events.unsubscribe" {
                        let id = request.params["subscriptionId"].as_str().unwrap_or("");
                        if request.params.as_object().is_some_and(|o| o.len()==1) && request.command_id.is_none()
                            && request.if_match.is_empty() && subscriptions.remove(id).is_some() {
                            ok(&request.request_id,json!({"closed":true}),None)?
                        } else {
                            failure(&request.request_id,"error","INVALID_REFERENCE","Unknown subscription or invalid unsubscribe payload")
                        }
                    } else {
                        handle.request(Principal::Human,request).await
                    };
                    queue(&sender,&budget,&serde_json::to_value(response)?)?;
                    send_events(&handle,&mut subscriptions,&sender,&budget).await?;
                }
                changed = changes.changed() => {
                    changed.context("work event service stopped")?;
                    send_events(&handle,&mut subscriptions,&sender,&budget).await?;
                }
                result = &mut writer_task => { return result.context("outbound writer failed")?; }
            }
        }
    }.await;
    drop(sender);
    // Bound disconnect cleanup; an unread pipe must never retain a service client forever.
    if !writer_task.is_finished() {
        if tokio::time::timeout(std::time::Duration::from_secs(2), &mut writer_task)
            .await
            .is_err()
        {
            writer_task.abort();
        }
    }
    result
}

async fn send_events(
    handle: &ServiceHandle,
    subscriptions: &mut HashMap<String, Subscription>,
    sender: &mpsc::Sender<Outbound>,
    budget: &Arc<Semaphore>,
) -> Result<()> {
    let mut closed = Vec::new();
    for (id, subscription) in subscriptions.iter_mut() {
        match handle
            .events(subscription.cursor.clone(), subscription.scope.clone())
            .await
        {
            Ok((events, cursor)) => {
                for mut event in events {
                    event["type"] = json!("event");
                    event["subscriptionId"] = json!(id);
                    queue(sender, budget, &event)?;
                }
                subscription.cursor = cursor;
            }
            Err(response) => {
                let response = serde_json::to_value(response)?;
                queue(
                    sender,
                    budget,
                    &json!({"type":"stream_error","subscriptionId":id,"failure":response["failure"],"cursor":subscription.cursor}),
                )?;
                closed.push(id.clone());
            }
        }
    }
    for id in closed {
        subscriptions.remove(&id);
    }
    Ok(())
}

type PendingReplies = Arc<Mutex<HashMap<String, oneshot::Sender<Result<Response>>>>>;

pub(crate) struct Client {
    writer:
        tokio::sync::Mutex<tokio::io::WriteHalf<tokio::net::windows::named_pipe::NamedPipeClient>>,
    pending: PendingReplies,
    events: tokio::sync::Mutex<mpsc::Receiver<Result<Value>>>,
    reader_task: tokio::task::JoinHandle<()>,
}

impl Drop for Client {
    fn drop(&mut self) {
        self.reader_task.abort();
    }
}

impl Client {
    pub(crate) async fn connect() -> Result<Self> {
        let root = state_root()?;
        tokio::fs::create_dir_all(&root)
            .await
            .context("creating Agent Center state directory")?;
        let name = pipe_name(&root)?;
        match ClientOptions::new().open(&name) {
            Ok(pipe) => return Self::from_pipe(pipe).await,
            Err(error) if matches!(error.raw_os_error(), Some(2 | 231)) => {}
            Err(error) => return Err(error).context("connecting to private Agent Center service"),
        }
        let mut child = Self::start_service_process()?;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
        loop {
            match ClientOptions::new().open(&name) {
                Ok(pipe) => return Self::from_pipe(pipe).await,
                Err(error) if matches!(error.raw_os_error(), Some(2 | 231)) => {}
                Err(error) => return Err(error).context("connecting to new Agent Center service"),
            }
            if tokio::time::Instant::now() >= deadline {
                let status = child
                    .try_wait()
                    .context("checking Agent Center service startup")?;
                bail!("Agent Center did not become available within 15 seconds (process status: {status:?}); inspect wta-center-service logs or run `wta center serve`");
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    #[cfg(test)]
    async fn connect_to(name: &str) -> Result<Self> {
        let pipe = ClientOptions::new()
            .open(name)
            .context("Agent Center is unavailable; run `wta center serve` in another terminal")?;
        Self::from_pipe(pipe).await
    }

    async fn from_pipe(mut pipe: tokio::net::windows::named_pipe::NamedPipeClient) -> Result<Self> {
        write_frame(&mut pipe,&json!({"type":"hello","versions":[1],"clientInstanceId":Uuid::new_v4().to_string(),"clientKind":"Console"})).await?;
        let welcome = read_frame(&mut pipe).await?;
        if welcome["type"] != "welcome"
            || welcome["version"] != 1
            || welcome["maxFrameBytes"] != MAX_FRAME_BYTES
            || !welcome["storeId"].is_string()
        {
            bail!("Agent Center did not negotiate protocol v1: {welcome}");
        }
        let (mut reader, writer) = tokio::io::split(pipe);
        let pending = PendingReplies::default();
        let replies = pending.clone();
        let (event_sender, events) = mpsc::channel(MAX_QUEUED_FRAMES);
        let reader_task = tokio::spawn(async move {
            let failure = loop {
                let frame = match read_frame(&mut reader).await {
                    Ok(frame) => frame,
                    Err(error) => break error,
                };
                match frame["type"].as_str() {
                    Some("response") => {
                        let response = match serde_json::from_value::<Response>(frame) {
                            Ok(response) => response,
                            Err(error) => break error.into(),
                        };
                        let reply = match replies.lock() {
                            Ok(mut replies) => replies.remove(&response.request_id),
                            Err(_) => break anyhow!("response routing lock poisoned"),
                        };
                        if let Some(reply) = reply {
                            let _ = reply.send(Ok(response));
                        } else {
                            break anyhow!("response has no matching request");
                        }
                    }
                    Some("event" | "stream_error") => {
                        if event_sender.try_send(Ok(frame)).is_err() {
                            break anyhow!("RESYNC_REQUIRED: client event consumer fell behind");
                        }
                    }
                    _ => break anyhow!("unexpected Agent Center frame: {frame}"),
                }
            };
            let message = failure.to_string();
            if let Ok(mut replies) = replies.lock() {
                for (_, reply) in replies.drain() {
                    let _ = reply.send(Err(anyhow!(message.clone())));
                }
            }
            let _ = event_sender.try_send(Err(failure));
        });
        Ok(Self {
            writer: tokio::sync::Mutex::new(writer),
            pending,
            events: tokio::sync::Mutex::new(events),
            reader_task,
        })
    }

    fn start_service_process() -> Result<std::process::Child> {
        use std::os::windows::process::CommandExt;
        use std::process::{Command, Stdio};
        use windows_sys::Win32::System::Threading::{
            CREATE_BREAKAWAY_FROM_JOB, CREATE_NEW_PROCESS_GROUP, DETACHED_PROCESS,
        };
        // A Console is disposable presentation. Refuse startup if its job cannot
        // release the authority instead of silently tying work to window closure.
        Command::new(std::env::current_exe().context("locating the WTA executable")?)
            .args(["center", "serve"])
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
            .creation_flags(CREATE_BREAKAWAY_FROM_JOB | CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS)
            .spawn()
            .context("cannot start the independent Agent Center service; run `wta center serve` outside the Console's process job")
    }

    pub(crate) async fn request(&self, request: Request) -> Result<Response> {
        if self.reader_task.is_finished() {
            bail!("Agent Center connection is closed");
        }
        let id = request.request_id.clone();
        let (sender, receiver) = oneshot::channel();
        {
            let mut pending = self
                .pending
                .lock()
                .map_err(|_| anyhow!("request routing lock poisoned"))?;
            if pending.contains_key(&id) {
                bail!("requestId is already outstanding");
            }
            pending.insert(id.clone(), sender);
        }
        let result = write_frame(
            &mut *self.writer.lock().await,
            &serde_json::to_value(request)?,
        )
        .await;
        if let Err(error) = result {
            self.pending
                .lock()
                .map_err(|_| anyhow!("request routing lock poisoned"))?
                .remove(&id);
            return Err(error);
        }
        receiver
            .await
            .context("Agent Center disconnected before responding")?
    }

    pub(crate) async fn next_event(&self) -> Result<Value> {
        self.events
            .lock()
            .await
            .recv()
            .await
            .context("Agent Center event stream closed")?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn private_pipe_routes_requests_to_the_durable_actor() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!("wta-center-pipe-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(&root).await.unwrap();
        let handle = start_engine(root.clone()).await.unwrap();
        let name = format!(r"\\.\pipe\AgentCenter-actor-test-{}", Uuid::new_v4());
        let server = create_pipe(&name, true).unwrap();
        let service = handle.clone();
        let server_task = tokio::spawn(async move {
            server.connect().await.unwrap();
            connection(server, service, Uuid::new_v4().to_string()).await
        });
        let client = Client::connect_to(&name).await.unwrap();
        let request = Request::new("project.list", json!({"limit": 10}));
        let request_id = request.request_id.clone();
        let response = client.request(request).await.unwrap();
        assert_eq!(response.request_id, request_id);
        assert_eq!(response.status, "ok");
        assert_eq!(response.data.unwrap()["items"], json!([]));

        let rejected = client
            .request(Request::new("work.create_draft", json!({})))
            .await
            .unwrap();
        assert_eq!(rejected.status, "error");
        assert!(rejected.failure.is_some());
        assert_eq!(
            client
                .request(Request::new("work.list", json!({"limit": 10})))
                .await
                .unwrap()
                .status,
            "ok"
        );

        drop(client);
        let disconnected = tokio::time::timeout(std::time::Duration::from_secs(5), server_task)
            .await
            .expect("disconnected Console must release its pipe handler")
            .unwrap();
        assert!(disconnected.is_err());
        handle.shutdown().await.unwrap();
        tokio::fs::remove_dir_all(root).await.unwrap();
    }

    #[tokio::test]
    async fn adapter_configuration_is_validated_locked_and_atomically_replaced() {
        let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!("wta-center-config-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(&directory).await.unwrap();
        let input = directory.join("input.json");
        let root = directory.join("state");
        let first = br#"{"capabilities":[]}"#;
        tokio::fs::write(&input, first).await.unwrap();
        configure_at(&input, &root).await.unwrap();
        let installed = root.join("adapters.json");
        assert_eq!(tokio::fs::read(&installed).await.unwrap(), first);

        tokio::fs::write(&input, b"not JSON").await.unwrap();
        assert!(configure_at(&input, &root).await.is_err());
        assert_eq!(tokio::fs::read(&installed).await.unwrap(), first);

        let replacement = b"{ \"capabilities\": [] }\n";
        tokio::fs::write(&input, replacement).await.unwrap();
        let authority = lock_authority(&root).unwrap();
        assert!(configure_at(&input, &root).await.is_err());
        assert_eq!(tokio::fs::read(&installed).await.unwrap(), first);
        drop(authority);
        configure_at(&input, &root).await.unwrap();
        assert_eq!(tokio::fs::read(&installed).await.unwrap(), replacement);

        tokio::fs::write(&input, vec![b' '; MAX_FRAME_BYTES + 1])
            .await
            .unwrap();
        assert!(configure_at(&input, &root)
            .await
            .unwrap_err()
            .to_string()
            .contains("exceeds 1 MiB"));
        assert_eq!(tokio::fs::read(&installed).await.unwrap(), replacement);
        assert!(!std::fs::read_dir(&root).unwrap().any(|entry| entry
            .unwrap()
            .path()
            .extension()
            .is_some_and(|extension| extension == "staging")));
        tokio::fs::remove_dir_all(directory).await.unwrap();
    }

    #[tokio::test]
    async fn frames_round_trip_unicode_and_reject_nonobjects() {
        let (mut writer, mut reader) = tokio::io::duplex(1024);
        let value = json!({"message":"\u{4ea4}\u{4ed8}","id":Uuid::new_v4().to_string()});
        write_frame(&mut writer, &value).await.unwrap();
        assert_eq!(read_frame(&mut reader).await.unwrap(), value);
        writer.write_u32_le(2).await.unwrap();
        writer.write_all(b"[]").await.unwrap();
        assert!(read_frame(&mut reader)
            .await
            .unwrap_err()
            .to_string()
            .contains("INVALID_FRAME"));
    }

    #[tokio::test]
    async fn oversized_frame_is_rejected_before_allocating_body() {
        let (mut writer, mut reader) = tokio::io::duplex(32);
        writer
            .write_u32_le(MAX_FRAME_BYTES as u32 + 1)
            .await
            .unwrap();
        assert!(read_frame(&mut reader)
            .await
            .unwrap_err()
            .to_string()
            .contains("INVALID_FRAME"));
    }

    #[test]
    fn outbound_limits_are_enforced_and_released() {
        let (sender, mut receiver) = mpsc::channel(1);
        let budget = Arc::new(Semaphore::new(32));
        queue(&sender, &budget, &json!({"a":1})).unwrap();
        assert!(queue(&sender, &budget, &json!({"a":2})).is_err());
        drop(receiver.try_recv().unwrap());
        assert_eq!(budget.available_permits(), 32);
        queue(&sender, &budget, &json!({"a":3})).unwrap();
        let (sender, _receiver) = mpsc::channel(5);
        let budget = Arc::new(Semaphore::new(1));
        assert!(queue(&sender, &budget, &json!({"a":1})).is_err());
    }

    #[tokio::test]
    async fn incompatible_hello_is_rejected_without_opening_work_store() {
        let name = format!(r"\\.\pipe\AgentCenter-hello-test-{}", Uuid::new_v4());
        let server = create_pipe(&name, true).unwrap();
        let mut client = ClientOptions::new().open(&name).unwrap();
        server.connect().await.unwrap();
        let task = tokio::spawn(async move {
            let mut server = server;
            assert!(negotiate(&mut server, "test-store", "test-instance")
                .await
                .unwrap()
                .is_none());
        });
        write_frame(&mut client,&json!({
            "type":"hello","versions":[2],"clientInstanceId":Uuid::new_v4().to_string(),"clientKind":"CLI"
        })).await.unwrap();
        let result = read_frame(&mut client).await.unwrap();
        assert_eq!(result["code"], "INCOMPATIBLE_VERSION");
        task.await.unwrap();
    }

    #[tokio::test]
    async fn client_correlates_reversed_responses_and_interleaved_events() {
        let name = format!(r"\\.\pipe\AgentCenter-client-test-{}", Uuid::new_v4());
        let mut server = create_pipe(&name, true).unwrap();
        let server_task = tokio::spawn(async move {
            server.connect().await.unwrap();
            negotiate(&mut server, "test-store", "test-instance")
                .await
                .unwrap();
            let first = read_frame(&mut server).await.unwrap();
            let second = read_frame(&mut server).await.unwrap();
            write_frame(
                &mut server,
                &json!({"type":"event","subscriptionId":"test-subscription","cursor":"one"}),
            )
            .await
            .unwrap();
            for request in [second, first] {
                write_frame(
                    &mut server,
                    &json!({
                        "type":"response","requestId":request["requestId"],"status":"ok",
                        "data":{"method":request["method"]},"subjects":[]
                    }),
                )
                .await
                .unwrap();
            }
        });
        let client = Client::connect_to(&name).await.unwrap();
        let request = |method| {
            serde_json::from_value(json!({
                "type":"request","requestId":Uuid::new_v4().to_string(),
                "method":method,"ifMatch":[],"params":{}
            }))
            .unwrap()
        };
        let (first, second) = tokio::join!(
            client.request(request("work.list")),
            client.request(request("project.list"))
        );
        assert_eq!(
            serde_json::to_value(first.unwrap()).unwrap()["data"]["method"],
            "work.list"
        );
        assert_eq!(
            serde_json::to_value(second.unwrap()).unwrap()["data"]["method"],
            "project.list"
        );
        assert_eq!(client.next_event().await.unwrap()["cursor"], "one");
        server_task.await.unwrap();
    }
}
