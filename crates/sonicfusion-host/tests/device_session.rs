//! Opens the real default output, so it is opt-in:
//! `cargo test -p sonicfusion-host --test device_session -- --ignored`.
//! The circuit is digital silence; nothing is audible.
#![cfg(feature = "audio-device")]

use std::time::Duration;

use sonicfusion_graph_api::{GraphSpec, RevisionStatus};
use sonicfusion_host::{Engine, Session, SessionExit, default_output_rate};
use tokio::{sync::mpsc, time::timeout};

fn silent(factor: f64) -> GraphSpec {
    serde_json::from_value(serde_json::json!({
        "version": 1,
        "nodes": [
            {"id": "osc", "kind": "sine", "params": {"frequency_hz": 220.0}},
            {"id": "gain", "kind": "gain", "params": {"factor": factor}},
            {"id": "out", "kind": "output", "params": {}}
        ],
        "wires": [
            {"from": "osc", "to": {"node": "gain", "port": "in"}},
            {"from": "gain", "to": {"node": "out", "port": "in"}}
        ]
    }))
    .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "opens the default audio output"]
async fn plays_and_edits_on_the_default_output_at_its_native_rate() {
    let rate = default_output_rate().expect("a default output device");
    let engine = Engine::with_settings(rate, 1024).unwrap();
    assert_eq!(engine.info().sample_rate_hz, rate);

    let session = Session::start_device(&engine, &silent(0.0)).unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(session.exit(), None, "playback should still be running");
    assert!(
        session.next_output_frame() > 0,
        "the device is consuming frames"
    );

    let (updates_tx, mut updates) = mpsc::unbounded_channel();
    session
        .apply(&silent(-0.0), move |update| {
            let _ = updates_tx.send(update.status);
        })
        .unwrap();
    let applied = timeout(Duration::from_secs(5), async {
        while let Some(status) = updates.recv().await {
            if let RevisionStatus::Applied { frame } = status {
                return frame;
            }
        }
        panic!("revision ended without applying");
    })
    .await
    .expect("the edit applies at a processing boundary");
    assert_eq!(applied % 1024, 0);
    assert_eq!(session.level(1024).await.unwrap().peak_dbfs, None);

    assert_eq!(session.stop().await, SessionExit::Stopped);

    // An engine at another rate is refused before anything opens.
    let other = Engine::with_settings(if rate == 48_000 { 44_100 } else { 48_000 }, 1024).unwrap();
    assert!(matches!(
        Session::start_device(&other, &silent(0.0)),
        Err(sonicfusion_host::HostError::DeviceRate { device_hz, .. }) if device_hz == rate
    ));
}
