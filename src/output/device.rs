//! Optional native playback for a live mono execution. Processing publishes
//! complete batches; only the host callback determines playback timing.

mod bridge;

use std::{future::Future, sync::Arc, time::Duration};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use tokio::sync::oneshot;

use crate::{
    ExecutionConfig, Result,
    runtime::{RunExit, SynthRuntime},
};

use self::bridge::{Channels, Counters};

#[derive(Debug, thiserror::Error)]
pub enum DeviceError {
    #[error("no default audio output device")]
    NoDefaultDevice,
    #[error("default device has no native f32 mono or two-channel output at {sample_rate_hz} Hz")]
    UnsupportedFormat { sample_rate_hz: u32 },
    #[error("default device has no native f32 mono or stereo output; advertised: {advertised}")]
    NoCompatibleFormat { advertised: String },
    #[error("runtime rate {runtime_hz} Hz does not match the selected output rate {output_hz} Hz")]
    RuntimeRateMismatch { runtime_hz: u32, output_hz: u32 },
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

/// A native output device and format selected before building a live graph.
/// The selection is fixed for the session; it does not follow default device changes.
pub struct DeviceOutput {
    device: cpal::Device,
    channels: Channels,
    config: cpal::StreamConfig,
}

impl DeviceOutput {
    /// Select the current default output without opening a stream. Use
    /// [`Self::sample_rate_hz`] to configure the graph and runtime before playback.
    /// Only native `f32` mono and stereo formats are considered. Prefer the
    /// device’s default sample rate, or the closest supported rate if needed.
    pub fn default_device() -> Result<Self> {
        Self::select_default(None)
    }

    fn select_default(exact_rate: Option<u32>) -> Result<Self> {
        let device = cpal::default_host()
            .default_output_device()
            .ok_or(DeviceError::NoDefaultDevice)?;
        let supported =
            device
                .supported_output_configs()
                .map_err(|source| DeviceError::Backend {
                    action: "query default output formats",
                    source,
                })?;
        let (channels, config) = match exact_rate {
            Some(rate) => select_config(supported, rate)?,
            None => {
                let preferred_rate = device.default_output_config().ok().map(|c| c.sample_rate());
                negotiate_config(&supported.collect::<Vec<_>>(), preferred_rate)?
            }
        };
        Ok(Self {
            device,
            channels,
            config,
        })
    }

    pub fn sample_rate_hz(&self) -> u32 {
        self.config.sample_rate
    }

    pub fn channels(&self) -> u16 {
        self.config.channels
    }

    /// Play the runtime on the selected device using a Tokio executor with timers.
    /// The runtime must match the selected rate and supply complete batches.
    /// Playback starts after at least two batches are ready and queues up to three.
    /// Stereo output duplicates mono. The host chooses the callback buffer size.
    /// Call [`crate::runtime::RuntimeControl::stop`] to stop and obtain stats.
    /// Stop discards queued audio; graph and device failures return an error.
    pub async fn play(self, runtime: SynthRuntime) -> Result<DevicePlaybackStats> {
        let config = runtime.config();
        if config.sample_rate_hz() != self.sample_rate_hz() {
            return Err(DeviceError::RuntimeRateMismatch {
                runtime_hz: config.sample_rate_hz(),
                output_hz: self.sample_rate_hz(),
            }
            .into());
        }
        play_selected(self, runtime).await
    }
}

/// Play a prebuilt runtime at its exact rate on the current default device.
/// For startup format negotiation, use [`DeviceOutput::default_device`] before
/// building the runtime. See [`DeviceOutput::play`] for playback behavior.
pub async fn play_default_device(runtime: SynthRuntime) -> Result<DevicePlaybackStats> {
    DeviceOutput::select_default(Some(runtime.config().sample_rate_hz()))?
        .play(runtime)
        .await
}

async fn play_selected(output: DeviceOutput, runtime: SynthRuntime) -> Result<DevicePlaybackStats> {
    let DeviceOutput {
        device,
        channels,
        config: device_config,
    } = output;
    let config = runtime.config();
    let control = runtime.control();
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
    let result = drive(runtime.run(&mut output), prefilled, failed, || {
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

fn is_playable(range: &cpal::SupportedStreamConfigRange) -> bool {
    range.sample_format() == cpal::SampleFormat::F32 && matches!(range.channels(), 1 | 2)
}

fn negotiate_config(
    supported: &[cpal::SupportedStreamConfigRange],
    preferred_rate: Option<u32>,
) -> std::result::Result<(Channels, cpal::StreamConfig), DeviceError> {
    // If the host's preferred rate isn't playable, use the closest playable
    // rate (lower wins a tie). If the host has no preference, aim for 48 kHz.
    let target = preferred_rate.unwrap_or_else(|| ExecutionConfig::default().sample_rate_hz());
    let rate = supported
        .iter()
        .filter(|range| is_playable(range))
        .filter_map(|range| {
            let min = range.min_sample_rate().max(1);
            let max = range.max_sample_rate();
            (min <= max).then(|| target.clamp(min, max))
        })
        .min_by_key(|rate| (rate.abs_diff(target), *rate))
        .ok_or_else(|| DeviceError::NoCompatibleFormat {
            advertised: describe_formats(supported),
        })?;
    select_config(supported.iter().copied(), rate)
}

fn describe_formats(supported: &[cpal::SupportedStreamConfigRange]) -> String {
    if supported.is_empty() {
        return "none".into();
    }
    supported
        .iter()
        .map(|range| {
            let min = range.min_sample_rate();
            let max = range.max_sample_rate();
            let rates = if min == max {
                min.to_string()
            } else {
                format!("{min}–{max}")
            };
            format!(
                "{} ch {} at {rates} Hz",
                range.channels(),
                range.sample_format()
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn select_config(
    supported: impl IntoIterator<Item = cpal::SupportedStreamConfigRange>,
    sample_rate_hz: u32,
) -> std::result::Result<(Channels, cpal::StreamConfig), DeviceError> {
    let format = supported
        .into_iter()
        .filter(is_playable)
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
    worker: impl Future<Output = Result<RunExit>>,
    prefilled: oneshot::Receiver<()>,
    mut failed: oneshot::Receiver<cpal::Error>,
    start_device: impl FnOnce() -> std::result::Result<(), DeviceError>,
) -> Result<()> {
    let worker = async {
        match worker.await? {
            RunExit::Stopped => Ok(()),
            RunExit::EndOfStream => Err(DeviceError::EndOfStream.into()),
        }
    };
    tokio::pin!(worker);
    tokio::select! {
        biased;
        failure = &mut failed => return Err(callback_error(failure.ok()).into()),
        outcome = &mut worker => return outcome,
        ready = prefilled => ready.map_err(|_| DeviceError::CallbackDisconnected)?,
    }

    // A device failure can arrive during the worker poll that signals prefill,
    // after select already checked the error channel in that turn.
    match failed.try_recv() {
        Ok(source) => return Err(callback_error(Some(source)).into()),
        Err(oneshot::error::TryRecvError::Closed) => return Err(callback_error(None).into()),
        Err(oneshot::error::TryRecvError::Empty) => {}
    }
    start_device()?;
    tokio::select! {
        biased;
        failure = failed => Err(callback_error(failure.ok()).into()),
        outcome = worker => outcome,
    }
}

fn callback_error(source: Option<cpal::Error>) -> DeviceError {
    match source {
        Some(source) => DeviceError::Backend {
            action: "play audio",
            source,
        },
        None => DeviceError::CallbackDisconnected,
    }
}

#[cfg(test)]
mod tests;
