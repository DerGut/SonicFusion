---
title: "feat: Normalized frame signals and dynamic low-pass cutoff"
type: feat
status: proposed
date: 2026-09-25
origin: docs/brainstorms/2026-08-29-datafusion-sound-engine-requirements.md
---

# Normalized Frame Signals and Dynamic Low-Pass Cutoff

## Goal

First give frame signals a guaranteed bipolar `[-1, 1]` value range and verify that change on its own. Only after that contract is in place, let `FrameLowPassFilterExec` take a cutoff that changes per frame while keeping its existing constant-cutoff behavior. Any compatible frame signal, including an LFO or an audio-rate oscillator, may modulate the cutoff. Make the cutoff mapping, alignment, missing frames, invalid values, and stream termination explicit so a source cannot silently change the filter's timing or duration.

This is the first implementation of a dynamic node parameter, not a general parameter framework. The graph-facing API can be designed separately; the physical plan must validate its inputs even when a builder has already checked them. The signal's intended use does not make it a different stream type.

## Current Contract

The filter currently takes one `Arc<dyn ExecutionPlan>` audio child and a constant `f64` cutoff. It validates `0 < cutoff_hz < sample_rate_hz / 2`, requires one partition and the frame schema, and keeps one filter state across batches. An absent audio frame advances the state with zero input under the constant coefficient, without producing an output row. `children()` and `with_new_children()` expose one child. The preview graph already creates a 1 Hz oscillator that can serve as a modulation example once mapped to a cutoff in hertz. Sine and square sources already emit within `[-1, 1]`, but gain and mix can exceed it; the render and output paths currently reject only non-finite samples.

## Shared Signal Contract

- Every frame node emits finite samples in `[-1, 1]`, including gain, mix, and LPF. The output decoder checks the range as a final guard for arbitrary DataFusion plans. This is a value guarantee, not an audio-versus-control classification.
- Gain and mix apply explicit hard saturation after their arithmetic: values below `-1` become `-1`, and values above `1` become `1`. Reject non-finite incoming samples before processing. Calculate with enough intermediate precision to avoid overflowing before saturation. Saturation belongs to these node implementations, not to the frame-alignment cursor.

## LPF Contract

- Keep the audio child as the driver. Emit exactly its frame numbers, in order. End when audio ends; an unbounded modulation child must not extend the output or be drained after the last audio row.
- Use the existing non-null `frame: UInt64`, `sample: Float32` frame schema for both children. Any oscillator or other frame signal can be the modulation source. Validate schema compatibility at construction and after child replacement. Require one partition at initial construction and execution; a child rewrite may temporarily introduce multiple partitions while DataFusion inserts a coalescing plan.
- The LPF cutoff port maps a normalized sample `m[n]` to `cutoff_hz[n] = base_hz + depth_hz * m[n]`, so the two range endpoints map to `base_hz - depth_hz` and `base_hz + depth_hz`. Validate finite base and depth and require both mapped endpoints to be strictly between zero and Nyquist at construction; a zero depth is allowed. The constant path retains its direct `cutoff_hz` value. The port owns the interpretation in hertz; the source remains a unitless frame signal.
- Require modulation rows for every integer frame from 0 through the last emitted audio frame. A row may be in any batch, but its frame numbers must start at 0 and increase by exactly one. Reject an early end, gap, duplicate, or out-of-order row with the expected and observed frame numbers. Do not require a modulation row when audio is empty, and ignore modulation rows beyond the final audio frame.
- Validate each consumed modulation sample as finite and within `[-1, 1]`, then validate its resulting cutoff as finite and strictly between zero and Nyquist. Report the frame and invalid value as an execution error; do not silently clamp invalid child data inside the LPF. Keep constant-cutoff validation at construction.
- Audio frames retain their current strictly increasing and non-null checks and must satisfy the common value range. Missing audio frames represent zero input to the filter; they do not produce output rows. For every frame in an audio gap, consume that frame's modulation sample and advance the state with zero input.

For each processed frame `n`, compute `alpha[n] = 1 - exp(-2π cutoff[n] / sample_rate)` and `state[n] = (1 - alpha[n]) * state[n-1] + alpha[n] * input[n]`. Use the existing numerically stable coefficient calculation. Repeated equal cutoffs must produce the same output as the constant path, including across audio gaps and batch boundaries. Each `execute()` starts with zero state and an expected modulation frame of zero.

## Implementation Order

### Phase 1: Enforce the common signal range

- Define `[-1, 1]` as the frame-signal output contract and share a small finite/range validator where useful. Change gain and mix to saturate their results explicitly; update their tests, including the current test that expects an out-of-range mix.
- Reject out-of-range samples in the frame decoder and output writers so a foreign plan cannot bypass the contract. Keep one clear frame number in each error. Update the existing waveform test that passes high-magnitude samples.
- Verify sine and square source bounds and the LPF's bounded-output invariant. Add or adjust tests for gain and mix saturation and for invalid child samples.

**Phase 1 gate:** every built-in frame node produces values in `[-1, 1]`, the output boundary detects a violating foreign plan, and ordinary mixing and gain remain usable with explicit saturation. Run the focused range tests and required checks, and review this change before starting LPF modulation. Do not combine incomplete range enforcement with the dynamic LPF implementation.

### Phase 2: Add dynamic LPF cutoff

Only start this phase after Phase 1's range contract passes. The LPF consumes the same unitless normalized frame signals as other nodes; it converts modulation samples to hertz inside its implementation.

#### Define the cutoff mapping

- Introduce the `FrameSignal` graph wrapper sketched below, validating the exact frame schema when a plan enters the builder. Keep DataFusion child plans erased internally.
- Document the common bipolar input range and the linear base-and-depth mapping. Default to 1,000 Hz base and 900 Hz depth where the sample rate permits; at lower rates, cap the base at half Nyquist and the depth at 90% of the base. Use the existing sine oscillator directly as the modulation child; its frequency may be below or within the audible range.
- Keep constants in the filter's plan state. A constant emitter is optional for graph editing but is not required for execution.

**Done when:** the existing sine oscillator can be wired to the cutoff port without a role conversion, and the port produces the documented cutoff values.

#### Extend the LPF without changing constant behavior

- Represent the cutoff internally as constant data or a modulation child plus base and depth. The dynamic variant has children ordered `[audio, modulation]`; the constant variant retains `[audio]`. `with_new_children()` must preserve this distinction, check child count and schemas, and rebuild properties after DataFusion rewrites.
- Declare single-partition distribution and frame ordering requirements for both dynamic children, and check actual partition counts at execution. Derive output schema, partitioning, ordering, and boundedness from the audio child; the modulation child's duration must not determine output duration. Account for both children when describing execution behavior to DataFusion.
- Pull modulation frames only as far as each audio frame requires. Advance the filter once per intervening frame, using zero audio input for gaps, and emit rows only for actual audio frames. Preserve streaming execution and bounded working memory.
- Keep the existing constant implementation efficient: it can continue using a precomputed coefficient and exponentiation across audio gaps.

**Done when:** the constant constructor and its existing tests still behave identically, and a bounded audio stream with an unbounded modulation stream finishes at the audio boundary.

#### Verify the strict LPF contract

- Compare constant and flat dynamic cutoff outputs on an impulse across multiple batches and an audio gap. Add a hand-calculated changing-cutoff fixture whose coefficient changes inside a gap, then exercise low- and audio-rate oscillator modulation.
- Exercise mismatched batch boundaries, empty audio, modulation ending early, first modulation frame other than zero, missing, duplicate and out-of-order modulation frames, non-finite and out-of-range samples, invalid mapping endpoints, and upstream errors.
- Run the DataFusion distribution and sorting optimizers on a dynamic filter. Verify that both child positions survive `with_new_children()`, state stays continuous, and output rows follow only the audio child.
- Run focused tests, formatting, Clippy, and `git diff --check`.

**Done when:** each malformed modulation stream fails at the first offending frame, valid streams are independent of batch boundaries, and the optimizer retains the same rendered samples.

## Stricter Graph API Sketch

The graph-facing API should accept one kind of `FrameSignal` for audio and modulation. It wraps an `Arc<dyn ExecutionPlan>` and validates the plan's output schema when the plan enters the graph. A marker trait such as `FrameNode: ExecutionPlan` does not by itself enforce `ExecutionPlan::schema()`; a fallible wrapper constructor makes that check unavoidable in the graph-facing API.

Pin the current `frame_schema(config)` as the contract: exactly two ordered, non-null fields, `frame: UInt64` and `sample: Float32`; schema metadata `audio.sample_rate_hz` equal to the configuration, `audio.channels=1`, and `audio.layout=frame`. Compare the full schema, including metadata, so a signal from another sample rate cannot be wired in by accident. The `audio.*` keys are existing timing and layout keys, not a restriction on whether a signal may be used for modulation. Make the schema helper available to authors of custom frame plans, rather than requiring them to duplicate those keys.

The intended API shape is:

```rust
#[derive(Clone)]
pub struct FramePlan(Arc<dyn ExecutionPlan>);

impl FramePlan {
    pub fn try_from_plan(config: &RenderConfig, plan: Arc<dyn ExecutionPlan>)
        -> datafusion::error::Result<Self>;
    pub fn into_plan(self) -> Arc<dyn ExecutionPlan>;
}

pub enum Cutoff {
    ConstantHz(f64),
    Modulated { cutoff: FramePlan },
}

pub fn low_pass(config: &RenderConfig, audio: FramePlan, cutoff: Cutoff)
    -> datafusion::error::Result<FramePlan>;
```

These signatures are a sketch, not a commitment to names. The graph API uses the default base/depth mapping, keeps constants as values, and allows the same oscillator signal to feed either LPF input. The `FrameSignal` check proves schema compatibility at construction; it cannot prove frame ordering, density, or sample range without executing the stream. Nodes must validate the rows they consume, and the output decoder remains a final guard. DataFusion still passes `Arc<dyn ExecutionPlan>` to `with_new_children()`, so each physical node must revalidate replacement children after optimizer rewrites.

Add no new range or unit metadata to signal outputs or LPF inputs. Every frame signal has the same value range and no unit; the LPF converts a sample to hertz internally. The existing sample-rate metadata stays because it distinguishes incompatible frame timelines.

## Scope

This work covers the common frame-signal value range and per-frame cutoff modulation on the existing mono frame timeline. It intentionally changes gain and mix behavior by saturating values outside the range. Slower update rates, interpolation, hold-last-value behavior, smoothing, alternative cutoff mappings such as octaves, dynamic oscillator frequency, and a general graph language need separate contracts. The strict per-frame rule gives those later features an unambiguous starting point.
