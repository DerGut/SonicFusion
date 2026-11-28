/**
 * PROTOTYPE — throwaway. How an oscillator shows an absolute Note input next to
 * the shipped relative FM input, without the two being confused.
 *
 *   npm --prefix editor run dev  →  /?scenario=osc&osc=A|B|C
 *
 * Rule (all variants): base = Note connected ? note(v) : Frequency;
 *                      frequency = base × 2^FM, capped below Nyquist.
 * Note maps v → MIDI 60 + 60·v. FM keeps the shipped ±1 octave mapping.
 *
 * A · "Note leads": Note takes the in/out row; FM moves to the Frequency row it scales.
 * B · "FM stays": FM keeps the in/out row; Note sits on the Frequency row it replaces.
 * C · "Inputs row": one two-line row lists both inputs, each with its port; Frequency becomes "Base".
 */
import { useSyncExternalStore } from "react";

import type { NodeSpec } from "../api/types";
import type { DocNode, EditorDocument } from "../model/document";
import { defaultFrequencyMode, hzToBpm, noteName, type FrequencyMode, type FrequencyRole } from "../model/pitch";
import { formatHz } from "../model/signal";
import { HEADER_HEIGHT, IO_HEIGHT, IO_PORT_TOP, ROW_HEIGHT } from "../components/flow";

export type OscVariant = "A" | "B" | "C";
const VARIANTS: { id: OscVariant; label: string }[] = [
  { id: "A", label: "Note leads · FM on Frequency row" },
  { id: "B", label: "FM stays · Note on Frequency row" },
  { id: "C", label: "Inputs row lists both" },
];

const listeners = new Set<() => void>();
const subscribe = (listener: () => void) => {
  listeners.add(listener);
  return () => listeners.delete(listener);
};
export const prototypeActive = () => new URLSearchParams(window.location.search).get("scenario") === "osc";
export function useOscVariant(): OscVariant {
  const query = useSyncExternalStore(subscribe, () => window.location.search);
  const id = new URLSearchParams(query).get("osc");
  return id === "B" || id === "C" ? id : "A";
}

export function OscSwitcher() {
  const variant = useOscVariant();
  if (!import.meta.env.DEV || !prototypeActive()) return null;
  const index = VARIANTS.findIndex((candidate) => candidate.id === variant);
  const step = (delta: number) => {
    const params = new URLSearchParams(window.location.search);
    params.set("osc", VARIANTS[(index + delta + VARIANTS.length) % VARIANTS.length].id);
    window.history.replaceState(null, "", "?" + params.toString());
    listeners.forEach((listener) => listener());
  };
  return (
    <div className="proto-switcher" role="toolbar" aria-label="Prototype variants">
      <button type="button" aria-label="Previous variant" onClick={() => step(-1)}>‹</button>
      <span><b>{variant}</b> {VARIANTS[index].label}</span>
      <button type="button" aria-label="Next variant" onClick={() => step(1)}>›</button>
    </div>
  );
}

/** Four oscillators, one per state: free, FM only, Note only, both. */
export function oscillatorScenario(doc: EditorDocument): EditorDocument {
  const node = (id: string, kind: DocNode["kind"], params: DocNode["params"], x: number, y: number, displayName: string): DocNode =>
    ({ id, kind, params, position: { x, y }, displayName });
  const wire = (from: string, node: string, port: string) => ({ from, to: { node, port } });
  return {
    ...doc,
    name: "pitch-inputs",
    nodes: [
      node("clock", "square", { frequency_hz: 4, pulse_width: 0.5 }, 32, 60, "Clock"),
      node("line", "melody", { notes: [45, 48, 52, 50] }, 32, 250, "Line"),
      node("lfo", "sine", { frequency_hz: 5 }, 32, 480, "LFO"),
      node("free", "sawtooth", { frequency_hz: 220 }, 300, 60, "Free"),
      node("fm", "sawtooth", { frequency_hz: 220 }, 300, 235, "FM"),
      node("noted", "sawtooth", { frequency_hz: 220 }, 300, 410, "Note"),
      node("both", "sawtooth", { frequency_hz: 220 }, 300, 585, "Note+FM"),
      node("mix", "mix", { weights: [0.25, 0.25, 0.25, 0.25] }, 580, 260, "Mix"),
      node("out", "output", {}, 820, 260, "out"),
    ],
    wires: [
      wire("clock", "line", "advance"),
      wire("lfo", "fm", "frequency"),
      wire("line", "noted", "note"),
      wire("line", "both", "note"),
      wire("lfo", "both", "frequency"),
      wire("free", "mix", "in.0"),
      wire("fm", "mix", "in.1"),
      wire("noted", "mix", "in.2"),
      wire("both", "mix", "in.3"),
      wire("mix", "out", "in"),
    ],
  };
}

// ---------------------------------------------------------------- readouts

/** What a Note source will play, when the editor can tell. */
export function noteRange(source: DocNode | undefined): string {
  if (!source) return "—";
  if (source.kind === "melody" && Array.isArray(source.params.notes)) {
    const notes = source.params.notes as number[];
    const low = Math.min(...notes), high = Math.max(...notes);
    return low === high ? noteName(low) : `${noteName(low)}–${noteName(high)}`;
  }
  if (source.kind === "constant" && typeof source.params.value === "number") {
    return noteName(Math.round(60 + 60 * Math.max(-1, Math.min(1, source.params.value))));
  }
  return "follows input";
}

type Source = { id: string; label: string } | undefined;

/**
 * The Note input speaks the receiving oscillator's unit: it reads "Note" on a
 * note-led oscillator, "Freq" on a Hz-led one and "Tempo" on a BPM-led one.
 * Only the label and readout change; the port and its mapping don't.
 */
export interface NoteView {
  unit: FrequencyMode;
  name: string;
  row: string;
  readout: string;
  /** The unit the readout swaps to: Note ⇄ Hz when audible, Hz ⇄ BPM below 20 Hz. */
  next?: FrequencyMode;
}

const midiHz = (midi: number) => 440 * 2 ** ((midi - 69) / 12);
const hzText = (hz: number) => (hz >= 1000 ? `${Number((hz / 1000).toPrecision(3))} kHz` : `${Number(hz.toPrecision(3))} Hz`);
const span = <T,>(low: T, high: T, text: (value: T) => string) => (low === high ? text(low) : `${text(low)}–${text(high)}`);

/** MIDI notes a source will send, when the editor can tell. */
function incomingNotes(source: DocNode | undefined): number[] | null {
  if (source?.kind === "melody" && Array.isArray(source.params.notes)) return source.params.notes as number[];
  if (source?.kind === "constant" && typeof source.params.value === "number") {
    return [60 + 60 * Math.max(-1, Math.min(1, source.params.value))];
  }
  return null;
}

export function noteView(source: DocNode | undefined, override: FrequencyMode | undefined, role: FrequencyRole, baseHz: number): NoteView {
  // Unconnected, the label follows the oscillator's own Frequency and unit.
  const notes = source ? incomingNotes(source) : baseHz > 0 ? [69 + 12 * Math.log2(baseHz / 440)] : null;
  if (!notes) return { unit: "note", name: "♪ Note", row: "Pitch", readout: "follows input" };
  const low = Math.min(...notes), high = Math.max(...notes);
  const lowHz = midiHz(low), highHz = midiHz(high);
  const audible = lowHz >= 20, rhythmic = highHz < 20;
  const unit: FrequencyMode = override === "hz" || (override === "note" && audible) || (override === "bpm" && rhythmic)
    ? override : defaultFrequencyMode(lowHz, role, true);
  const next: FrequencyMode | undefined = audible ? (unit === "note" ? "hz" : "note") : rhythmic ? (unit === "bpm" ? "hz" : "bpm") : undefined;
  if (unit === "note") return { unit, next, name: "♪ Note", row: "Pitch", readout: span(Math.round(low), Math.round(high), noteName) };
  const bpm = (hz: number) => Number(hzToBpm(hz).toPrecision(4));
  if (unit === "bpm") return { unit, next, name: "♩ Tempo", row: "Tempo", readout: lowHz === highHz ? `${bpm(lowHz)} BPM` : `${bpm(lowHz)}–${bpm(highHz)} BPM` };
  const sameScale = (lowHz >= 1000) === (highHz >= 1000);
  const readout = lowHz === highHz || !sameScale ? span(lowHz, highHz, hzText)
    : `${hzText(lowHz).split(" ")[0]}–${hzText(highHz)}`;
  return { unit, next, name: "Freq", row: "Frequency", readout };
}

/** Row heights are fixed, so ports can be placed inside a row. */
export const rowTop = (index: number) => HEADER_HEIGHT + IO_HEIGHT + ROW_HEIGHT * index;

/**
 * The Frequency row while Note is connected: the row keeps its "Pitch" label and
 * shows where the pitch comes from; the stored Frequency is kept but unused.
 */
export function NoteDrivenRow({ note, fm, view, base, variant, onUnit }: {
  note: NonNullable<Source>;
  fm: Source;
  view: NoteView;
  base: number;
  variant: OscVariant;
  onUnit: (unit: FrequencyMode) => void;
}) {
  return (
    <div className={`param modulated proto-note-row${variant === "C" ? " base-unused" : ""}`}>
      <div className="param-top">
        <span>{variant === "C" ? "Base" : view.row}<PortBadge variant={variant} on={variant === "A" ? Boolean(fm) : true} label={view.name} /></span>
        {variant === "C" ? <span className="value">{formatHz(base)}</span> : <UnitSwap view={view} onUnit={onUnit} />}
      </div>
      <p className="param-note" title={`Frequency (${formatHz(base)}) is kept but unused while Note is connected`}>
        {variant === "C" ? "unused while Note is connected" : <>♪ {note.label} · replaces Frequency</>}
      </p>
    </div>
  );
}

/** Names the port that sits on the Frequency row: FM in A, Note in B. */
export function PortBadge({ variant, on, label = "♪ Note" }: { variant: OscVariant; on: boolean; label?: string }) {
  if (variant === "C") return null;
  return <span className={`proto-port-badge${on ? " on" : ""}`}>{variant === "A" ? "∿FM" : label.replace(" ", "")}</span>;
}

/** The incoming value in the oscillator's unit; clicking swaps the unit, as on the Frequency row. */
export function UnitSwap({ view, onUnit }: { view: NoteView; onUnit: (unit: FrequencyMode) => void }) {
  if (!view.next) return <span className="value">{view.readout}</span>;
  const nextName = view.next === "note" ? "notes" : view.next === "bpm" ? "BPM" : "Hz";
  return (
    <button type="button" className="value nodrag proto-unit-swap" title={`Show in ${nextName}`} onClick={() => onUnit(view.next!)}>
      {view.readout}
    </button>
  );
}

/** C: one row naming both inputs, so each port has a label right next to it. */
export function InputsRow({ note, fm, view, onUnit }: { note: Source; fm: Source; view: NoteView; onUnit: (unit: FrequencyMode) => void }) {
  return (
    <div className="param proto-inputs-row">
      <div className={`proto-input${note ? " on" : ""}`}>
        <span className="proto-input-name">{view.name}</span>
        {note ? (
          <span className="proto-input-state" title={`from ${note.label}`}><UnitSwap view={view} onUnit={onUnit} /> · {note.label}</span>
        ) : <span className="proto-input-state">sets the {view.unit === "note" ? "note" : view.unit === "bpm" ? "tempo" : "frequency"} · uses Base</span>}
      </div>
      <div className={`proto-input${fm ? " on" : ""}`}>
        <span className="proto-input-name">∿ FM</span>
        <span className="proto-input-state" title={fm ? `from ${fm.label}` : undefined}>{fm ? `±1 oct · ${fm.label}` : "bends ±1 octave"}</span>
      </div>
    </div>
  );
}

/** The result, once any pitch input is connected: what the oscillator plays. */
export function PitchSummary({ note, fm, view, base }: { note: Source; fm: Source; view: NoteView; base: number }) {
  if (!note && !fm) return null;
  const root = note ? view.readout : formatHz(base);
  return (
    <p className="proto-pitch-summary" title={`Note sets the base; FM scales it by 2^control, up to one octave each way${fm ? ` (FM from ${fm.label})` : ""}`}>
      Plays <strong>{root}</strong>{fm ? <> ±1 oct (FM)</> : null}
    </p>
  );
}

/** Port positions for each variant: [note, fm]. */
export function portTops(variant: OscVariant, frequencyRow: number): { note: number; fm: number } {
  if (variant === "A") return { note: IO_PORT_TOP, fm: rowTop(frequencyRow) + 16 };
  if (variant === "B") return { note: rowTop(frequencyRow) + 16, fm: IO_PORT_TOP };
  return { note: rowTop(0) + 15, fm: rowTop(0) + 36 };
}

export function ioLabel(variant: OscVariant): string {
  return variant === "A" ? "note" : variant === "B" ? "FM ±1 octave" : "";
}

export function melodySummary(node: NodeSpec): string {
  const notes = (node.params.notes as number[] | undefined) ?? [];
  return `${notes.length} notes · ${noteRange(node as DocNode)}`;
}
