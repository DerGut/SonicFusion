# SonicFusion editor

A desktop graph editor for SonicFusion circuits: a Tauri v2 shell with a React + TypeScript UI. It follows the Studio v2 design in [`docs/design/editor`](../docs/design/editor/README.md). The plan and its decisions are in [`docs/plans/2026-11-27-002-feat-graph-editor-plan.md`](../docs/plans/2026-11-27-002-feat-graph-editor-plan.md).

```sh
cd editor
npm install
npm run tauri dev   # desktop app with the real engine
npm run dev         # browser only, against a mock engine (no sound)
npm test            # UI unit tests
cd src-tauri && cargo test   # command layer, headless
cargo test -p sonicfusion-host --test device_session -- --ignored   # real output, silent
```

## How it reaches the engine

```
React UI ──invoke/events──▶ editor/src-tauri ──▶ sonicfusion-host ──▶ sonicfusion
                                   │                    │
                                   └── sonicfusion-graph-api (serde types) ──┘
```

`editor/src-tauri` doesn't depend on `sonicfusion`. It sends complete `GraphSpec`s to `sonicfusion-host`, which:
- validates them against the session
- lowers them to the DSL
- submits them as whole-graph replacements

The node vocabulary (kinds, ports, parameter ranges) comes from `sonicfusion_graph_api::catalog()`. The browser mock reads the same catalog from `crates/sonicfusion-graph-api/fixtures/catalog.json`. A Rust test keeps that file current; regenerate it with `UPDATE_FIXTURES=1 cargo test -p sonicfusion-graph-api`.

## Behaviour worth knowing

- **Applying is a hard cut.** The new circuit starts at the next 1024-frame block with fresh filter state. The previous circuit keeps playing until then, and keeps playing if the engine rejects an edit.
- **Drafts may be invalid.** Errors appear on the nodes and in the status bar, and nothing is submitted until the current draft has been checked and has no errors. Auto-apply submits valid drafts 350 ms after edits settle.
- **Nodes that don't reach the output only warn.** They aren't played, so their problems don't block applying. Wires the engine wouldn't use (for example out of the output node) are ignored with a warning.
- **Circuits are bounded.** A chain may be at most 256 nodes deep. The engine re-runs a shared node once per consumer, so reconvergent fan-out is capped at 256 node executions per circuit.
- **Adding nodes:** drag from the palette, press Tab over the canvas, or drop a wire on empty canvas to add a node that's already connected.
- **Moving wires:** drag from a connected input to pick up its wire, then drop it on another input to move it, or on empty canvas to detach it. Dragging an output adds a wire; ⌥-dragging one hands an existing wire to another output.
- **The level meter and the Scope/Spectrum probe show engine output.** The host renders a node's signal offline at the session's current frame; it isn't tapped from the device stream. The UI only draws it: it picks the scope's trigger point and computes the spectrum's FFT. The meter follows what is playing; the probe follows the draft and says so when the draft isn't applied. The meter's ceiling is a warning threshold only, because the engine has no limiter.
- **Frequency display.** At 20 Hz and above, oscillators and adjustable cutoffs show both Hz and the note. Modulators lead with Hz. Click the secondary value to swap the display for that node. A note-led slider snaps to semitones. Hold Alt (Option on macOS) to adjust it freely. Click the primary value for the note keyboard, detune, and exact entry in Hz or as a note such as `C#4`. Notes use A4 = 440 Hz and C4 = MIDI 60. The Spectrum adds the closest note when the probe window resolves the pitch.
- **Step sequencer.** The Sequencer node outputs a 0/1 gate from a one-bar, 16-step pattern. Select it to paint steps in the panel's Pattern tab (drag to paint, Space or Enter toggles, Clear/Shift/Invert); one drag is one undo step. Without a clock it runs four steps per beat at its tempo; a connected clock advances one step per rising edge and the tempo row shows the source instead. Its Scope shows one bar.
- **BPM entry.** Below 20 Hz, an oscillator shows Hz and BPM. A gate source leads with BPM. Click the secondary value to swap units. A BPM-led slider snaps to whole BPM unless Alt is held. Click the BPM value for presets, halve, double, tap tempo, and exact entry below 1200 BPM. Oscillator fields accept `120 bpm` or `2 Hz` with either unit selected. One beat is one oscillator cycle. An explicit unit autosaves with the node, and Auto restores its role-based display. The graph stores Hz.
- **Sine and Sawtooth frequency modulation.** Their optional FM port maps control `m` to `base Hz × 2^m`: −1 halves the frequency, 0 keeps it, and +1 doubles it. The result is capped just below the session’s Nyquist frequency; a zero base stays zero. The base control stays editable, and disconnecting FM restores fixed frequency. Phase advances per frame and carries across batches.
- **Low-pass cutoff modulation** uses the engine's fixed mapping (1 kHz ± 900 Hz at 44.1 and 48 kHz).
- **Constant** emits a steady value in [-1, 1]. Connect it to Sine/Sawtooth FM or Low-pass cutoff to see the mapped Hz and supported frequency range for each destination. The raw slider value is saved with the patch and can be auto-applied during playback.
- **Gain** multiplies the incoming signal. Its slider runs from 0 to 4: 0 mutes, values below 1 lower the level, 1 leaves it unchanged, and values above 1 amplify it. Click Factor to type an exact value, including values beyond the slider range. Gain clips samples to [-1, 1], so amplification can distort a signal that runs out of headroom.
- **The session runs at the default output's native rate.** The engine doesn't resample. The editor checks drafts against the rate the output had at launch; if the default output has changed when you press Play, the engine is rebuilt at the new rate and the draft is checked again, because limits like Nyquist move with it. The output isn't followed mid-session.
- **Projects autosave** to local storage. Export `.sf` shows the DSL to run with `live_runtime`.
