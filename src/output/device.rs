//! Optional native playback for a live mono execution. Processing publishes
//! complete batches; only the host callback determines playback timing.

mod bridge;

use std::{future::pending, sync::Arc, time::Duration};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use tokio::sync::oneshot;

use crate::{
    Result,
    runtime::{RunExit, SynthRuntime},
};

use self::bridge::{BatchOutput, Channels, Counters};

#[derive(Debug, thiserror::Error)]
pub enum DeviceError {
    #[error("no default audio output device")]
    NoDefaultDevice,
    #[error("default device has no native f32 mono or two-channel output at {sample_rate_hz} Hz")]
    UnsupportedFormat { sample_rate_hz: u32 },
    #[error("audio device callback disconnected")]
    CallbackDisconnected,
    #[error("live audio graph ended unexpectedly")]
    EndOfStream,
    #[error("failed to {action}: {source}")]
    Backend {
        action: &'static str,
        #[source]
        source: cpal::Error,
    },
}

/// Counts for a stopped playback run, in frames (not interleaved samples).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DevicePlaybackStats {
    /// Frames ready when playback started, or zero if stopped during prefill.
    pub prefilled_frames: u64,
    /// Graph frames taken from the queue. Excludes synthetic silence.
    pub graph_frames_consumed: u64,
    /// Device frames filled with silence because the queue was short.
    pub silent_frames_inserted: u64,
    /// Total frames filled by the callback, not necessarily heard by the listener.
    pub device_frames_submitted: u64,
}

/// Play a continuous runtime on the default audio output on a Tokio executor
/// with timers enabled. Call [`crate::runtime::RuntimeControl::stop`] to stop
/// and obtain stats. Stop discards queued audio; it does not drain the device.
///
/// The runtime must supply full processing batches of its configured size.
/// Playback queues up to three batches and starts once at least two are ready.
/// Only native `f32` mono or two-channel output at the runtime rate is supported.
/// Two-channel output is dual mono. The host chooses the callback buffer size.
/// Graph and device failures terminate playback and return an error.
pub async fn play_default_device(runtime: SynthRuntime) -> Result<DevicePlaybackStats> {
    let config = runtime.config();
    let control = runtime.control();
    let device = cpal::default_host()
        .default_output_device()
        .ok_or(DeviceError::NoDefaultDevice)?;
    let supported = device
        .supported_output_configs()
        .map_err(|source| DeviceError::Backend {
            action: "query default output formats",
            source,
        })?;
    let (channels, device_config) = select_config(supported, config.sample_rate_hz())?;
    let counters = Arc::new(Counters::default());
    let (mut output, mut callback, prefilled) = bridge::bridge(
        config.batch_frame_capacity(),
        channels,
        Arc::clone(&counters),
    )?;
    let (errors, failed) = oneshot::channel();
    let mut errors = Some(errors);
    let stream = device
        .build_output_stream(
            device_config,
            move |buffer: &mut [f32], _| callback.fill(buffer),
            move |error| {
                // CPAL's separate error callback, not the audio data callback.
                if let Some(sender) = errors.take() {
                    let _ = sender.send(error);
                }
            },
            Some(Duration::from_secs(5)),
        )
        .map_err(|source| DeviceError::Backend {
            action: "open default audio output",
            source,
        })?;

    let mut prefilled_frames = 0;
    let result = drive(runtime, &mut output, prefilled, failed, || {
        let queued_frames = control.next_output_frame() - config.start_frame();
        stream.play().map_err(|source| DeviceError::Backend {
            action: "start default audio output",
            source,
        })?;
        prefilled_frames = queued_frames;
        Ok(())
    })
    .await;
    drop(stream);
    result?;
    Ok(counters.snapshot(prefilled_frames))
}

fn select_config(
    supported: impl IntoIterator<Item = cpal::SupportedStreamConfigRange>,
    sample_rate_hz: u32,
) -> std::result::Result<(Channels, cpal::StreamConfig), DeviceError> {
    let format = supported
        .into_iter()
        .filter(|range| {
            range.sample_format() == cpal::SampleFormat::F32 && matches!(range.channels(), 1 | 2)
        })
        .filter_map(|range| range.try_with_sample_rate(sample_rate_hz))
        .min_by_key(|format| format.channels())
        .ok_or(DeviceError::UnsupportedFormat { sample_rate_hz })?;
    let channels = if format.channels() == 1 {
        Channels::Mono
    } else {
        Channels::DualMono
    };
    Ok((channels, format.config()))
}

async fn drive(
    runtime: SynthRuntime,
    output: &mut BatchOutput,
    prefilled: oneshot::Receiver<()>,
    failed: oneshot::Receiver<cpal::Error>,
    start_device: impl FnOnce() -> std::result::Result<(), DeviceError>,
) -> Result<()> {
    tokio::select! {
        biased;
        failure = failed => {
            let error = match failure {
                Ok(source) => DeviceError::Backend { action: "play audio", source },
                Err(_) => DeviceError::CallbackDisconnected,
            };
            Err(error.into())
        }
        outcome = runtime.run(output) => match outcome? {
            RunExit::Stopped => Ok(()),
            RunExit::EndOfStream => Err(DeviceError::EndOfStream.into()),
        },
        startup = async {
            prefilled.await.map_err(|_| DeviceError::CallbackDisconnected)?;
            start_device()?;
            pending::<Result<()>>().await
        } => startup,
    }
}

#[cfg(test)]
mod tests;
