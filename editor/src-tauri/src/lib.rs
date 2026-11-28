//! Tauri commands for the editor. Every engine interaction goes through
//! `sonicfusion-host` with complete graphs; this crate never sees engine types.

use std::sync::{
    Arc, RwLock,
    atomic::{AtomicU64, Ordering},
};

use serde::Serialize;
use sonicfusion_graph_api::{
    GraphIssue, GraphSpec, LevelReading, NodeDescriptor, NodeId, ProbeWindow, RevisionStatus,
    ScopeWindow, SessionInfo, catalog as node_catalog,
};
use sonicfusion_host::{Engine, HostError, Session, SessionExit, default_output_rate};
use tauri::{AppHandle, Builder, Emitter, Runtime, State};
use tokio::sync::Mutex;

const BATCH_FRAMES: usize = 1024;

struct AppState {
    /// Replaced when the default output's native rate changes between sessions.
    engine: RwLock<Engine>,
    /// Commands clone the session out of the slot so long engine work (level,
    /// probe) never blocks start, stop or apply.
    session: Mutex<Option<Running>>,
    next_session: AtomicU64,
}

#[derive(Clone)]
struct Running {
    id: u64,
    session: Arc<Session>,
}

/// Revision numbers restart with every session, so events carry the session.
#[derive(Clone, Debug, Serialize)]
struct RevisionEvent {
    session: u64,
    revision: u64,
    status: RevisionStatus,
}

#[derive(Clone, Debug, Serialize)]
struct Started {
    session: u64,
    /// The settings the session runs with, which follow the output device.
    info: SessionInfo,
}

#[derive(Clone, Debug, Serialize)]
struct SessionEnded {
    session: u64,
    message: String,
}

/// What a failed command rejects with on the TypeScript side.
#[derive(Debug, Serialize)]
struct CommandError {
    kind: &'static str,
    message: String,
    issues: Vec<GraphIssue>,
}

impl From<HostError> for CommandError {
    fn from(error: HostError) -> Self {
        let message = error.to_string();
        match error {
            HostError::Invalid(issues) => Self {
                kind: "invalid",
                message,
                issues,
            },
            HostError::Engine(_) => Self {
                kind: "engine",
                message,
                issues: Vec::new(),
            },
            HostError::NoSession => Self {
                kind: "no_session",
                message,
                issues: Vec::new(),
            },
            HostError::DeviceRate { .. } => Self {
                kind: "engine",
                message,
                issues: Vec::new(),
            },
        }
    }
}

type CommandResult<T> = Result<T, CommandError>;

#[tauri::command]
fn catalog() -> Vec<NodeDescriptor> {
    node_catalog()
}

#[tauri::command]
fn session_info(state: State<'_, AppState>) -> SessionInfo {
    state.engine().info()
}

#[tauri::command]
fn validate(state: State<'_, AppState>, graph: GraphSpec) -> Vec<GraphIssue> {
    state.engine().validate(&graph)
}

#[tauri::command]
fn export_dsl(state: State<'_, AppState>, graph: GraphSpec) -> CommandResult<String> {
    Ok(state.engine().lower(&graph)?.source)
}

/// Replace any running session with one playing `graph` on the default
/// device at its native rate. An invalid graph leaves a running session alone.
#[tauri::command]
async fn start_session<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, AppState>,
    graph: GraphSpec,
) -> CommandResult<Started> {
    // Refuse a broken draft before touching the device or a running session.
    state.engine().lower(&graph)?;
    let mut slot = state.session.lock().await;
    // The engine can only follow the device between sessions.
    let device_hz = default_output_rate()?;
    let engine = if device_hz == state.engine().info().sample_rate_hz {
        state.engine()
    } else {
        // Limits such as Nyquist move with the rate, so check again.
        let engine = Engine::with_settings(device_hz, BATCH_FRAMES)?;
        engine.lower(&graph)?;
        *state.engine.write().unwrap() = engine.clone();
        engine
    };
    if let Some(previous) = slot.take() {
        previous.session.stop().await;
    }
    let session = Arc::new(Session::start_device(&engine, &graph)?);
    let id = state.next_session.fetch_add(1, Ordering::Relaxed) + 1;
    let exited = session.exited();
    *slot = Some(Running { id, session });
    drop(slot);
    tauri::async_runtime::spawn(async move {
        // Stops requested through `stop_session` are not news to the UI.
        if let SessionExit::Failed(message) = exited.await {
            let _ = app.emit(
                "session-ended",
                SessionEnded {
                    session: id,
                    message,
                },
            );
        }
    });
    Ok(Started {
        session: id,
        info: engine.info(),
    })
}

#[tauri::command]
async fn stop_session(state: State<'_, AppState>) -> CommandResult<()> {
    let running = state.session.lock().await.take();
    if let Some(running) = running {
        running.session.stop().await;
    }
    Ok(())
}

/// Submit a complete replacement; progress arrives as `revision-status` events.
#[tauri::command]
async fn apply_graph<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, AppState>,
    graph: GraphSpec,
) -> CommandResult<u64> {
    let running = current(&state).await.ok_or(HostError::NoSession)?;
    let session = running.id;
    Ok(running.session.apply(&graph, move |update| {
        let _ = app.emit(
            "revision-status",
            RevisionEvent {
                session,
                revision: update.revision,
                status: update.status,
            },
        );
    })?)
}

/// Recompute `node`'s output in the draft at the session's next output frame
/// (frame 0 without a session).
#[tauri::command]
async fn probe(
    state: State<'_, AppState>,
    graph: GraphSpec,
    node: String,
    frames: usize,
) -> CommandResult<ProbeWindow> {
    let start_frame = current(&state)
        .await
        .map_or(0, |running| running.session.next_output_frame());
    let node = NodeId::new(node);
    let engine = state.engine();
    let samples = engine.probe(&graph, &node, start_frame, frames).await?;
    Ok(ProbeWindow {
        node,
        start_frame,
        sample_rate_hz: engine.info().sample_rate_hz,
        samples,
    })
}

/// Recompute a slow scope at the same timeline origin as the short probe.
#[tauri::command]
async fn scope(
    state: State<'_, AppState>,
    graph: GraphSpec,
    node: String,
    seconds: f64,
) -> CommandResult<ScopeWindow> {
    let start_frame = current(&state)
        .await
        .map_or(0, |running| running.session.next_output_frame());
    Ok(state
        .engine()
        .scope(&graph, &NodeId::new(node), start_frame, seconds)
        .await?)
}

/// Peak of what is playing, or nothing without a running session.
#[tauri::command]
async fn level(state: State<'_, AppState>, frames: usize) -> CommandResult<Option<LevelReading>> {
    let Some(running) = current(&state).await else {
        return Ok(None);
    };
    match running.session.level(frames).await {
        Ok(reading) => Ok(Some(reading)),
        Err(HostError::NoSession) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

async fn current(state: &AppState) -> Option<Running> {
    state.session.lock().await.clone()
}

impl AppState {
    fn engine(&self) -> Engine {
        self.engine.read().unwrap().clone()
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Validate against the default output's rate from the start; without a
    // usable device the editor still works, and Play reports why.
    let rate = default_output_rate().unwrap_or(48_000);
    let engine = Engine::with_settings(rate, BATCH_FRAMES)
        .expect("cannot start the SonicFusion engine host");
    with_engine(tauri::Builder::default(), engine)
        .run(tauri::generate_context!())
        .expect("error while running the SonicFusion editor");
}

fn with_engine<R: Runtime>(builder: Builder<R>, engine: Engine) -> Builder<R> {
    builder
        .manage(AppState {
            engine: RwLock::new(engine),
            session: Mutex::new(None),
            next_session: AtomicU64::new(0),
        })
        .invoke_handler(tauri::generate_handler![
            catalog,
            session_info,
            validate,
            export_dsl,
            start_session,
            stop_session,
            apply_graph,
            probe,
            level,
            scope,
        ])
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};
    use tauri::{
        ipc::{CallbackFn, InvokeBody},
        test::{
            INVOKE_KEY, MockRuntime, get_ipc_response, mock_builder, mock_context, noop_assets,
        },
        webview::{InvokeRequest, WebviewWindow},
    };

    use super::*;

    fn webview() -> (tauri::App<MockRuntime>, WebviewWindow<MockRuntime>) {
        let app = with_engine(mock_builder(), Engine::new().unwrap())
            .build(mock_context(noop_assets()))
            .unwrap();
        let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();
        (app, webview)
    }

    fn call(webview: &WebviewWindow<MockRuntime>, cmd: &str, body: Value) -> Result<Value, Value> {
        get_ipc_response(
            webview,
            InvokeRequest {
                cmd: cmd.into(),
                callback: CallbackFn(0),
                error: CallbackFn(1),
                url: "tauri://localhost".parse().unwrap(),
                body: InvokeBody::Json(body),
                headers: Default::default(),
                invoke_key: INVOKE_KEY.into(),
            },
        )
        .map(|response| response.deserialize::<Value>().unwrap())
    }

    fn fixture() -> Value {
        serde_json::from_str(include_str!(
            "../../../crates/sonicfusion-graph-api/fixtures/studio-patch.json"
        ))
        .unwrap()
    }

    #[test]
    fn commands_speak_the_graph_api_json() {
        let (_app, webview) = webview();
        let catalog = call(&webview, "catalog", json!({})).unwrap();
        let expected_catalog: Value = serde_json::from_str(include_str!(
            "../../../crates/sonicfusion-graph-api/fixtures/catalog.json"
        ))
        .unwrap();
        assert_eq!(catalog, expected_catalog);
        assert_eq!(
            call(&webview, "session_info", json!({})).unwrap()["sample_rate_hz"],
            48_000
        );
        assert_eq!(
            call(&webview, "validate", json!({"graph": fixture()})).unwrap(),
            json!([])
        );

        let dsl = call(&webview, "export_dsl", json!({"graph": fixture()})).unwrap();
        assert!(dsl.as_str().unwrap().ends_with("$n5 -> out\n"));

        let probe = call(
            &webview,
            "probe",
            json!({"graph": fixture(), "node": "osc_a", "frames": 64}),
        )
        .unwrap();
        assert_eq!(probe["start_frame"], 0);
        assert_eq!(probe["samples"].as_array().unwrap().len(), 64);
        let scope = call(
            &webview,
            "scope",
            json!({"graph": fixture(), "node": "osc_a", "seconds": 4}),
        )
        .unwrap();
        assert_eq!(scope["frames"], 192_000);
        assert_eq!(scope["bands"].as_array().unwrap().len(), 1_000);
        assert_eq!(scope["gate"], Value::Null);
        assert_eq!(
            call(&webview, "level", json!({"frames": 64})).unwrap(),
            Value::Null
        );
    }

    #[test]
    fn failures_carry_kind_and_issues() {
        let (_app, webview) = webview();
        let mut broken = fixture();
        broken["nodes"][0]["params"]["frequency_hz"] = json!(30_000.0);
        let error = call(&webview, "export_dsl", json!({"graph": broken.clone()})).unwrap_err();
        assert_eq!(error["kind"], "invalid");
        assert_eq!(error["issues"][0]["node"], "osc_a");
        assert_eq!(error["issues"][0]["param"], "frequency_hz");

        // Refused before any device is opened or a running session is stopped.
        let error = call(&webview, "start_session", json!({"graph": broken})).unwrap_err();
        assert_eq!(error["kind"], "invalid");
        assert_eq!(error["message"], "the graph has 1 error");

        let error = call(&webview, "apply_graph", json!({"graph": fixture()})).unwrap_err();
        assert_eq!(error["kind"], "no_session");
    }
}
