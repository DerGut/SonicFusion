---
title: "feat: Shorten the graph-to-listening feedback cycle"
type: feat
status: proposed
date: 2026-09-25
origin: docs/brainstorms/2026-08-29-datafusion-sound-engine-requirements.md
---

# Shorten the Graph-to-Listening Feedback Cycle

## Goal

Make a graph change audible with less manual work, then use the same output boundary for direct playback. Measure the time from invoking a preview (and, later, saving a graph change) to playback startup. Track build or graph-load time, time to the first decoded batch, output preparation, and player launch or first device callback separately; confirm perceived time to sound by listening. Record a baseline before setting a latency target.

This is follow-up work to the offline representation lab. It addresses the later playback feasibility decision in R16 of the [project requirements](../brainstorms/2026-08-29-datafusion-sound-engine-requirements.md). A better listening loop does not require claiming that DataFusion provides audio deadlines. DataFusion execution may produce batches incrementally; the output bridge owns pacing and buffers.

## Current Path

`examples/render.rs` and `examples/complex_render.rs` build a physical graph in Rust, cap it with `GlobalLimitExec`, call `collect`, decode all batches into one sample vector, and write WAV and SVG files. The user then opens the WAV separately. `decode_from_frames` validates schema, contiguous frame numbers, finite samples, and exact configured coverage, but it accepts only fully collected batches. Oscillator executions start at frame zero and emit unbounded batches; the finite limit belongs to the offline render root.

The initial output is mono `f32` at the configured sample rate. `RenderConfig` defaults to 48 kHz and 1,024 frames per batch (about 21 ms of audio per batch). That batch duration is a useful tuning input, not a prediction of end-to-end latency.

## Milestones

### 1. One-command recorded preview

- Add a small preview command or example that builds a graph, renders a configurable short excerpt, writes a WAV, and launches a local player. Keep waveform generation available but optional for quick listening.
- Keep the existing bounded render and sample validation. Report the WAV path and distinguish render, file, and player-launch errors so a failed launch cannot look like a successful listen.
- Make the playback command configurable or provide a clear platform-specific default; do not bind the library to one desktop player.
- Record elapsed time at the stages named in the goal. Include compilation time separately when the graph is still edited as Rust source.

**Done when:** one invocation plays the newly rendered excerpt without manual file navigation; a replay never silently uses an older WAV; the rendered samples match the existing example for the same graph and configuration.

### 2. Incremental frame decoding and output

- Extract the frame validation in `decode_from_frames` into a stateful decoder that accepts successive `RecordBatch`es and yields ordered sample chunks. Preserve the existing schema, continuity, and finite-sample checks. For finite renders, verify exact coverage at end of stream.
- Keep the collected decoder as a convenience caller of this shared validation, then add a bounded streaming WAV writer that consumes the plan's batch stream without first collecting every batch and sample. Write to a temporary path and publish the finalized WAV only after successful completion, since dropping a WAV writer may otherwise leave a playable partial file.
- Treat output destinations as consumers of validated chunks. Keep optional SVG generation simple; add streaming fan-out only if a real use case justifies its queueing and failure behavior.
- Define cancellation and error propagation so an upstream failure cannot be presented as a complete recording.

**Done when:** collected and streaming paths produce equivalent samples across different batch sizes; gaps, duplicates, non-finite samples, and early termination fail with the offending frame; the streaming recording uses memory bounded by its working buffers rather than the full duration.

### 3. Buffered direct playback spike

- Run DataFusion plan execution and decoding on a producer task or thread. Feed a bounded sample queue; let the audio-device callback copy ready samples without polling DataFusion, blocking, or allocating.
- Establish the device's sample rate, channel count, and sample format at the output boundary. Start with the simplest supported mono path; specify conversion or an explicit unsupported-device error before claiming broader device support. Apply a deliberate master output policy for samples outside `[-1, 1]` rather than silently changing mixer behavior.
- Prefill the queue before starting playback. Define an underrun policy (for example, output silence and count the missing frames), queue backpressure, stop and cancellation behavior, and device-error reporting.
- Measure time to first sound, queue depth, batch production time, and underruns while varying batch capacity and prefill. Test the queue and decoder with a fake device consumer; use a separate manual device smoke check.

**Done when:** a finite graph plays directly without an intermediate WAV, playback can stop cleanly, and the run reports underruns and measured startup time. This is a feasibility result, not a hard real-time guarantee.

### 4. Resident graph editing

- Keep a runner and audio device open so a graph edit need not relaunch the process. Start with editable parameters or a small graph description for the existing sources, gains, and mixer; avoid designing a general graph language before the editing loop is tested.
- Construct and validate a replacement plan away from the device callback. Swap at a defined audio boundary, with a short crossfade if needed to avoid a click. Preserve the previous graph and report errors if a replacement cannot be built.
- Decide timeline semantics explicitly. Current oscillator `execute` calls restart at frame zero; a replacement can intentionally restart its phase, or a later execution interface can accept a shared absolute frame position. Do not imply phase or state continuity before it exists.
- Measure save-to-sound time independently from compilation, graph construction, and audio buffering. Use these measurements to decide whether the editing interface needs further work.

**Done when:** a valid edit becomes audible in the running process, an invalid edit leaves the prior graph playing, and the chosen phase/reset behavior is documented and tested.

## Decisions to Make During Implementation

- **Recorded preview:** local player invocation and portable fallback behavior.
- **Direct output:** device library, supported host formats, queue capacity, prefill, and underrun policy. CPAL is a candidate to evaluate, not a dependency chosen by this plan.
- **Editing:** whether the first resident interface is parameter control, file reload, or both; how an edit maps to a replacement physical plan.
- **Timing:** whether graph replacement restarts at frame zero or follows a shared absolute frame clock. Stateful DSP will need an explicit migration or reset rule too.
- **Output level:** clipping, limiting, or explicit rejection at the device boundary; keep gain and mixing semantics in their graph nodes.

## Scope

This plan starts with the existing single-partition frame graph. It does not require SQL, logical planning, a general sequencer, or a hard real-time DataFusion executor. The finite preview remains available even if the direct playback spike finds unacceptable underruns. Preserve deterministic offline WAV and waveform output as reference results while developing the faster loop.
