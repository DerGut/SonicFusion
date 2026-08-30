---
date: 2026-08-29
topic: datafusion-sound-engine
---

# DataFusion Sound Engine Learning Project

## Problem Frame

SonicFusion should be a hands-on environment for learning both Apache DataFusion internals and foundational digital signal processing. The project should produce quick audible and visual feedback while leaving the core implementation work to the learner rather than turning into an agent-built audio framework.

The initial challenge is to discover a useful Arrow representation for audio while learning DataFusion's physical execution model. The project will begin with deterministic offline rendering so that real-time audio constraints do not obscure those lessons.

---

## Key Flows

- F1. Representation experiment
  - **Trigger:** The learner runs the first SonicFusion example.
  - **Steps:** Build the same finite sine-to-gain graph using each candidate Arrow layout; execute it as a DataFusion physical plan; collect the resulting batches once; write both a WAV file and a waveform plot; compare the implementations and outputs.
  - **Outcome:** Both layouts produce equivalent signals, and their trade-offs are documented well enough to choose a direction or justify retaining both.
  - **Covered by:** R4, R5, R6, R7, R8, R9, R10

- F2. Guided learning cycle
  - **Trigger:** The learner starts a milestone or encounters an unfamiliar API.
  - **Steps:** Agree on a small exercise and acceptance checks; the assistant supplies explanation and narrow pair-programming snippets; the learner implements the core; both review the result and extract lessons before advancing.
  - **Outcome:** Each milestone creates working software and an explicit understanding of the DataFusion and DSP concepts involved.
  - **Covered by:** R1, R2, R3

---

## Requirements

**Learning experience**

- R1. The project must optimize for understanding DataFusion and signal processing, not for reaching a feature-complete engine as quickly as possible.
- R2. The initial collaboration style must use explanations and small pair-programming snippets while leaving core nodes and decisions for the learner to implement.
- R3. The collaboration style must be revisited after the first milestone; guided exercises, review-first work, or a more Socratic style may replace it.

**First milestone: physical execution and representation lab**

- R4. The first graph must render a finite, deterministic, mono sine oscillator through a gain processor offline.
- R5. A Rust builder API must compose custom DataFusion physical execution nodes directly for the first milestone, without requiring SQL or custom logical planning.
- R6. The same graph must be implemented with two Arrow layouts: one audio frame per row and one processing block per row.
- R7. Each implementation must preserve sample order, oscillator phase, and gain behavior across multiple RecordBatches so that batch boundaries do not change the signal.
- R8. Each candidate layout must execute and collect exactly once; within that layout, the same in-memory result must produce both its WAV file and waveform plot.
- R9. The two layouts must be checked for equivalent frame count, timing, and sample values within an appropriate floating-point tolerance.
- R10. The representation comparison must document at least: schema clarity, ease of Arrow inspection, execution-node complexity, handling of batch boundaries, conversion to output formats, and likely suitability for later stateful DSP. Performance benchmarking is optional at this stage.

**Later learning milestones**

- R11. After the representation lab, sources should expand to a square oscillator and existing audio samples or tracks, with sources exposed through DataFusion table-provider concepts when that becomes the active learning topic.
- R12. Processing nodes should progress from stateless gain to stateful and multi-input operations such as low-pass filtering, distortion, and mixing.
- R13. Logical planning must be a tracked follow-up rather than disappearing behind the direct-physical prototype: the Rust builder should eventually produce domain-specific logical nodes, and a DataFusion extension planner should convert those nodes into the corresponding physical execution nodes.
- R14. A later optimizer exercise should fuse equivalent adjacent processors, with `gain(0.5).gain(0.5)` becoming `gain(0.25)` while preserving observable output.
- R15. Frequency-domain visualization should accompany the filter and distortion milestones so changes can be inspected as well as heard.
- R16. SQL, streaming fan-out, and real-time playback should each receive an explicit later feasibility decision; they are not implied requirements for the offline learning engine.

---

## Acceptance Examples

- AE1. **Covers R4, R7, R8.** Given a finite sine render whose duration spans several batches, running the example produces one playable WAV and one waveform image without phase discontinuities at batch boundaries.
- AE2. **Covers R6, R9.** Given identical oscillator, gain, sample-rate, duration, and block-size settings, the frame-row and block-row plans produce the same number of frames and numerically equivalent samples.
- AE3. **Covers R10.** After completing both implementations, the learner can explain why a RecordBatch can itself act as a DSP block, what nesting samples inside a row changes, and which layout should be carried forward.
- AE4. **Covers R14.** In the later optimizer milestone, plan inspection shows one gain node with factor `0.25` where two adjacent `0.5` nodes existed, and rendered samples remain equivalent within tolerance.

---

## Follow-up Milestone Tracker

These items are deliberately deferred from milestone one but must remain visible when later plans are created:

- [ ] Add a square oscillator and use its aliasing as a DSP lesson.
- [ ] Wrap generated and file-backed sources in DataFusion `TableProvider` implementations.
- [ ] Add stateful low-pass filtering with continuity tests across RecordBatch boundaries, then add frequency-domain visualization.
- [ ] Add distortion and inspect its generated harmonics.
- [ ] Add a multi-input mixer and define timing, channel, and duration alignment rules.
- [ ] Replace direct physical-plan construction with domain-specific logical nodes that a DataFusion extension planner lowers into physical execution nodes.
- [ ] Add a physical optimizer rule that fuses adjacent gain nodes and exposes before/after plans for inspection.
- [ ] Decide whether SQL adds educational value once the builder and logical planner exist.
- [ ] Decide whether a streaming multi-sink tee is worth adding after the collected offline path is understood.
- [ ] Evaluate a real-time playback bridge only after the offline engine is stable; do not assume DataFusion itself belongs on a hard real-time callback thread.

---

## Success Criteria

- The first milestone gives immediate audible and visual feedback from one command or example run.
- The learner can explain the roles of `ExecutionPlan`, `RecordBatchStream`, plan properties, Arrow schemas, and state carried across batch boundaries.
- The representation choice is based on a completed side-by-side experiment rather than intuition alone.
- The learner authors the core implementation, with the assistant providing bounded snippets, review, and debugging help.
- Later planning can sequence DataFusion and DSP concepts without inventing the project's learning goals or first vertical slice.

---

## Scope Boundaries

- Real-time playback and hard real-time safety are deferred; the initial engine is an offline renderer.
- SQL syntax, a custom SQL dialect, and full logical-plan integration are not part of the first milestone.
- Table-provider wrappers are deferred until after the direct physical execution exercise.
- Stereo and arbitrary channel layouts, mixing, filters, distortion, sample-track sources, and optimizer rules are not part of the first milestone.
- The first milestone will not build a streaming two-sink tee; it collects once and writes both artifacts afterward.
- A DAW, plugin format, sequencer, MIDI system, and production-grade audio engine are outside the current learning scope.
- Early code should favor inspectability and correctness over performance abstraction or speculative extensibility.

---

## Key Decisions

- Offline before real-time: deterministic bounded execution fits DataFusion's model and shortens the feedback loop.
- WAV plus waveform: audio and visualization are both first-class feedback, with frequency-domain views deferred until filter work.
- Rust builder before SQL: the initial interface should expose graph construction without requiring language design.
- Physical plans before logical plans: learn execution and streaming batches before adding planner-extension boilerplate.
- Compare both Arrow representations: implement the same small graph twice and choose from evidence.
- Sine before square: start with an analytically predictable signal and introduce aliasing as a later DSP lesson.
- Collect once, write twice: avoid DAG sharing and backpressure concerns in the first milestone.
- Pair-programming first: use bounded snippets initially and revisit the teaching style after milestone one.

---

## Dependencies / Assumptions

- `SonicFusion/` remains a Rust project and currently contains only starter crate scaffolding.
- A specific DataFusion release will be pinned during planning because physical-plan APIs evolve between releases.
- The first render is intentionally short enough to collect fully in memory.
- WAV encoding and waveform rendering may use small supporting crates; selecting them is a planning decision rather than a learning objective.
- The initial plan can use a single execution partition to make temporal ordering explicit before exploring parallelism.

---

## Outstanding Questions

### Deferred to Planning

- [Affects R6, R10][Technical] What exact schemas and metadata should represent sample rate, channel identity, frame position, and nested blocks in each experiment?
- [Affects R4, R7][Technical] What defaults should be used for sample rate, render duration, and RecordBatch/block size?
- [Affects R5][Needs research] Which pinned DataFusion version provides the best documented current `ExecutionPlan` and stream APIs for the exercise?
- [Affects R8][Technical] Which minimal WAV and waveform-output crates best preserve focus on DataFusion and DSP?
- [Affects R12][Technical] How should later stateful processors expose and test state continuity across batches?

---

## Next Steps

-> `/ce-plan` for structured implementation planning
