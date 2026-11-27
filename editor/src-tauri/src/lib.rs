//! Tauri commands for the editor. Every engine interaction goes through
//! `sonicfusion-host` with complete graphs; this crate never sees engine types.

use serde::Serialize;
use sonicfusion_graph_api::{
    GraphIssue, GraphSpec, LevelReading, NodeDescriptor, NodeId, ProbeWindow, SessionInfo,
    catalog as node_catalog,
};
use sonicfusion_host::{Engine, HostError, Session, SessionExit};
use tauri::{AppHandle, Builder, Emitter, Runtime, State};
use tokio::sync::Mutex;

struct AppState {
    engine: Engine,
    session: Mutex<Option<Session>>,
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
    state.engine.info()
}

#[tauri::command]
fn validate(state: State<'_, AppState>, graph: GraphSpec) -> Vec<GraphIssue> {
    state.engine.validate(&graph)
}

#[tauri::command]
fn export_dsl(state: State<'_, AppState>, graph: GraphSpec) -> CommandResult<String> {
    Ok(state.engine.lower(&graph)?.source)
}

/// Replace any running session with one playing `graph` on the default device.
#[tauri::command]
async fn start_session<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, AppState>,
    graph: GraphSpec,
) -> CommandResult<()> {
    let mut slot = state.session.lock().await;
    if let Some(previous) = slot.take() {
        previous.stop().await;
    }
    let session = Session::start_device(&state.engine, &graph)?;
    let exited = session.exited();
    *slot = Some(session);
    drop(slot);
    tauri::async_runtime::spawn(async move {
        // Stops requested through `stop_session` are not news to the UI.
        if let SessionExit::Failed(message) = exited.await {
            let _ = app.emit("session-ended", Some(message));
        }
    });
    Ok(())
}

#[tauri::command]
async fn stop_session(state: State<'_, AppState>) -> CommandResult<()> {
    if let Some(session) = state.session.lock().await.take() {
        session.stop().await;
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
    let slot = state.session.lock().await;
    let session = slot.as_ref().ok_or(HostError::NoSession)?;
    Ok(session.apply(&graph, move |update| {
        let _ = app.emit("revision-status", update);
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
    let start_frame = match state.session.lock().await.as_ref() {
        Some(session) => session.next_output_frame(),
        None => 0,
    };
    let node = NodeId::new(node);
    let samples = state
        .engine
        .probe(&graph, &node, start_frame, frames)
        .await?;
    Ok(ProbeWindow {
        node,
        start_frame,
        sample_rate_hz: state.engine.info().sample_rate_hz,
        samples,
    })
}

#[tauri::command]
async fn level(state: State<'_, AppState>, frames: usize) -> CommandResult<Option<LevelReading>> {
    let slot = state.session.lock().await;
    match slot.as_ref() {
        Some(session) => Ok(Some(session.level(frames).await?)),
        None => Ok(None),
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let engine = Engine::new().expect("cannot start the SonicFusion engine host");
    with_engine(tauri::Builder::default(), engine)
        .run(tauri::generate_context!())
        .expect("error while running the SonicFusion editor");
}

fn with_engine<R: Runtime>(builder: Builder<R>, engine: Engine) -> Builder<R> {
    builder
        .manage(AppState {
            engine,
            session: Mutex::new(None),
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
        assert_eq!(catalog.as_array().unwrap().len(), 7);
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
        let error = call(&webview, "export_dsl", json!({"graph": broken})).unwrap_err();
        assert_eq!(error["kind"], "invalid");
        assert_eq!(error["issues"][0]["node"], "osc_a");
        assert_eq!(error["issues"][0]["param"], "frequency_hz");

        let error = call(&webview, "apply_graph", json!({"graph": fixture()})).unwrap_err();
        assert_eq!(error["kind"], "no_session");
    }
}
