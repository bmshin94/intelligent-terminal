use super::*;
use crate::agent_sessions::{AgentStatus, SessionEvent, SessionOrigin};
use crate::app::tests::test_app_with_master_rx;
use crate::session_registry::{InMemoryRegistry, SessionInfo, SessionRegistry};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

#[tokio::test]
async fn ssh_cached_followups_do_not_dirty_the_other_platform_in_a_loop() {
    let _locale = crate::test_support::lock_locale();
    tokio::task::LocalSet::new()
        .run_until(async {
            let registry = SharedRegistry::new();
            let source = source("devbox");
            let mut helper = Helper::new("posix", &source, &registry);
            for (id, platform, sequence) in [
                ("posix", SshPlatform::Posix, 1),
                ("windows", SshPlatform::Windows, 2),
            ] {
                let tab = helper.app.tab_mut(id);
                tab.current_view = View::Agents;
                tab.agents_view.ssh_source = Some(source.clone());
                tab.agents_view.ssh_profile =
                    super::ssh_profile::SessionsProfile::Ssh(source.target.clone(), platform);
                tab.agents_view.snapshot = Some(Vec::new());
                helper
                    .app
                    .ssh_resumes
                    .pending_cached
                    .insert((source.clone(), platform), sequence);
                helper
                    .app
                    .ssh_resumes
                    .dirty_cached
                    .insert((source.clone(), platform));
            }
            helper.app.ssh_resumes.sequence = 2;
            let snapshot = Snapshot {
                source: source.clone(),
                platform: SshPlatform::Posix,
                epoch: uuid::Uuid::new_v4(),
                revision: 1,
                sessions: Vec::new(),
            };
            helper.app.handle_ssh_registry_result(
                source.clone(),
                SshPlatform::Posix,
                1,
                SshRegistryAction::Cached,
                Ok(snapshot),
            );
            helper.app.handle_ssh_registry_result(
                source.clone(),
                SshPlatform::Windows,
                2,
                SshRegistryAction::Cached,
                Err("platform conflict".into()),
            );
            assert!(helper.app.ssh_resumes.dirty_cached.is_empty());
            assert_eq!(helper.app.ssh_resumes.pending_cached.len(), 2);
            assert!(helper.master.try_recv().is_err());
        })
        .await;
}

#[test]
fn ssh_helper_never_applies_wrong_platform_cache_and_errors_are_platform_scoped() {
    let _locale = crate::test_support::lock_locale();
    let (mut app, _) = test_app_with_master_rx();
    let source = source("same-target");
    for (tab_id, platform) in [
        ("posix", SshPlatform::Posix),
        ("windows", SshPlatform::Windows),
    ] {
        let tab = app.tab_mut(tab_id);
        tab.current_view = View::Agents;
        tab.agents_view.ssh_source = Some(source.clone());
        tab.agents_view.ssh_profile =
            super::ssh_profile::SessionsProfile::Ssh(source.target.clone(), platform);
        tab.agents_view.snapshot = Some(Vec::new());
    }
    let cached = Snapshot {
        source: source.clone(),
        platform: SshPlatform::Posix,
        epoch: uuid::Uuid::new_v4(),
        revision: 7,
        sessions: vec![history(&source)],
    };
    app.ssh_resumes.accept(cached.clone(), 1).unwrap();
    app.refresh_ssh_resume_snapshots();
    assert_eq!(
        app.tab_mut("posix")
            .agents_view
            .snapshot
            .as_ref()
            .unwrap()
            .len(),
        1
    );
    assert!(app
        .tab_mut("windows")
        .agents_view
        .snapshot
        .as_ref()
        .unwrap()
        .is_empty());
    app.set_shared_ssh_error(
        &source,
        SshPlatform::Windows,
        Some("platform conflict".into()),
        SshErrorOperation::Activate,
    );
    assert!(app.tab_mut("posix").agents_view.ssh_error.is_none());
    assert_eq!(
        app.tab_mut("windows").agents_view.ssh_error.as_deref(),
        Some("platform conflict")
    );
    app.set_shared_ssh_error(
        &source,
        SshPlatform::Windows,
        None,
        SshErrorOperation::Cached,
    );
    assert!(app.tab_mut("windows").agents_view.ssh_error.is_some());

    app.ssh_resumes
        .pending_cached
        .insert((source.clone(), SshPlatform::Windows), 2);
    app.handle_ssh_registry_result(
        source.clone(),
        SshPlatform::Windows,
        2,
        SshRegistryAction::Cached,
        Ok(cached.clone()),
    );
    assert_eq!(
        app.ssh_resumes.snapshots[&source].snapshot.platform,
        SshPlatform::Posix
    );
    assert!(app
        .tab_mut("windows")
        .agents_view
        .snapshot
        .as_ref()
        .unwrap()
        .is_empty());
    assert!(app.tab_mut("posix").agents_view.ssh_error.is_none());

    let mut changed_platform = cached;
    changed_platform.platform = SshPlatform::Windows;
    assert!(app.ssh_resumes.accept(changed_platform, 3).is_err());
}

#[test]
fn ssh_helper_ignores_late_history_after_platform_change_and_rejects_legacy_response() {
    let _locale = crate::test_support::lock_locale();
    let (mut app, _) = test_app_with_master_rx();
    let source = source("devbox");
    let tab_id = "windows";
    let tab = app.tab_mut(tab_id);
    tab.current_view = View::Agents;
    tab.agents_view.ssh_source = Some(source.clone());
    tab.agents_view.ssh_profile =
        super::ssh_profile::SessionsProfile::Ssh(source.target.clone(), SshPlatform::Windows);
    tab.agents_view.snapshot = Some(Vec::new());
    tab.agents_view.latest_request_id = Some(8);
    tab.agents_view.refetch_in_flight = true;
    let legacy: Snapshot = serde_json::from_value(serde_json::json!({
        "source": source, "epoch": uuid::Uuid::new_v4().to_string(),
        "revision": 1, "sessions": [history(&source)]
    }))
    .unwrap();
    let action = SshRegistryAction::History {
        tab_id: tab_id.into(),
        request_id: 8,
    };
    app.handle_ssh_registry_result(
        source.clone(),
        SshPlatform::Posix,
        1,
        action.clone(),
        Ok(legacy.clone()),
    );
    assert!(app.tab_mut(tab_id).agents_view.refetch_in_flight);
    assert!(app.tab_mut(tab_id).agents_view.ssh_error.is_none());
    app.handle_ssh_registry_result(source.clone(), SshPlatform::Windows, 2, action, Ok(legacy));
    assert!(!app.tab_mut(tab_id).agents_view.refetch_in_flight);
    assert!(app
        .tab_mut(tab_id)
        .agents_view
        .ssh_error
        .as_deref()
        .unwrap()
        .contains("platform"));
    assert!(app
        .tab_mut(tab_id)
        .agents_view
        .snapshot
        .as_ref()
        .unwrap()
        .is_empty());
    assert!(!app.ssh_resumes.snapshots.contains_key(&source));
}

// Separate helper Apps use one shared registry endpoint. Native lifecycle
// execution and wire dispatch are covered by the master's own tests.
struct SharedRegistry {
    registries: Mutex<HashMap<Source, Arc<InMemoryRegistry>>>,
    requests: Mutex<Vec<Request>>,
    failure: Mutex<Option<String>>,
    epoch: uuid::Uuid,
    revision: AtomicU64,
    creates: AtomicU64,
    focuses: AtomicU64,
}

impl SharedRegistry {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            registries: Mutex::new(HashMap::new()),
            requests: Mutex::new(Vec::new()),
            failure: Mutex::new(None),
            epoch: uuid::Uuid::new_v4(),
            revision: AtomicU64::new(1),
            creates: AtomicU64::new(0),
            focuses: AtomicU64::new(0),
        })
    }

    fn registry(&self, source: &Source) -> Arc<InMemoryRegistry> {
        self.registries
            .lock()
            .unwrap()
            .entry(source.clone())
            .or_insert_with(|| Arc::new(InMemoryRegistry::new()))
            .clone()
    }

    async fn close(&self, source: &Source, pane: &str) {
        self.registry(source)
            .apply_event(SessionEvent::PaneClosed {
                pane_session_id: pane.into(),
            })
            .await;
        self.revision.fetch_add(1, Ordering::SeqCst);
    }
}

#[async_trait::async_trait(?Send)]
impl SshRegistryClient for SharedRegistry {
    async fn request(&self, request: Request) -> Result<Snapshot> {
        let request: Request = serde_json::from_value(serde_json::to_value(request)?)?;
        self.requests.lock().unwrap().push(request.clone());
        if let Some(error) = self.failure.lock().unwrap().take() {
            anyhow::bail!("{error}");
        }
        let source = match &request {
            Request::List { source, .. } | Request::Activate { source, .. } => source.clone(),
        };
        let registry = self.registry(&source);
        match request {
            Request::List {
                refresh_history, ..
            } => {
                if refresh_history || registry.snapshot().await.is_empty() {
                    registry.upsert_if_absent(history(&source)).await;
                }
            }
            Request::Activate { session_id, .. } => {
                if registry
                    .apply_event(SessionEvent::ResumeDispatched {
                        key: session_id.clone(),
                    })
                    .await
                {
                    self.creates.fetch_add(1, Ordering::SeqCst);
                    registry
                        .apply_event(SessionEvent::ResumePaneAssigned {
                            key: session_id,
                            pane_session_id: uuid::Uuid::new_v4().to_string(),
                        })
                        .await;
                    self.revision.fetch_add(1, Ordering::SeqCst);
                } else {
                    self.focuses.fetch_add(1, Ordering::SeqCst);
                }
            }
        }
        Ok(Snapshot {
            source,
            platform: SshPlatform::Posix,
            epoch: self.epoch,
            revision: self.revision.load(Ordering::SeqCst),
            sessions: registry.snapshot().await,
        })
    }
}

fn source(name: &str) -> Source {
    Source {
        target: crate::ssh_sessions::SshTarget::new(name, None).unwrap(),
        agent_id: "copilot".into(),
    }
}

fn history(source: &Source) -> SessionInfo {
    let mut info = SessionInfo::new("same-id".into(), "/home/me/project".into());
    info.title = Some("Remote conversation".into());
    info.status = Some(AgentStatus::Historical);
    info.origin = Some(SessionOrigin::Unknown);
    info.cli_source = CliSource::from_agent_id(&source.agent_id);
    info.location = SessionLocation::Ssh {
        target: source.target.clone(),
    };
    info
}

struct Helper {
    app: App,
    events: mpsc::UnboundedReceiver<AppEvent>,
    master: mpsc::UnboundedReceiver<crate::protocol::acp::client::MasterExtRequest>,
    tab: String,
}

impl Helper {
    fn new(tab: &str, source: &Source, registry: &Arc<SharedRegistry>) -> Self {
        let (mut app, master) = test_app_with_master_rx();
        app.owner_tab_id = Some(tab.into());
        app.tab_id = Some(tab.into());
        app.current_agent_id = source.agent_id.clone();
        app.state = ConnectionState::Connected;
        app.show_welcome_hint = false;
        let (sender, events) = mpsc::unbounded_channel();
        app.event_tx = Some(sender);
        app.ssh_resumes.client = Some(registry.clone());
        app.set_initial_sessions_ssh_profile(
            Some(source.target.destination()),
            source.target.port(),
            None,
            None,
        );
        Self {
            app,
            events,
            master,
            tab: tab.to_string(),
        }
    }

    async fn next(&mut self) {
        let event = tokio::time::timeout(Duration::from_secs(2), self.events.recv())
            .await
            .expect("shared registry response timed out")
            .expect("event channel closed");
        self.app.handle_event(event);
    }

    async fn open(&mut self) {
        self.app.open_agents_view_for_tab(self.tab.clone());
        self.next().await;
    }

    fn enter(&mut self) {
        self.app
            .handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    }

    fn row(&self) -> AgentSession {
        self.app
            .agents_rows_for_tab(&self.tab)
            .into_iter()
            .next()
            .unwrap()
    }
}

#[test]
fn ssh_resume_pending_is_platform_scoped() {
    let _locale = crate::test_support::lock_locale();
    let (mut app, _) = test_app_with_master_rx();
    let source = source("same-target");
    let session = session_info_to_agent_session(&history(&source));
    {
        let tab = app.current_tab_mut();
        tab.agents_view.ssh_source = Some(source.clone());
        tab.agents_view.ssh_profile =
            super::ssh_profile::SessionsProfile::Ssh(source.target.clone(), SshPlatform::Posix);
    }
    app.ssh_resumes
        .pending_actions
        .insert((source.clone(), SshPlatform::Posix, session.key.clone()), 1);
    assert!(app.ssh_resume_pending(&session));

    app.current_tab_mut().agents_view.ssh_profile =
        super::ssh_profile::SessionsProfile::Ssh(source.target.clone(), SshPlatform::Windows);
    assert!(!app.ssh_resume_pending(&session));
    app.ssh_resumes
        .pending_actions
        .insert((source, SshPlatform::Windows, session.key.clone()), 2);
    assert!(app.ssh_resume_pending(&session));
}

#[tokio::test]
async fn new_ssh_helper_reads_idle_from_shared_registry_and_focuses_the_same_pane() {
    let _locale = crate::test_support::lock_locale();
    rust_i18n::set_locale("en-US");
    tokio::task::LocalSet::new()
        .run_until(async {
            let registry = SharedRegistry::new();
            let source = source("wsl-ubuntu");
            let mut first = Helper::new("tab-a", &source, &registry);
            first.open().await;
            assert_eq!(first.row().status, AgentStatus::Historical);
            first.enter();
            first.enter();
            first.next().await;
            let pane = first.row().pane_session_id.unwrap();

            let mut second = Helper::new("tab-b", &source, &registry);
            second.open().await;
            assert_eq!(second.row().status, AgentStatus::Idle);
            assert_eq!(second.row().pane_session_id.as_deref(), Some(pane.as_str()));
            assert_eq!(second.row().origin, SessionOrigin::Unknown);
            assert!(first.app.agent_sessions.iter_sorted().is_empty());
            assert!(second.app.agent_sessions.iter_sorted().is_empty());
            assert!(first.master.try_recv().is_err());
            assert!(second.master.try_recv().is_err());

            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(110, 12)).unwrap();
            terminal
                .draw(|frame| crate::ui::render(frame, &mut second.app))
                .unwrap();
            let text: String = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            assert!(text.contains("Idle") && text.contains("Remote conversation"));
            second.enter();
            second.next().await;
            assert_eq!(registry.creates.load(Ordering::SeqCst), 1);
            assert_eq!(registry.focuses.load(Ordering::SeqCst), 1);
        })
        .await;
}

#[tokio::test]
async fn shared_binding_outlives_original_helper_and_close_updates_another_helper() {
    let _locale = crate::test_support::lock_locale();
    tokio::task::LocalSet::new()
        .run_until(async {
            let registry = SharedRegistry::new();
            let source = source("wsl-ubuntu");
            let mut first = Helper::new("tab-a", &source, &registry);
            first.open().await;
            first.enter();
            first.next().await;
            let pane = first.row().pane_session_id.unwrap();
            drop(first);
            let mut second = Helper::new("tab-b", &source, &registry);
            second.open().await;
            assert_eq!(second.row().status, AgentStatus::Idle);

            registry.close(&source, &pane).await;
            second
                .app
                .handle_event(AppEvent::SshSessionsChanged(source.clone()));
            second.next().await;
            assert_eq!(second.row().status, AgentStatus::Ended);
            assert!(second.row().pane_session_id.is_none());
            assert!(matches!(
                registry.requests.lock().unwrap().last().unwrap(),
                Request::List {
                    refresh_history: false,
                    ..
                }
            ));
            second.enter();
            second.next().await;
            assert_eq!(second.row().status, AgentStatus::Idle);
            assert_ne!(second.row().pane_session_id.as_deref(), Some(pane.as_str()));
        })
        .await;
}

#[tokio::test]
async fn shared_history_refresh_and_reopen_keep_idle_without_local_binding_authority() {
    let _locale = crate::test_support::lock_locale();
    tokio::task::LocalSet::new()
        .run_until(async {
            let registry = SharedRegistry::new();
            let source = source("wsl-ubuntu");
            let mut helper = Helper::new("tab-a", &source, &registry);
            helper.open().await;
            helper.enter();
            helper.next().await;
            let pane = helper.row().pane_session_id;
            helper
                .app
                .handle_key(KeyEvent::new(KeyCode::F(5), KeyModifiers::NONE));
            helper.next().await;
            assert_eq!(helper.row().status, AgentStatus::Idle);
            helper.app.close_agents_view_for_tab(&helper.tab);
            helper.open().await;
            assert_eq!(helper.row().pane_session_id, pane);
            assert_eq!(helper.row().status, AgentStatus::Idle);
        })
        .await;
}

#[tokio::test]
async fn independent_helpers_do_not_share_bindings_between_ssh_sources() {
    let _locale = crate::test_support::lock_locale();
    tokio::task::LocalSet::new()
        .run_until(async {
            let registry = SharedRegistry::new();
            let own = source("yuazha@ubuntu");
            let mut first = Helper::new("first", &own, &registry);
            first.open().await;
            first.enter();
            first.next().await;
            for other in [
                source("yuazha@debian"),
                source("other@ubuntu"),
                Source {
                    target: crate::ssh_sessions::SshTarget::new("yuazha@ubuntu", Some(2222))
                        .unwrap(),
                    agent_id: "copilot".into(),
                },
                Source {
                    target: own.target.clone(),
                    agent_id: "claude".into(),
                },
            ] {
                let mut second = Helper::new("second", &other, &registry);
                second.open().await;
                assert_eq!(second.row().key, first.row().key);
                assert_eq!(second.row().status, AgentStatus::Historical);
                assert!(second.row().pane_session_id.is_none());
            }
        })
        .await;
}

#[tokio::test]
async fn failed_shared_activation_is_retryable_and_its_error_survives_cached_polling() {
    let _locale = crate::test_support::lock_locale();
    tokio::task::LocalSet::new()
        .run_until(async {
            let registry = SharedRegistry::new();
            let source = source("ubuntu");
            let mut helper = Helper::new("first", &source, &registry);
            helper.open().await;
            *registry.failure.lock().unwrap() = Some("native creation failed".into());
            helper.enter();
            helper.next().await;
            helper.next().await;
            assert_eq!(helper.row().status, AgentStatus::Historical);
            assert!(!helper.app.ssh_resume_pending(&helper.row()));
            assert!(helper
                .app
                .current_tab()
                .agents_view
                .ssh_error
                .as_deref()
                .unwrap()
                .contains("native creation failed"));
            helper.enter();
            helper.next().await;
            assert_eq!(helper.row().status, AgentStatus::Idle);
            assert!(helper.app.current_tab().agents_view.ssh_error.is_none());
        })
        .await;
}

#[tokio::test]
async fn successful_cached_poll_clears_only_its_own_transient_error() {
    let _locale = crate::test_support::lock_locale();
    tokio::task::LocalSet::new()
        .run_until(async {
            let registry = SharedRegistry::new();
            let source = source("ubuntu");
            let mut helper = Helper::new("first", &source, &registry);
            helper.open().await;
            *registry.failure.lock().unwrap() = Some("registry temporarily unavailable".into());
            helper
                .app
                .handle_event(AppEvent::SshSessionsChanged(source.clone()));
            helper.next().await;
            assert!(helper.app.current_tab().agents_view.ssh_error.is_some());
            helper
                .app
                .handle_event(AppEvent::SshSessionsChanged(source));
            helper.next().await;
            assert!(helper.app.current_tab().agents_view.ssh_error.is_none());
        })
        .await;
}

#[tokio::test]
async fn successful_cached_poll_does_not_claim_a_failed_ssh_refresh_recovered() {
    let _locale = crate::test_support::lock_locale();
    tokio::task::LocalSet::new()
        .run_until(async {
            let registry = SharedRegistry::new();
            let source = source("ubuntu");
            let mut helper = Helper::new("first", &source, &registry);
            helper.open().await;
            *registry.failure.lock().unwrap() = Some("SSH connection refused".into());
            helper
                .app
                .handle_key(KeyEvent::new(KeyCode::F(5), KeyModifiers::NONE));
            helper.next().await;
            helper
                .app
                .handle_event(AppEvent::SshSessionsChanged(source));
            helper.next().await;
            assert!(helper
                .app
                .current_tab()
                .agents_view
                .ssh_error
                .as_deref()
                .unwrap()
                .contains("SSH connection refused"));
            helper
                .app
                .handle_key(KeyEvent::new(KeyCode::F(5), KeyModifiers::NONE));
            helper.next().await;
            assert!(helper.app.current_tab().agents_view.ssh_error.is_none());
        })
        .await;
}

#[test]
fn snapshot_revisions_and_master_epochs_reject_stale_idle_history() {
    let source = source("ubuntu");
    let epoch = uuid::Uuid::new_v4();
    let make = |revision, status| {
        let mut row = history(&source);
        row.status = Some(status);
        Snapshot {
            source: source.clone(),
            platform: SshPlatform::Posix,
            epoch,
            revision,
            sessions: vec![row],
        }
    };
    let mut cache = SshResumes::default();
    cache.accept(make(2, AgentStatus::Idle), 2).unwrap();
    cache.accept(make(1, AgentStatus::Historical), 3).unwrap();
    assert_eq!(
        cache.snapshots[&source].snapshot.sessions[0].status,
        Some(AgentStatus::Idle)
    );
    let mut replacement = make(0, AgentStatus::Historical);
    replacement.epoch = uuid::Uuid::new_v4();
    cache.accept(replacement.clone(), 4).unwrap();
    cache.accept(make(9, AgentStatus::Idle), 5).unwrap();
    assert_eq!(cache.snapshots[&source].snapshot.epoch, replacement.epoch);
    assert_eq!(
        cache.snapshots[&source].snapshot.sessions[0].status,
        Some(AgentStatus::Historical)
    );
}

#[test]
fn shared_snapshot_validation_rejects_cross_source_or_cross_agent_rows() {
    let source = source("ubuntu");
    let mut cache = SshResumes::default();
    let mut row = history(&source);
    row.location = SessionLocation::Host;
    let snapshot = Snapshot {
        source: source.clone(),
        platform: SshPlatform::Posix,
        epoch: uuid::Uuid::new_v4(),
        revision: 1,
        sessions: vec![row],
    };
    assert!(cache.accept(snapshot, 1).is_err());
    let mut row = history(&source);
    row.cli_source = Some(CliSource::Claude);
    assert!(cache
        .accept(
            Snapshot {
                source,
                platform: SshPlatform::Posix,
                epoch: uuid::Uuid::new_v4(),
                revision: 1,
                sessions: vec![row]
            },
            2
        )
        .is_err());
}

#[tokio::test]
async fn cached_polling_updates_open_helpers_without_remote_history_refresh() {
    let _locale = crate::test_support::lock_locale();
    tokio::task::LocalSet::new()
        .run_until(async {
            let registry = SharedRegistry::new();
            let source = source("ubuntu");
            let mut first = Helper::new("first", &source, &registry);
            let mut second = Helper::new("second", &source, &registry);
            first.open().await;
            second.open().await;
            first.enter();
            first.next().await;
            second.app.ssh_resumes.last_poll = None;
            second.app.handle_event(AppEvent::Tick);
            second.next().await;
            assert_eq!(second.row().status, AgentStatus::Idle);
            assert!(matches!(
                registry.requests.lock().unwrap().last().unwrap(),
                Request::List {
                    refresh_history: false,
                    ..
                }
            ));
        })
        .await;
}
