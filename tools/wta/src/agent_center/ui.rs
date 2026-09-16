// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use super::client::{capture_versions, new_context, resolve_input_args, send, status_exit_code};
use super::commands::{self, Action, CommandContext, Input, Operation};
use super::transport::Client;
use anyhow::{bail, Context, Result};
use crossterm::{
    event::{
        DisableBracketedPaste, EnableBracketedPaste, Event, EventStream, KeyCode, KeyEventKind,
        KeyModifiers,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use futures::StreamExt;
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Style},
    widgets::{Block, Borders, Paragraph, Wrap},
    Terminal,
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use tokio::sync::mpsc;

struct WorkView {
    draft: String,
    transcript: Vec<String>,
    scroll: u16,
    follow: bool,
    messages: Vec<Value>,
}

impl Default for WorkView {
    fn default() -> Self {
        Self {
            draft: String::new(),
            transcript: vec![],
            scroll: 0,
            follow: true,
            messages: vec![],
        }
    }
}

struct PendingConfirmation {
    operation: Operation,
    preview: Value,
    work: Option<String>,
    input: String,
}

#[derive(Clone)]
struct InboxRefresh {
    operation: Operation,
    versions: BTreeMap<(String, String), u64>,
}

struct State {
    context: CommandContext,
    views: BTreeMap<Option<String>, WorkView>,
    works: Vec<Value>,
    inbox: Vec<Value>,
    notice: String,
    completion: usize,
    pending: Option<PendingConfirmation>,
    busy: bool,
    event_ids: BTreeSet<String>,
    chunks: BTreeSet<(String, String, u64)>,
    event_versions: BTreeMap<(String, String), u64>,
    cursor: Option<String>,
    retry: Option<(String, Option<String>, JobKind)>,
    conversations: BTreeMap<(Option<String>, Option<String>), String>,
    conversation_targets: BTreeMap<String, Option<String>>,
    prepared: Option<(Operation, Option<String>, String)>,
}

impl State {
    fn new() -> Self {
        Self {
            context: new_context(),
            views: BTreeMap::from([(None, WorkView::default())]),
            works: vec![],
            inbox: vec![],
            notice: t!("agent_center.welcome").into_owned(),
            completion: 0,
            pending: None,
            busy: false,
            event_ids: BTreeSet::new(),
            chunks: BTreeSet::new(),
            event_versions: BTreeMap::new(),
            cursor: None,
            retry: None,
            conversations: BTreeMap::new(),
            conversation_targets: BTreeMap::new(),
            prepared: None,
        }
    }
    fn view(&self) -> Option<&WorkView> {
        self.views.get(&self.context.work_id)
    }
    fn view_mut(&mut self) -> &mut WorkView {
        self.views.entry(self.context.work_id.clone()).or_default()
    }
    fn select(&mut self, work: Option<String>) {
        self.context.work_id = work;
        self.context.context_version += 1;
        self.views.entry(self.context.work_id.clone()).or_default();
        self.completion = 0;
        // A confirmation must never follow the user to a different work.
        self.pending = None;
        self.select_conversation();
    }
    fn select_conversation(&mut self) {
        self.context.conversation_id = self
            .conversations
            .entry((
                self.context.project_id.clone(),
                self.context.work_id.clone(),
            ))
            .or_insert_with(|| uuid::Uuid::new_v4().to_string())
            .clone();
    }
    fn append(&mut self, work: Option<String>, value: &Value) {
        let view = self.views.entry(work).or_default();
        if let Some(messages) = value.get("messages").and_then(Value::as_array) {
            for message in messages {
                if let Some(existing) = view
                    .messages
                    .iter_mut()
                    .find(|item| item["id"] == message["id"])
                {
                    *existing = message.clone();
                } else {
                    view.messages.push(message.clone());
                }
            }
            return;
        }
        let text = serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string());
        view.transcript.push(text);
        if view.transcript.len() > 200 {
            view.transcript.remove(0);
        }
    }
    fn observe(&mut self, response: &Value) {
        capture_versions(&mut self.context, response);
        if let Some(items) = response.pointer("/data/items").and_then(Value::as_array) {
            if !items.is_empty() && items.iter().all(|item| item.get("work").is_some()) {
                self.works = items.clone();
                for item in items {
                    self.remember_work(item);
                }
            }
        }
        if let Some(data) = response.get("data") {
            self.remember_work(data);
        }
        for (key, version) in &self.context.versions {
            let current = self.event_versions.entry(key.clone()).or_default();
            *current = (*current).max(*version);
        }
    }
    fn observe_inbox(&mut self, response: &Value, refresh: &InboxRefresh) {
        if status_exit_code(response) != 0 {
            return;
        }
        let Some(items) = response.pointer("/data/items").and_then(Value::as_array) else {
            return;
        };
        let scope = refresh.operation.params["workId"].as_str();
        // An in-flight list must not erase or resurrect obligations updated by
        // events since the read was queued. Keep tombstone versions as well.
        let changed = |id: &str, versions: &BTreeMap<(String, String), u64>| {
            let key = ("AttentionItem".into(), id.into());
            versions.get(&key).copied().unwrap_or(0)
                > refresh.versions.get(&key).copied().unwrap_or(0)
        };
        let after = refresh.operation.params["afterId"].as_str();
        let next = response
            .pointer("/data/nextAfterId")
            .and_then(Value::as_str);
        self.inbox.retain(|item| {
            let id = item["id"].as_str().unwrap_or("");
            scope.is_some_and(|work| item["workId"] != work)
                || after.is_some_and(|after| id <= after)
                || next.is_some_and(|next| id > next)
                || changed(id, &self.event_versions)
        });
        for item in items {
            let Some(id) = item["id"].as_str() else {
                continue;
            };
            if item["kind"] != "AttentionItem"
                || scope.is_some_and(|work| item["workId"] != work)
                || changed(id, &self.event_versions)
            {
                continue;
            }
            self.inbox.retain(|existing| existing["id"] != id);
            if attention_open(item) {
                self.inbox.push(item.clone());
            }
        }
        self.observe(response);
    }
    fn remember_work(&mut self, view: &Value) {
        let work = &view["work"];
        if let (Some(id), Some(version)) = (work["id"].as_str(), work["version"].as_u64()) {
            let current = self
                .context
                .versions
                .entry(("Work".into(), id.into()))
                .or_default();
            *current = (*current).max(version);
        }
        let candidate = &view["candidate"];
        if let (Some(id), Some(version)) = (candidate["id"].as_str(), candidate["version"].as_u64())
        {
            let current = self
                .context
                .versions
                .entry(("DeliveryCandidate".into(), id.into()))
                .or_default();
            *current = (*current).max(version);
        }
    }
    fn receive(&mut self, update: Update) {
        self.busy = false;
        match update.result {
            Ok(Outcome::Prepared(operation)) => {
                self.prepared = Some((operation, update.work, update.input));
            }
            Err(error) => {
                self.notice = format!("{error:#}");
                // Keep the exact submitted draft and explicit target on every failure.
            }
            Ok(Outcome::Response(response)) => {
                if response.get("operationId").is_none() || response["status"] != "error" {
                    self.retry = None;
                }
                self.observe(&response);
                self.notice = status_label(&response);
                if matches!(status_exit_code(&response), 0 | 2 | 3) {
                    let view = self.views.entry(update.work.clone()).or_default();
                    if view.draft == update.input {
                        view.draft.clear();
                    }
                    self.pending = None;
                }
                self.append(update.work, &response);
            }
            Ok(Outcome::Inbox { response, refresh }) => {
                self.retry = None;
                self.observe_inbox(&response, &refresh);
                self.notice = status_label(&response);
                if status_exit_code(&response) == 0 {
                    let view = self.views.entry(update.work.clone()).or_default();
                    if view.draft == update.input {
                        view.draft.clear();
                    }
                }
                self.append(update.work, &response);
            }
            Ok(Outcome::StartPreview { operation, preview }) => {
                self.observe(&preview);
                self.pending = Some(PendingConfirmation {
                    operation,
                    preview,
                    work: update.work,
                    input: update.input,
                });
                self.notice = t!("agent_center.confirm_prompt").into_owned();
            }
            Ok(Outcome::SelectedWork { id, response }) => {
                self.observe(&response);
                if status_exit_code(&response) == 0 {
                    let previous = self.views.entry(update.work).or_default();
                    if previous.draft == update.input
                        && update.input.trim_start().starts_with("/work use")
                    {
                        previous.draft.clear();
                    }
                    if let Some(project) = response
                        .pointer("/data/work/projectId")
                        .and_then(Value::as_str)
                    {
                        self.context.project_id = Some(project.into());
                    }
                    self.select(Some(id.clone()));
                    self.append(Some(id), &response);
                } else {
                    self.append(update.work, &response);
                }
                self.notice = status_label(&response);
                self.retry = None;
            }
            Ok(Outcome::SelectedProject { id, response }) => {
                self.observe(&response);
                if status_exit_code(&response) == 0 {
                    self.context.project_id = Some(id);
                    self.context.context_version += 1;
                    self.select_conversation();
                    let view = self.views.entry(update.work.clone()).or_default();
                    if view.draft == update.input {
                        view.draft.clear();
                    }
                }
                self.append(update.work, &response);
                self.notice = status_label(&response);
                self.retry = None;
            }
        }
    }

    fn event(&mut self, event: Value) {
        if event["type"] == "stream_error" {
            self.notice = format!("{}: {}", t!("agent_center.stream_failed"), event["failure"]);
            return;
        }
        let Some(id) = event["eventId"].as_str() else {
            return;
        };
        if !self.event_ids.insert(id.into()) {
            return;
        }
        capture_versions(&mut self.context, &event);
        let work = event["workId"].as_str().map(str::to_owned);
        if let Some(changes) = event["changes"].as_array() {
            for change in changes {
                let subject = &change["subject"];
                let key = (
                    subject["kind"].as_str().unwrap_or("").to_owned(),
                    subject["id"].as_str().unwrap_or("").to_owned(),
                );
                let version = subject["version"].as_u64().unwrap_or(0);
                let view = &change["view"];
                if let (Some(message), Some(part), Some(chunk)) = (
                    view["messageId"].as_str(),
                    view["partId"].as_str(),
                    view["chunkIndex"].as_u64(),
                ) {
                    if !self.chunks.insert((message.into(), part.into(), chunk)) {
                        continue;
                    }
                } else {
                    let previous = self.event_versions.entry(key).or_default();
                    if version <= *previous {
                        continue;
                    }
                    *previous = version;
                }
                if subject["kind"] == "Work" {
                    self.remember_work(view);
                    if let Some(existing) = self
                        .works
                        .iter_mut()
                        .find(|item| item["work"]["id"] == subject["id"])
                    {
                        *existing = view.clone();
                    } else if view.get("work").is_some() {
                        self.works.push(view.clone());
                    }
                }
                if subject["kind"] == "AttentionItem" {
                    self.inbox.retain(|item| item["id"] != subject["id"]);
                    if attention_open(view) {
                        self.inbox.push(view.clone());
                    }
                }
                let target = work.clone().or_else(|| {
                    if subject["kind"] == "Conversation" {
                        subject["id"]
                            .as_str()
                            .and_then(|id| self.conversation_targets.get(id))
                            .cloned()
                            .flatten()
                    } else {
                        None
                    }
                });
                self.append(target, view);
            }
        }
        self.cursor = event["cursor"].as_str().map(str::to_owned);
        // Routine progress/text is a silent view update. Human attention has a
        // separate, coalesced surface derived from current obligations.
    }
}

fn attention_open(item: &Value) -> bool {
    matches!(
        item["state"].as_str().or_else(|| item["status"].as_str()),
        Some("Open")
    )
}

fn status_label(response: &Value) -> String {
    match response["status"].as_str() {
        Some("ok") => t!("agent_center.status_ok").into_owned(),
        Some("pending") => t!(
            "agent_center.status_pending",
            id = response["operationId"].as_str().unwrap_or("")
        )
        .into_owned(),
        Some("needs_input") => t!(
            "agent_center.status_needs_input",
            id = response["inputRequest"]["id"].as_str().unwrap_or("")
        )
        .into_owned(),
        Some("conflict") => t!("agent_center.status_conflict").into_owned(),
        Some("unsupported") => t!("agent_center.status_unsupported").into_owned(),
        _ => t!("agent_center.status_error").into_owned(),
    }
}

#[derive(Clone)]
enum JobKind {
    Send(Operation),
    RefreshInbox(InboxRefresh),
    Resolve {
        args: Vec<String>,
        context: CommandContext,
        fallback_command_id: String,
    },
    PrepareStart(String),
    PrepareApply(commands::preparation::ApplyTarget),
    PrepareTransfer(commands::preparation::TransferTarget),
    PrepareProposal(commands::preparation::ProposalIntake),
    SelectWork(String),
    SelectProject(String),
}
struct Job {
    kind: JobKind,
    work: Option<String>,
    input: String,
    conversation: Option<String>,
}
enum Outcome {
    Prepared(Operation),
    Response(Value),
    Inbox {
        response: Value,
        refresh: InboxRefresh,
    },
    SelectedWork {
        id: String,
        response: Value,
    },
    SelectedProject {
        id: String,
        response: Value,
    },
    StartPreview {
        operation: Operation,
        preview: Value,
    },
}
struct Update {
    work: Option<String>,
    input: String,
    result: Result<Outcome>,
    conversation: Option<String>,
}

async fn prepare_action(
    client: &Client,
    work_id: &str,
    apply: Option<&commands::preparation::ApplyTarget>,
) -> Result<Outcome> {
    match commands::preparation::prepare(client, work_id, apply).await? {
        commands::preparation::Preparation::Ready { operation, preview } => {
            Ok(Outcome::StartPreview { operation, preview })
        }
        commands::preparation::Preparation::Response(response) => Ok(Outcome::Response(response)),
    }
}

async fn resolve_operation(
    args: Vec<String>,
    context: CommandContext,
    fallback_command_id: String,
) -> Result<Outcome> {
    let mut args = resolve_input_args(args, false).await?;
    if !args
        .iter()
        .any(|arg| arg == "--request-json" || arg == "--command-id")
    {
        args.extend(["--command-id".into(), fallback_command_id]);
    }
    match commands::compile(&args, &context, true)? {
        Action::Operation(operation) if operation.confirmation => Ok(Outcome::StartPreview {
            preview: operation_preview(&operation),
            operation,
        }),
        Action::Operation(operation) => Ok(Outcome::Prepared(operation)),
        _ => bail!("{}", t!("agent_center.invalid_request")),
    }
}

fn operation_preview(operation: &Operation) -> Value {
    json!({"method":operation.method,"params":operation.params,"ifMatch":operation.if_match,
        "commandId":operation.command_id})
}

struct TerminalGuard;
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if let Err(error) = disable_raw_mode() {
            tracing::warn!(target:"wta::agent_center", %error, "Unable to restore terminal raw mode");
        }
        if let Err(error) = execute!(
            std::io::stdout(),
            DisableBracketedPaste,
            LeaveAlternateScreen
        ) {
            tracing::warn!(target:"wta::agent_center", %error, "Unable to leave Agent Center screen");
        }
    }
}

pub async fn run_async() -> Result<()> {
    // Fail before taking over the terminal. Closing this client does not stop work.
    let connection = Arc::new(
        Client::connect()
            .await
            .with_context(|| t!("agent_center.service_unavailable").into_owned())?,
    );
    let initial = send(
        &connection,
        &Operation::read("work.list", json!({"limit":100})),
    )
    .await?;
    if status_exit_code(&initial) != 0 {
        bail!("{}: {}", t!("agent_center.service_unavailable"), initial);
    }
    let subscription = send(
        &connection,
        &Operation::read("events.subscribe", json!({"scope":{"kind":"WorkList"}})),
    )
    .await?;
    if status_exit_code(&subscription) != 0 {
        bail!("{}: {}", t!("agent_center.stream_failed"), subscription);
    }
    let (jobs_tx, mut jobs_rx) = mpsc::unbounded_channel::<Job>();
    let (updates_tx, mut updates_rx) = mpsc::unbounded_channel::<Update>();
    let worker_connection = connection.clone();
    let worker = tokio::spawn(async move {
        while let Some(job) = jobs_rx.recv().await {
            let result = match job.kind {
                JobKind::Send(operation) => send(&worker_connection, &operation)
                    .await
                    .map(Outcome::Response),
                JobKind::RefreshInbox(refresh) => send(&worker_connection, &refresh.operation)
                    .await
                    .map(|response| Outcome::Inbox { response, refresh }),
                JobKind::Resolve {
                    args,
                    context,
                    fallback_command_id,
                } => resolve_operation(args, context, fallback_command_id).await,
                JobKind::PrepareStart(work) => {
                    prepare_action(&worker_connection, &work, None).await
                }
                JobKind::PrepareApply(target) => {
                    prepare_action(&worker_connection, &target.work_id, Some(&target)).await
                }
                JobKind::PrepareTransfer(target) => {
                    match commands::preparation::prepare_transfer(&worker_connection, &target).await
                    {
                        Ok(commands::preparation::Preparation::Ready { operation, preview }) => {
                            Ok(Outcome::StartPreview { operation, preview })
                        }
                        Ok(commands::preparation::Preparation::Response(response)) => {
                            Ok(Outcome::Response(response))
                        }
                        Err(error) => Err(error),
                    }
                }
                JobKind::PrepareProposal(intake) => {
                    match commands::preparation::prepare_proposal(&worker_connection, &intake).await
                    {
                        Ok(commands::preparation::Preparation::Ready { operation, .. }) => {
                            Ok(Outcome::Prepared(operation))
                        }
                        Ok(commands::preparation::Preparation::Response(response)) => {
                            Ok(Outcome::Response(response))
                        }
                        Err(error) => Err(error),
                    }
                }
                JobKind::SelectWork(id) => send(
                    &worker_connection,
                    &Operation::read("work.get", json!({"workId":id})),
                )
                .await
                .map(|response| Outcome::SelectedWork { id, response }),
                JobKind::SelectProject(id) => send(
                    &worker_connection,
                    &Operation::read("project.get", json!({"projectId":id})),
                )
                .await
                .map(|response| Outcome::SelectedProject { id, response }),
            };
            if updates_tx
                .send(Update {
                    work: job.work,
                    input: job.input,
                    conversation: job.conversation,
                    result,
                })
                .is_err()
            {
                break;
            }
        }
    });
    enable_raw_mode()?;
    let _guard = TerminalGuard;
    execute!(
        std::io::stdout(),
        EnterAlternateScreen,
        EnableBracketedPaste
    )?;
    let mut terminal = Terminal::new(CrosstermBackend::new(std::io::stdout()))?;
    let mut state = State::new();
    state.observe(&initial);
    state.append(None, &initial);
    if state.works.is_empty() {
        let projects = send(
            &connection,
            &Operation::read("project.list", json!({"limit":100})),
        )
        .await?;
        state.append(None, &projects);
    }
    let inbox = send(
        &connection,
        &Operation::read("inbox.list", json!({"limit":100})),
    )
    .await?;
    state.observe_inbox(
        &inbox,
        &InboxRefresh {
            operation: Operation::read("inbox.list", json!({"limit":100})),
            versions: state.event_versions.clone(),
        },
    );
    if status_exit_code(&inbox) != 0 {
        state.append(None, &inbox);
    }
    if let Some(snapshot) = subscription.pointer("/data/snapshot") {
        state.observe(&json!({"data":snapshot}));
    }
    let mut input = EventStream::new();
    let mut conversation_subscriptions = BTreeSet::new();
    let mut events_open = true;
    let mut subscriptions = BTreeMap::new();
    if let Some(id) = subscription
        .pointer("/data/subscriptionId")
        .and_then(Value::as_str)
    {
        subscriptions.insert(id.to_owned(), json!({"kind":"WorkList"}));
    }
    loop {
        terminal.draw(|frame| render(frame, &state))?;
        tokio::select! {
            event = connection.next_event(), if events_open => {
                match event {
                    Ok(event) => {
                        if event["type"] == "stream_error" {
                            state.event(event.clone());
                            if let Some(scope) = event["subscriptionId"].as_str()
                                .and_then(|id| subscriptions.remove(id)) {
                                match send(&connection, &Operation::read("events.subscribe", json!({"scope":scope}))).await {
                                    Ok(response) if status_exit_code(&response) == 0 => {
                                        if let Some(snapshot) = response.pointer("/data/snapshot") {
                                            state.observe(&json!({"data":snapshot}));
                                            state.append(state.context.work_id.clone(), snapshot);
                                        }
                                        if let Some(id) = response.pointer("/data/subscriptionId").and_then(Value::as_str) {
                                            subscriptions.insert(id.to_owned(), scope);
                                        }
                                    }
                                    Ok(response) => state.append(state.context.work_id.clone(), &response),
                                    Err(error) => state.notice = format!("{}: {error:#}", t!("agent_center.stream_failed")),
                                }
                            }
                        } else { state.event(event); }
                    },
                    Err(error) => {
                        events_open = false;
                        state.notice = format!("{}: {error:#}", t!("agent_center.stream_failed"));
                    }
                }
            }
            update = updates_rx.recv() => {
                if let Some(update) = update {
                    let submitted_conversation = update.conversation.clone();
                    let intake_recorded = matches!(&update.result,
                        Ok(Outcome::Response(response)) if response.pointer("/data/intakeTurnId").is_some());
                    state.receive(update);
                    if let Some((operation, work, input)) = state.prepared.take() {
                        if let Err(error) = queue_captured(&mut state, &jobs_tx, JobKind::Send(operation), work, input) {
                            state.notice = format!("{error:#}");
                        }
                    }
                    if let Some(conversation) = submitted_conversation.filter(|id| intake_recorded && !conversation_subscriptions.contains(id)) {
                        let subscription = send(&connection, &Operation::read("events.subscribe",
                            json!({"scope":{"kind":"Conversation","id":conversation}}))).await;
                        match subscription {
                            Ok(response) => {
                                if status_exit_code(&response) == 0 {
                                    conversation_subscriptions.insert(conversation.clone());
                                }
                                if let Some(id) = response.pointer("/data/subscriptionId").and_then(Value::as_str) {
                                    subscriptions.insert(id.to_owned(), json!({"kind":"Conversation","id":conversation}));
                                }
                                if let Some(snapshot) = response.pointer("/data/snapshot") {
                                    state.append(state.context.work_id.clone(), snapshot);
                                } else { state.append(state.context.work_id.clone(), &response); }
                            }
                            Err(error) => state.notice = format!("{}: {error:#}", t!("agent_center.stream_failed")),
                        }
                    }
                }
            }
            event = input.next() => {
                let Some(event) = event else { break; };
                match event? {
                    Event::Key(key) if key.kind != KeyEventKind::Release => {
                        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                            break;
                        }
                        if let Err(error) = handle_key(&mut state, key.code, key.modifiers, &jobs_tx) {
                            state.notice = format!("{error:#}");
                        }
                    }
                    Event::Paste(text) => state.view_mut().draft.push_str(&text),
                    _ => {}
                }
            }
        }
    }
    worker.abort();
    terminal.show_cursor()?;
    Ok(())
}

fn handle_key(
    state: &mut State,
    key: KeyCode,
    modifiers: KeyModifiers,
    jobs: &mpsc::UnboundedSender<Job>,
) -> Result<()> {
    if state.pending.is_some() {
        match key {
            KeyCode::Esc => {
                state.pending = None;
                state.retry = None;
                state.notice = t!("agent_center.confirm_cancelled").into_owned();
            }
            KeyCode::Enter if modifiers.contains(KeyModifiers::CONTROL) && !state.busy => {
                if let Some(pending) = state.pending.as_ref() {
                    let kind = JobKind::Send(pending.operation.clone());
                    let work = pending.work.clone();
                    let input = pending.input.clone();
                    queue_captured(state, jobs, kind, work, input)?;
                }
            }
            KeyCode::PageUp => {
                state.view_mut().follow = false;
                state.view_mut().scroll = state.view_mut().scroll.saturating_sub(10);
            }
            KeyCode::PageDown => {
                state.view_mut().scroll = state.view_mut().scroll.saturating_add(10)
            }
            _ => {}
        }
        return Ok(());
    }
    let draft = state.view().map(|view| view.draft.as_str()).unwrap_or("");
    let suggestions = if draft.trim_start().starts_with('/') {
        commands::complete_with_context(draft, &state.context)
    } else {
        vec![]
    };
    match key {
        KeyCode::Char(c) if !modifiers.contains(KeyModifiers::CONTROL) => {
            state.view_mut().draft.push(c);
            state.completion = 0;
        }
        KeyCode::Backspace => {
            state.view_mut().draft.pop();
            state.completion = 0;
        }
        KeyCode::Up if !suggestions.is_empty() => {
            state.completion = state.completion.saturating_sub(1);
        }
        KeyCode::Down if !suggestions.is_empty() => {
            state.completion = (state.completion + 1).min(suggestions.len() - 1);
        }
        KeyCode::Tab if !suggestions.is_empty() => {
            state.view_mut().draft = format!(
                "{} ",
                suggestions[state.completion.min(suggestions.len() - 1)]
            );
        }
        KeyCode::PageUp => {
            state.view_mut().follow = false;
            state.view_mut().scroll = state.view_mut().scroll.saturating_sub(10);
        }
        KeyCode::PageDown => {
            state.view_mut().follow = false;
            state.view_mut().scroll = state.view_mut().scroll.saturating_add(10);
        }
        KeyCode::End if modifiers.contains(KeyModifiers::CONTROL) => state.view_mut().follow = true,
        KeyCode::F(2) if !state.busy && !state.works.is_empty() => {
            let current = state
                .works
                .iter()
                .position(|view| view["work"]["id"].as_str() == state.context.work_id.as_deref());
            let next = current
                .map(|index| (index + 1) % state.works.len())
                .unwrap_or(0);
            if let Some(id) = state.works[next]["work"]["id"].as_str() {
                queue(state, jobs, JobKind::SelectWork(id.into()))?;
            }
        }
        KeyCode::Esc => {
            state.completion = 0;
        }
        KeyCode::Enter if modifiers.contains(KeyModifiers::SHIFT) => {
            state.view_mut().draft.push('\n')
        }
        KeyCode::Enter if !state.busy => submit(state, jobs)?,
        _ => {}
    }
    Ok(())
}

fn queue(state: &mut State, jobs: &mpsc::UnboundedSender<Job>, kind: JobKind) -> Result<()> {
    let work = state.context.work_id.clone();
    let input = state
        .view()
        .map(|view| view.draft.clone())
        .unwrap_or_default();
    queue_captured(state, jobs, kind, work, input)
}

fn queue_captured(
    state: &mut State,
    jobs: &mpsc::UnboundedSender<Job>,
    kind: JobKind,
    work: Option<String>,
    input: String,
) -> Result<()> {
    let kind = match kind {
        JobKind::Send(operation) if operation.method == "inbox.list" => {
            JobKind::RefreshInbox(InboxRefresh {
                operation,
                versions: state.event_versions.clone(),
            })
        }
        other => other,
    };
    let conversation = if let JobKind::Send(operation) = &kind {
        operation.params["conversationId"]
            .as_str()
            .map(str::to_owned)
    } else {
        None
    };
    if let Some(id) = &conversation {
        let target = if let JobKind::Send(operation) = &kind {
            operation
                .params
                .pointer("/context/selectedWorkId")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or_else(|| work.clone())
        } else {
            work.clone()
        };
        state.conversation_targets.insert(id.clone(), target);
    }
    if matches!(
        &kind,
        JobKind::Send(_) | JobKind::Resolve { .. } | JobKind::PrepareTransfer(_)
    ) {
        state.retry = Some((input.clone(), work.clone(), kind.clone()));
    }
    jobs.send(Job {
        kind,
        conversation,
        work,
        input,
    })
    .with_context(|| t!("agent_center.worker_stopped").into_owned())?;
    state.busy = true;
    state.notice = t!("agent_center.sending").into_owned();
    Ok(())
}

fn submit(state: &mut State, jobs: &mpsc::UnboundedSender<Job>) -> Result<()> {
    let input = state
        .view()
        .map(|view| view.draft.clone())
        .unwrap_or_default();
    if let Some((draft, work, kind)) = &state.retry {
        if draft == &input && work == &state.context.work_id {
            return queue(state, jobs, kind.clone());
        }
    }
    let action = match commands::parse_input(&input)? {
        Input::Text(text) => {
            Action::Operation(commands::conversation(text, &state.context, false)?)
        }
        Input::Command(args) => {
            let external_input = args
                .iter()
                .take_while(|arg| arg.as_str() != "--")
                .any(|arg| arg == "--input-json");
            if external_input && !args.last().is_some_and(|arg| arg == "--help") {
                let context = state.context.clone();
                return queue(
                    state,
                    jobs,
                    JobKind::Resolve {
                        args,
                        context,
                        fallback_command_id: uuid::Uuid::new_v4().to_string(),
                    },
                );
            }
            commands::compile(&args, &state.context, true)?
        }
    };
    match action {
        Action::Home => {
            state.view_mut().draft.clear();
            state.select(None);
        }
        Action::Help(prefix) => {
            state.append(state.context.work_id.clone(), &commands::help(&prefix));
            state.view_mut().draft.clear();
        }
        Action::SelectWork(work) => {
            queue(state, jobs, JobKind::SelectWork(work))?;
        }
        Action::SelectProject(project) => {
            queue(state, jobs, JobKind::SelectProject(project))?;
        }
        Action::PrepareStart(work) => queue(state, jobs, JobKind::PrepareStart(work))?,
        Action::PrepareApply(target) => queue(state, jobs, JobKind::PrepareApply(target))?,
        Action::PrepareTransfer(target) => queue(state, jobs, JobKind::PrepareTransfer(target))?,
        Action::PrepareProposal(intake) => queue(state, jobs, JobKind::PrepareProposal(intake))?,
        Action::Unsupported(command) => {
            let (code, message) = commands::unsupported_failure(&command);
            bail!("{code}: {message}")
        }
        Action::Operation(operation) => {
            if operation.confirmation {
                state.pending = Some(PendingConfirmation {
                    preview: operation_preview(&operation),
                    operation,
                    work: state.context.work_id.clone(),
                    input,
                });
                state.notice = t!("agent_center.confirm_prompt").into_owned();
            } else {
                queue(state, jobs, JobKind::Send(operation))?;
            }
        }
    }
    Ok(())
}

fn render(frame: &mut ratatui::Frame<'_>, state: &State) {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(3),
            Constraint::Length(5),
        ])
        .split(frame.area());
    let work = state.context.work_id.as_deref().unwrap_or("—");
    let project = state.context.project_id.as_deref().unwrap_or("—");
    frame.render_widget(
        Paragraph::new(t!("agent_center.header", work = work, project = project).into_owned())
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(t!("agent_center.title").into_owned()),
            ),
        vertical[0],
    );
    let middle = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(25), Constraint::Percentage(75)])
        .split(vertical[1]);
    let mut works = state
        .works
        .iter()
        .map(|view| {
            let work = &view["work"];
            format!(
                "{}\n{}\n{}\n",
                work["id"].as_str().unwrap_or(""),
                view["spec"]["goal"]
                    .as_str()
                    .or_else(|| work["goal"].as_str())
                    .unwrap_or(""),
                work["lifecycle"].as_str().unwrap_or("")
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    works.push_str(&format!(
        "\n{}\n",
        t!("agent_center.inbox_count", count = state.inbox.len())
    ));
    for item in state.inbox.iter().take(100) {
        works.push_str(&format!(
            "{}\n{}\n{}\n\n",
            item["id"].as_str().unwrap_or(""),
            item["workId"].as_str().unwrap_or(""),
            item["reason"]
                .as_str()
                .or_else(|| item["question"].as_str())
                .unwrap_or("")
        ));
    }
    frame.render_widget(
        Paragraph::new(works).wrap(Wrap { trim: false }).block(
            Block::default()
                .borders(Borders::ALL)
                .title(t!("agent_center.works").into_owned()),
        ),
        middle[0],
    );
    let body = if let Some(pending) = &state.pending {
        serde_json::to_string_pretty(&pending.preview).unwrap_or_default()
    } else {
        state
            .view()
            .map(|view| {
                let messages = view
                    .messages
                    .iter()
                    .map(|message| {
                        let mut text = message["text"].as_str().unwrap_or("").to_owned();
                        if let Some(parts) = message["parts"].as_array() {
                            for part in parts {
                                text.push_str(part["text"].as_str().unwrap_or(""));
                            }
                        }
                        format!(
                            "{} · {} · {}\n{}",
                            message["role"].as_str().unwrap_or(""),
                            message["id"].as_str().unwrap_or(""),
                            message["status"].as_str().unwrap_or(""),
                            text
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n");
                format!("{}\n\n{messages}", view.transcript.join("\n\n"))
            })
            .unwrap_or_default()
    };
    let paragraph = Paragraph::new(body).wrap(Wrap { trim: false });
    let max_scroll = paragraph
        .line_count(middle[1].width.saturating_sub(2))
        .saturating_sub(middle[1].height.saturating_sub(2) as usize)
        .min(u16::MAX as usize) as u16;
    let scroll = if state.pending.is_some() {
        state.view().map(|view| view.scroll).unwrap_or(0)
    } else {
        state
            .view()
            .map(|view| {
                if view.follow {
                    max_scroll
                } else {
                    view.scroll.min(max_scroll)
                }
            })
            .unwrap_or(0)
    };
    frame.render_widget(
        paragraph
            .scroll((scroll, 0))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(if state.pending.is_some() {
                        t!("agent_center.confirm_title").into_owned()
                    } else {
                        t!("agent_center.activity").into_owned()
                    }),
            ),
        middle[1],
    );
    frame.render_widget(
        Paragraph::new(if state.inbox.is_empty() {
            state.notice.clone()
        } else {
            format!(
                "{}\n{}",
                state.notice,
                t!("agent_center.inbox_count", count = state.inbox.len())
            )
        })
        .wrap(Wrap { trim: false })
        .style(Style::default().fg(Color::Yellow)),
        vertical[2],
    );
    let draft = state.view().map(|view| view.draft.as_str()).unwrap_or("");
    let suggestions = commands::complete_with_context(draft, &state.context);
    let hint = if draft.starts_with('/') {
        suggestions
            .get(state.completion)
            .map(String::as_str)
            .unwrap_or("")
    } else {
        ""
    };
    let input_width = vertical[3].width.saturating_sub(2).max(1);
    let input_height = vertical[3].height.saturating_sub(2).max(1);
    let draft_lines = Paragraph::new(draft)
        .wrap(Wrap { trim: false })
        .line_count(input_width)
        .max(1);
    let input_scroll = draft_lines.saturating_sub(input_height.saturating_sub(1).max(1) as usize);
    frame.render_widget(
        Paragraph::new(format!("{draft}\n{hint}"))
            .wrap(Wrap { trim: false })
            .scroll((input_scroll.min(u16::MAX as usize) as u16, 0))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(t!("agent_center.input_help").into_owned()),
            ),
        vertical[3],
    );
    if state.pending.is_none() && vertical[3].width > 2 && vertical[3].height > 2 {
        let column = unicode_width::UnicodeWidthStr::width(draft.rsplit('\n').next().unwrap_or(""))
            as u16
            % input_width;
        let row = draft_lines
            .saturating_sub(1)
            .saturating_sub(input_scroll)
            .min(input_height.saturating_sub(1) as usize) as u16;
        frame.set_cursor_position((vertical[3].x + 1 + column, vertical[3].y + 1 + row));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attention_event(id: &str, work: &str, version: u64, status: &str) -> Value {
        json!({"type":"event","eventId":format!("{id}-{version}"),"cursor":format!("store:{version}"),
            "kind":"AttentionChanged","workId":work,"changes":[{
                "subject":{"kind":"AttentionItem","id":id,"version":version},
                "view":{"kind":"AttentionItem","id":id,"workId":work,"version":version,
                    "status":status,"reason":"DecisionRequested","subjectId":format!("decision-{work}")}}]})
    }

    #[test]
    fn scoped_and_empty_inbox_refreshes_preserve_other_work_and_event_tombstones() {
        let _locale = crate::test_support::lock_locale();
        let mut state = State::new();
        state.event(attention_event("attention-a", "a", 1, "Open"));
        state.event(attention_event("attention-b", "b", 1, "Open"));
        let refresh = InboxRefresh {
            operation: Operation::read("inbox.list", json!({"workId":"a","limit":100})),
            versions: state.event_versions.clone(),
        };
        let item = state.inbox[0].clone();
        state.observe_inbox(
            &json!({"status":"ok","data":{"items":[item.clone()]}}),
            &refresh,
        );
        assert_eq!(state.inbox.len(), 2);
        state.observe_inbox(&json!({"status":"ok","data":{"items":[]}}), &refresh);
        assert_eq!(state.inbox.len(), 1);
        assert_eq!(state.inbox[0]["workId"], "b");
        state.event(attention_event("attention-a", "a", 2, "Resolved"));
        state.observe_inbox(&json!({"status":"ok","data":{"items":[item]}}), &refresh);
        assert_eq!(state.inbox.len(), 1);
        let global = InboxRefresh {
            operation: Operation::read("inbox.list", json!({"limit":100})),
            versions: state.event_versions.clone(),
        };
        state.event(attention_event("attention-c", "c", 1, "Open"));
        state.observe_inbox(&json!({"status":"ok","data":{"items":[]}}), &global);
        assert_eq!(state.inbox.len(), 1);
        assert_eq!(state.inbox[0]["workId"], "c");
    }

    #[test]
    fn routine_flood_is_silent_and_attention_is_coalesced_without_stealing_b_draft() {
        let _locale = crate::test_support::lock_locale();
        let mut state = State::new();
        state.select(Some("b".into()));
        state.view_mut().draft = "unfinished b".into();
        state.view_mut().scroll = 9;
        state.view_mut().follow = false;
        state.notice = "existing notice".into();
        state.event(attention_event("attention-a", "a", 1, "Open"));
        for version in 2..502 {
            state.event(attention_event("attention-a", "a", version, "Open"));
            state.event(json!({"eventId":format!("progress-{version}"),"kind":"ProgressReported",
                "workId":"a","changes":[{"subject":{"kind":"ProgressReport","id":"report","version":version},
                    "view":{"text":"progress"}}]}));
            state.event(json!({"eventId":format!("text-{version}"),"kind":"TextDelta","workId":"a",
                "changes":[{"subject":{"kind":"Conversation","id":"conversation","version":version},
                    "view":{"messageId":"message","partId":"part","chunkIndex":version,"text":"text"}}]}));
        }
        assert_eq!(state.inbox.len(), 1);
        assert_eq!(state.notice, "existing notice");
        assert_eq!(state.context.work_id.as_deref(), Some("b"));
        assert_eq!(state.view().unwrap().draft, "unfinished b");
        assert_eq!(state.view().unwrap().scroll, 9);
        assert!(!state.view().unwrap().follow);
        assert_eq!(state.views[&Some("a".into())].transcript.len(), 200);
        state.event(attention_event("attention-b", "b", 1, "Open"));
        state.event(attention_event("attention-a", "a", 502, "Resolved"));
        assert_eq!(state.inbox.len(), 1);
        assert_eq!(state.inbox[0]["workId"], "b");
    }

    #[test]
    fn answering_a_decision_leaves_b_draft_and_unrelated_attention_intact() {
        let _locale = crate::test_support::lock_locale();
        let mut state = State::new();
        state.select(Some("b".into()));
        state.view_mut().draft = "unfinished b".into();
        state.view_mut().scroll = 14;
        state.event(attention_event("attention-a", "a", 1, "Open"));
        state.event(attention_event("attention-b", "b", 1, "Open"));
        state.select(Some("a".into()));
        state
            .context
            .versions
            .insert(("DecisionRequest".into(), "decision-a".into()), 3);
        state.view_mut().draft = "/decision answer decision-a --value true".into();
        let (jobs, mut receiver) = mpsc::unbounded_channel();
        submit(&mut state, &jobs).unwrap();
        let job = receiver.try_recv().unwrap();
        let JobKind::Send(operation) = job.kind else {
            panic!("expected answer")
        };
        assert_eq!(
            operation.params,
            json!({"decisionId":"decision-a","value":true})
        );
        assert_eq!(
            operation.if_match,
            [json!({"kind":"DecisionRequest","id":"decision-a","version":3})]
        );
        state.select(Some("b".into()));
        state.event(attention_event("attention-a", "a", 2, "Resolved"));
        state.receive(Update {
            work: job.work,
            input: job.input,
            conversation: None,
            result: Ok(Outcome::Response(
                json!({"status":"ok","data":{},"subjects":[]}),
            )),
        });
        assert_eq!(state.context.work_id.as_deref(), Some("b"));
        assert_eq!(state.view().unwrap().draft, "unfinished b");
        assert_eq!(state.view().unwrap().scroll, 14);
        assert_eq!(state.inbox.len(), 1);
        assert_eq!(state.inbox[0]["id"], "attention-b");
    }

    #[test]
    fn transfer_preparation_confirmation_retry_and_cancel_keep_frozen_target_and_drafts() {
        let _locale = crate::test_support::lock_locale();
        let mut state = State::new();
        state.select(Some("a".into()));
        state
            .context
            .versions
            .insert(("Work".into(), "a".into()), 7);
        let draft = "/workspace handback --summary 'updated input' --resume-affected false";
        state.view_mut().draft = draft.into();
        let (jobs, mut receiver) = mpsc::unbounded_channel();
        submit(&mut state, &jobs).unwrap();
        let job = receiver.try_recv().unwrap();
        let JobKind::PrepareTransfer(target) = job.kind else {
            panic!("expected preparation")
        };
        assert_eq!(target.work_id, "a");
        assert_eq!(target.work_version, Some(7));
        assert_eq!(target.handback, Some(("updated input".into(), false)));
        state.select(Some("b".into()));
        state.view_mut().draft = "unfinished b".into();
        let operation = Operation {
            method: "workspace.handback".into(),
            params: json!({"workspaceId":"workspace-a","summary":"updated input","resumeAffected":false}),
            if_match: vec![json!({"kind":"Workspace","id":"workspace-a","version":8})],
            mutation: true,
            confirmation: true,
            command_id: target.command_id,
        };
        let frozen = operation_preview(&operation);
        state.receive(Update {
            work: job.work,
            input: job.input,
            conversation: None,
            result: Ok(Outcome::StartPreview {
                operation,
                preview: frozen.clone(),
            }),
        });
        state.event(attention_event("attention-a", "a", 1, "Open"));
        assert_eq!(state.pending.as_ref().unwrap().preview, frozen);
        handle_key(&mut state, KeyCode::Enter, KeyModifiers::NONE, &jobs).unwrap();
        assert!(receiver.try_recv().is_err());
        handle_key(&mut state, KeyCode::Enter, KeyModifiers::CONTROL, &jobs).unwrap();
        let sent = receiver.try_recv().unwrap();
        let JobKind::Send(sent_operation) = sent.kind else {
            panic!("expected send")
        };
        assert_eq!(operation_preview(&sent_operation), frozen);
        state.receive(Update {
            work: sent.work,
            input: sent.input,
            conversation: None,
            result: Err(anyhow::anyhow!("transport failed")),
        });
        state
            .context
            .versions
            .insert(("Workspace".into(), "workspace-a".into()), 99);
        handle_key(&mut state, KeyCode::Enter, KeyModifiers::CONTROL, &jobs).unwrap();
        let retried = receiver.try_recv().unwrap();
        let JobKind::Send(retried_operation) = retried.kind else {
            panic!("expected frozen retry")
        };
        assert_eq!(operation_preview(&retried_operation), frozen);
        state.receive(Update {
            work: retried.work,
            input: retried.input,
            conversation: None,
            result: Ok(Outcome::Response(json!({"status":"conflict"}))),
        });
        handle_key(&mut state, KeyCode::Esc, KeyModifiers::NONE, &jobs).unwrap();
        assert!(state.pending.is_none());
        assert!(state.retry.is_none());
        assert_eq!(state.context.work_id.as_deref(), Some("b"));
        assert_eq!(state.view().unwrap().draft, "unfinished b");
        assert_eq!(state.views[&Some("a".into())].draft, draft);
    }

    #[test]
    fn work_switch_restores_independent_drafts_and_reading_positions() {
        let _locale = crate::test_support::lock_locale();
        let mut state = State::new();
        state.select(Some("a".into()));
        state.view_mut().draft = "unfinished a".into();
        state.view_mut().scroll = 12;
        state.select(Some("b".into()));
        state.view_mut().draft = "unfinished b".into();
        state.select(Some("a".into()));
        assert_eq!(state.view().unwrap().draft, "unfinished a");
        assert_eq!(state.view().unwrap().scroll, 12);
    }
    #[test]
    fn background_response_cannot_steal_input_or_selection() {
        let _locale = crate::test_support::lock_locale();
        let mut state = State::new();
        state.select(Some("a".into()));
        state.view_mut().draft = "new draft".into();
        state.receive(Update {
            work: Some("b".into()),
            conversation: None,
            input: "old draft".into(),
            result: Ok(Outcome::Response(
                json!({"status":"ok","data":{},"subjects":[]}),
            )),
        });
        assert_eq!(state.context.work_id.as_deref(), Some("a"));
        assert_eq!(state.view().unwrap().draft, "new draft");
    }
    #[test]
    fn errors_and_conflicts_preserve_input() {
        let _locale = crate::test_support::lock_locale();
        let mut state = State::new();
        state.view_mut().draft = "/review accept candidate".into();
        state.receive(Update {
            work: None,
            conversation: None,
            input: "/review accept candidate".into(),
            result: Ok(Outcome::Response(
                json!({"status":"conflict","subjects":[]}),
            )),
        });
        assert_eq!(state.view().unwrap().draft, "/review accept candidate");
    }

    #[test]
    fn events_are_deduplicated_without_stealing_confirmation_or_draft() {
        let _locale = crate::test_support::lock_locale();
        let mut state = State::new();
        state.select(Some("a".into()));
        state.view_mut().draft = "answer the question in a".into();
        state.pending = Some(PendingConfirmation {
            operation: Operation::read("work.get", json!({"workId":"a"})),
            preview: json!({"workId":"a","version":2}),
            work: Some("a".into()),
            input: "answer the question in a".into(),
        });
        let event = json!({"type":"event","eventId":"event-1","cursor":"store:1",
        "kind":"MessageDelta","workId":"b","changes":[{
            "subject":{"kind":"Conversation","id":"conversation","version":1},
            "view":{"messageId":"message","partId":"part","chunkIndex":0,"text":"hello"}
        }]});
        state.event(event.clone());
        state.event(event.clone());
        let mut duplicate_chunk = event;
        duplicate_chunk["eventId"] = json!("event-2");
        state.event(duplicate_chunk);
        assert_eq!(state.views[&Some("b".into())].transcript.len(), 1);
        assert_eq!(state.context.work_id.as_deref(), Some("a"));
        assert_eq!(state.view().unwrap().draft, "answer the question in a");
        assert_eq!(state.pending.as_ref().unwrap().preview["version"], 2);
    }

    #[test]
    fn failed_navigation_preserves_selection_and_command() {
        let _locale = crate::test_support::lock_locale();
        let mut state = State::new();
        state.select(Some("a".into()));
        state.view_mut().draft = "/work use missing".into();
        state.receive(Update {
            work: Some("a".into()),
            conversation: None,
            input: "/work use missing".into(),
            result: Ok(Outcome::SelectedWork {
                id: "missing".into(),
                response: json!({"status":"error","subjects":[]}),
            }),
        });
        assert_eq!(state.context.work_id.as_deref(), Some("a"));
        assert_eq!(state.view().unwrap().draft, "/work use missing");
    }

    #[test]
    fn keyboard_switch_preserves_unsent_message_and_restores_target_draft() {
        let _locale = crate::test_support::lock_locale();
        let mut state = State::new();
        state.select(Some("b".into()));
        state.view_mut().draft = "draft b".into();
        state.select(Some("a".into()));
        state.view_mut().draft = "draft a".into();
        state.receive(Update { work:Some("a".into()), input:"draft a".into(), conversation:None,
            result:Ok(Outcome::SelectedWork { id:"b".into(),
                response:json!({"status":"ok","subjects":[],"data":{"work":{"id":"b","version":1,"projectId":"p"}}}) }) });
        assert_eq!(state.view().unwrap().draft, "draft b");
        assert_eq!(state.views[&Some("a".into())].draft, "draft a");
    }

    #[test]
    fn renders_service_data_and_exact_confirmation_without_sample_cards() {
        let _locale = crate::test_support::lock_locale();
        let mut state = State::new();
        state.works.push(
            json!({"work":{"id":"work-real","lifecycle":"Active","phase":"OBSOLETE_PHASE"},
            "spec":{"goal":"Requested goal"}}),
        );
        state.append(
            None,
            &json!({"status":"pending","operationId":"operation-real"}),
        );
        let backend = ratatui::backend::TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, &state)).unwrap();
        let screen = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(screen.contains("operation-real"));
        assert!(screen.contains("Requested goal"));
        assert!(screen.contains("Active"));
        assert!(!screen.contains("OBSOLETE_PHASE"));
    }

    #[test]
    fn asynchronous_input_preparation_keeps_original_target_and_draft() {
        let _locale = crate::test_support::lock_locale();
        let mut state = State::new();
        state.select(Some("a".into()));
        let submitted = "/workspace takeover --input-json 'transfer file.json'";
        state.view_mut().draft = submitted.into();
        let (jobs, mut receiver) = mpsc::unbounded_channel();
        submit(&mut state, &jobs).unwrap();
        let queued = receiver.try_recv().unwrap();
        let JobKind::Resolve { context, .. } = queued.kind else {
            panic!("expected asynchronous file load");
        };
        assert_eq!(context.work_id.as_deref(), Some("a"));
        state.select(Some("b".into()));
        state.view_mut().draft = "new unsent draft".into();
        state.receive(Update {
            work: Some("a".into()),
            input: submitted.into(),
            conversation: None,
            result: Ok(Outcome::StartPreview {
                operation: Operation::read(
                    "workspace.takeover",
                    json!({"workspaceId":"workspace-a"}),
                ),
                preview: json!({"workspaceId":"workspace-a"}),
            }),
        });
        handle_key(&mut state, KeyCode::Enter, KeyModifiers::CONTROL, &jobs).unwrap();
        let confirmation = receiver.try_recv().unwrap();
        assert_eq!(confirmation.work.as_deref(), Some("a"));
        assert_eq!(confirmation.input, submitted);
        state.receive(Update {
            work: confirmation.work,
            input: confirmation.input,
            conversation: None,
            result: Ok(Outcome::Response(
                json!({"status":"ok","data":{},"subjects":[]}),
            )),
        });
        assert_eq!(state.context.work_id.as_deref(), Some("b"));
        assert_eq!(state.view().unwrap().draft, "new unsent draft");
    }

    #[tokio::test]
    async fn prepared_request_keeps_file_identity_and_routes_intake_before_sending() {
        let _locale = crate::test_support::lock_locale();
        let mut state = State::new();
        state.select(Some("other-work".into()));
        state.view_mut().draft = "original request".into();
        let command_id = uuid::Uuid::new_v4().to_string();
        let conversation_id = uuid::Uuid::new_v4().to_string();
        let document = json!({"method":"conversation.submit","commandId":command_id,"ifMatch":[],
            "params":{"conversationId":conversation_id,"clientMessageId":uuid::Uuid::new_v4().to_string(),
                "text":"revise the requested check","declaredIntent":"WorkDiscussion",
                "context":{"consoleSessionId":uuid::Uuid::new_v4().to_string(),"contextVersion":1,
                    "projectId":"project","selectedWorkId":"file-work"}}});
        let outcome = resolve_operation(
            vec![
                "plan".into(),
                "revise".into(),
                "--request-json".into(),
                document.to_string(),
            ],
            state.context.clone(),
            uuid::Uuid::new_v4().to_string(),
        )
        .await
        .unwrap();
        state.receive(Update {
            work: Some("other-work".into()),
            input: "original request".into(),
            conversation: None,
            result: Ok(outcome),
        });
        let (operation, work, input) = state.prepared.take().expect("prepared, not yet sent");
        assert_eq!(operation.command_id, command_id);
        assert_eq!(state.view().unwrap().draft, "original request");
        let (jobs, mut receiver) = mpsc::unbounded_channel();
        queue_captured(&mut state, &jobs, JobKind::Send(operation), work, input).unwrap();
        let job = receiver.try_recv().unwrap();
        assert_eq!(job.conversation.as_deref(), Some(conversation_id.as_str()));
        assert_eq!(
            state.conversation_targets[&conversation_id].as_deref(),
            Some("file-work")
        );
        assert!(
            matches!(&state.retry, Some((_,_,JobKind::Send(operation))) if operation.command_id == command_id)
        );
        assert_eq!(state.context.work_id.as_deref(), Some("other-work"));
    }

    #[test]
    fn ordinary_apply_keeps_target_draft_and_frozen_confirmation_across_updates() {
        let _locale = crate::test_support::lock_locale();
        let mut state = State::new();
        state.select(Some("work-a".into()));
        state
            .context
            .versions
            .insert(("Work".into(), "work-a".into()), 7);
        state
            .context
            .versions
            .insert(("ChangeProposal".into(), "change-a".into()), 2);
        state.view_mut().draft = "/work apply change-a".into();
        let (jobs, mut receiver) = mpsc::unbounded_channel();
        submit(&mut state, &jobs).unwrap();
        let job = receiver.try_recv().unwrap();
        let JobKind::PrepareApply(target) = job.kind else {
            panic!("expected proposal preparation")
        };
        assert_eq!(target.work_id, "work-a");
        state.select(Some("work-b".into()));
        state.view_mut().draft = "new work-b draft".into();
        let operation = Operation {
            method: "work.apply_change".into(),
            params: json!({"proposalId":"change-a","grantProposalId":"frozen-grant"}),
            if_match: vec![
                json!({"kind":"Work","id":"work-a","version":7}),
                json!({"kind":"ChangeProposal","id":"change-a","version":2}),
            ],
            mutation: true,
            confirmation: true,
            command_id: target.command_id,
        };
        let frozen = operation_preview(&operation);
        state.receive(Update {
            work: job.work,
            input: job.input,
            conversation: None,
            result: Ok(Outcome::StartPreview {
                operation,
                preview: frozen.clone(),
            }),
        });
        state.observe(
            &json!({"subjects":[{"kind":"Work","id":"work-a","version":99},
            {"kind":"ChangeProposal","id":"change-a","version":99}]}),
        );
        handle_key(&mut state, KeyCode::Enter, KeyModifiers::NONE, &jobs).unwrap();
        assert!(receiver.try_recv().is_err());
        handle_key(&mut state, KeyCode::Enter, KeyModifiers::CONTROL, &jobs).unwrap();
        let confirmed = receiver.try_recv().unwrap();
        let JobKind::Send(operation) = confirmed.kind else {
            panic!("expected frozen submission")
        };
        assert_eq!(operation_preview(&operation), frozen);
        assert_eq!(confirmed.work.as_deref(), Some("work-a"));
        assert_eq!(confirmed.input, "/work apply change-a");
        assert_eq!(state.context.work_id.as_deref(), Some("work-b"));
        assert_eq!(state.view().unwrap().draft, "new work-b draft");
        assert_eq!(
            state.views[&Some("work-a".into())].draft,
            "/work apply change-a"
        );
    }
}
