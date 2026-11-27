---
title: "Interactive graph editor: first slice"
status: in progress
date: 2026-11-27
---

# Interactive graph editor: first slice

**Scope:** [#8](https://github.com/DerGut/SonicFusion/issues/8). The design spec is in [`docs/design/editor/`](../design/editor/README.md); the build target is `Studio.dc.html`. Decisions are summarized on [#8](https://github.com/DerGut/SonicFusion/issues/8#issuecomment-7226602945) and [#6](https://github.com/DerGut/SonicFusion/issues/6#issuecomment-7226603327).

## Acceptance (from #8)

A user builds oscillator → gain → output, applies it to a running session, inserts an LPF, and reapplies without restarting the runtime. An invalid edit leaves the previous circuit running and shows a useful error. Hard cuts and fresh processor state are presented honestly.

## Architecture

```
crates/sonicfusion-graph-api   serde only. GraphSpec, catalog, validation, revision status.
crates/sonicfusion-host        sonicfusion + graph-api. Lowering, sessions, probes.
editor/src-tauri               graph-api + host only. Tauri commands and events.
editor/src                     React + TypeScript UI (Vite). EngineClient: Tauri | mock.
```

The Tauri crate must not depend on `sonicfusion`. Every engine interaction goes through `sonicfusion-host`, which only accepts complete graphs.

### `sonicfusion-graph-api`

- `GraphSpec { version, nodes: [NodeSpec { id, kind, params }], wires: [Wire { from, to: { node, port } }] }`.
  - Node kinds: `sine`, `square`, `sawtooth`, `gain`, `low_pass`, `mix`, `output`.
  - Every non-output node has one output.
  - Input ports: `in` for gain, low-pass and output; `cutoff` (optional) for low-pass; `in.0…in.{n-1}` for mix, with `n = weights.len()`.
- `catalog()` describes kinds, ports (audio or control, optional or required) and parameters (unit, range, default, log or linear scale). The UI renders from it.
- `validate(&GraphSpec) -> Vec<GraphIssue>` covers structure only. Each issue has a code, a node, a port and a message.
  - Errors: duplicate or empty ids, a wire to an unknown node or port, two wires into one input, a missing required input, zero or multiple outputs, a cycle, a non-finite or out-of-range parameter.
  - Warning: a node that doesn't reach the output; it isn't lowered.
- `RevisionStatus` mirrors `runtime::ReplacementEvent`: submitted, accepted, superseded, rejected (with an optional node), scheduled, applied, activation-rejected, run-closed.
- `SessionInfo` holds the sample rate, batch size and the engine's fixed LPF modulation mapping.

### `sonicfusion-host`

- **Lowering:** `lower(&GraphSpec)` and `lower_rooted(&GraphSpec, node)` produce DSL text with one statement per line in topological order (`$n<k>` bindings), plus a line → node map.
- **Diagnostics:** the engine's parameter errors (Nyquist, ranges) carry no source position. So `validate` takes `SessionInfo` and mirrors those limits per node, and engine rejection is only a backstop.
- **Fan-out:** fan-out of any node is allowed. Each consumer re-executes its input, which is equivalent because every node is deterministic from the session origin.
- **Session:** `EngineSession` owns the `SynthRuntime` and its `RuntimeControl`.
  - The default device runs on a dedicated thread with a current-thread Tokio runtime, because CPAL streams are not `Send`. Tests inject a `PacedOutput`.
  - `apply(graph)` validates, lowers and calls `submit_replacement`, then forwards events as `RevisionStatus` through a callback.
  - On `Applied`, the session records that revision's graph as the active graph.
- **Probe:** `probe(graph, node, start_frame, frames)` renders the node's ancestor sub-graph offline from the live plan at that frame, after a short warm-up so filter state settles. `level()` probes the active graph's output at the session's next output frame and returns its peak in dBFS. Both are real engine output, recomputed rather than tapped from the device stream.
- **Runtime:** the host owns a multi-thread Tokio runtime and exposes async functions, so the Tauri runtime never blocks on engine work.

### Editor (Tauri v2 + React)

- **Commands:** `catalog`, `session_info`, `validate`, `start_session`, `apply_graph`, `stop_session`, `probe`, `level`. Revision status arrives as a `revision-status` event.
- **UI, per the Studio v2 spec:**
  - Top bar with live/revision status, Start/Stop audio, Apply and an auto-apply toggle.
  - A searchable node palette driven by the catalog.
  - An `@xyflow/react` canvas with custom nodes. Audio ports are circles and control ports diamonds. Audio wires are solid amber, control wires dashed blue.
  - Wires snap within 24 px. Cycles and duplicate inputs are rejected, and dropping a wire onto a used input replaces the old one.
  - Dropping a wire on empty canvas opens a menu that adds a node already connected.
  - The Output node has a segmented level meter and a ceiling, which is a warning threshold only.
  - A Scope/Spectrum probe panel for the selected node, and a status bar.
- **Drafts:** a draft may be invalid. Issues are shown on the nodes, and Apply is disabled until the draft is valid.
- **Project format:** `EditorDocument v1` (graph plus node positions), autosaved to local storage, plus Export `.sf`. File dialogs are a follow-up.

## Tests

- **Rust:**
  - Serde round-trip of a shared fixture (also used by the TS tests).
  - Each validation rule.
  - Lowering builds with `build_live_plan`.
  - Nyquist rejection maps to the right node.
  - Probe samples match a closed-form sine.
  - A session with an injected output applies osc → gain → out, inserts an LPF and reapplies; an invalid edit keeps the previous circuit.
- **TypeScript (Vitest):** document ↔ GraphSpec, connection rules (cycles, replacing an input), wire-drop insertion, spectrum.
- **Manual and agent checks:** run the UI with the mock engine in a browser, and build the Tauri app.

## Out of scope

Stereo (#7), Substrait transport (#6, #27), engine-side limiting, modulation of anything other than LPF cutoff, the deferred Graph/Code view (#28), and file dialogs.
