/**
 * PROTOTYPE — throwaway. Where do notes go once the step sequencer (#57) exists?
 *
 * `?mel=` A (extend: the sequencer always has notes and a pitch output)
 *         B (toggle: a Notes switch adds the note lane and the pitch output)
 *         C (separate: a Melody node steps through notes on each gate edge)
 * `?scenario=mel` seeds the matching patch.
 *
 * Pitch signals map v → MIDI 60 + 60·v (C4 at 0, ±5 octaves). An oscillator's
 * new Pitch input overrides its Frequency while connected.
 */
import { useState } from "react";

import type { DocNode, EditorDocument } from "../model/document";
import { noteName } from "../model/pitch";
import { useDocNodes, useNodeLabel, useVariant } from "./design-33-41.prototype";

const BASS_STEPS = [1, 0, 0, 1, 0, 0, 1, 0, 0, 0, 1, 0, 1, 0, 0, 0];
const BASS_NOTES = [45, 45, 45, 48, 48, 48, 52, 52, 52, 52, 50, 50, 47, 47, 47, 47];
const MELODY = [45, 48, 52, 50, 47];

export function notesScenario(doc: EditorDocument, variant: string): EditorDocument {
  const node = (id: string, kind: DocNode["kind"], params: DocNode["params"], x: number, y: number, displayName?: string): DocNode =>
    ({ id, kind, params, position: { x, y }, ...(displayName ? { displayName } : {}) });
  const separate = variant === "C";
  return {
    ...doc,
    name: "bassline",
    nodes: [
      node("seq", "sequencer", { tempo_bpm: 120, gate: 0.5, steps: BASS_STEPS, ...(separate ? {} : { notes: BASS_NOTES, notes_on: 1 }) }, 32, 60, "Rhythm"),
      ...(separate ? [node("mel", "melody", { notes: MELODY }, 272, 220, "Line")] : []),
      node("bass", "sawtooth", { frequency_hz: 110 }, separate ? 512 : 272, separate ? 220 : 260, "Bass"),
      node("env", "envelope", { attack_s: 0.005, decay_s: 0.12, sustain: 0.4, release_s: 0.08 }, separate ? 752 : 512, 60),
      node("out", "output", {}, separate ? 992 : 752, 60),
    ],
    wires: [
      { from: "seq", to: { node: "env", port: "gate" } },
      ...(separate
        ? [{ from: "seq", to: { node: "mel", port: "advance" } }, { from: "mel", to: { node: "bass", port: "pitch" } }]
        : [{ from: "seq", to: { node: "bass", port: "pitch" } }]),
      { from: "bass", to: { node: "env", port: "in" } },
      { from: "env", to: { node: "out", port: "in" } },
    ],
  };
}

/** Whether a sequencer node currently has a note lane and pitch output. */
export function useSequencerNotes(node: DocNode): boolean {
  const variant = useVariant("mel");
  const active = new URLSearchParams(window.location.search).get("scenario") === "mel";
  if (!active || node.kind !== "sequencer" || variant === "C") return false;
  return variant === "A" || Number(node.params.notes_on ?? 0) === 1;
}

export function useNotesToggle(): boolean {
  return useVariant("mel") === "B" && new URLSearchParams(window.location.search).get("scenario") === "mel";
}

const notesOf = (node: DocNode | undefined): number[] =>
  Array.isArray(node?.params.notes) ? (node!.params.notes as number[]) : [];

/** Notes a source can actually play: active steps for a sequencer, every slot for a melody. */
function playable(node: DocNode | undefined): number[] {
  if (!node) return [];
  const notes = notesOf(node);
  if (node.kind === "melody") return notes;
  const steps = (node.params.steps as number[] | undefined) ?? [];
  return notes.filter((_, index) => steps[index]);
}

function range(notes: number[]): string {
  if (notes.length === 0) return "—";
  const low = Math.min(...notes), high = Math.max(...notes);
  return low === high ? noteName(low) : `${noteName(low)}–${noteName(high)}`;
}

/** A tiny contour of the notes, one bar per slot; inactive steps are dimmed. */
export function Contour({ notes, mask }: { notes: number[]; mask?: number[] }) {
  const active = notes.filter((_, index) => !mask || mask[index]);
  const low = Math.min(...active, 127), high = Math.max(...active, 0);
  const span = Math.max(1, high - low);
  return (
    <div className="proto-contour" style={{ gridTemplateColumns: `repeat(${notes.length}, 1fr)` }} aria-hidden="true">
      {notes.map((note, index) => (
        <span key={index} className={mask && !mask[index] ? "off" : ""} style={{ height: `${25 + (75 * (note - low)) / span}%` }} />
      ))}
    </div>
  );
}

/** The Notes row on a sequencer (A always, B behind the switch). Its pitch port sits on this row. */
export function NotesRow({ node, setParam }: { node: DocNode; setParam: (name: string, value: number | number[], continuous: boolean) => void }) {
  const toggle = useNotesToggle();
  const on = useSequencerNotes(node);
  const steps = (node.params.steps as number[] | undefined) ?? [];
  const notes = notesOf(node);
  return (
    <div className="param proto-notes-row">
      <div className="param-top">
        <span>Notes</span>
        <span className="proto-notes-right">
          {on && <span className="value">{range(playable(node))}</span>}
          {toggle && (
            <button type="button" role="switch" aria-checked={on} className={`proto-switch nodrag${on ? " on" : ""}`}
              title={on ? "Turn notes off: removes the pitch output" : "Turn notes on: adds a note per step and a pitch output"}
              onClick={() => {
                setParam("notes_on", on ? 0 : 1, false);
                if (!on && notes.length !== steps.length) setParam("notes", steps.map(() => 48), false);
              }}>
              <span />
            </button>
          )}
        </span>
      </div>
      {on ? <Contour notes={notes} mask={steps} /> : <p className="param-note">off · gate only</p>}
    </div>
  );
}

/** C: the Melody node's rows. The Advance port sits on the first row. */
export function melodyRows({ node, advanceFrom, openPanel }: { node: DocNode; advanceFrom: string | undefined; openPanel: () => void }) {
  const label = useNodeLabel();
  const nodes = useDocNodes();
  const notes = notesOf(node);
  return [
    <div key="advance" className={`param proto-gate${advanceFrom ? "" : " invalid"}`}>
      <div className="param-top">
        <span className="proto-gate-label">Advance</span>
        <span className="proto-gate-rule">next note per hit</span>
      </div>
      <p className="param-note proto-gate-note">
        {advanceFrom ? <>from {label(nodes.get(advanceFrom), advanceFrom)}</> : "not connected · holds the first note"}
      </p>
    </div>,
    <div key="notes" className="param proto-notes-row">
      <div className="param-top">
        <span>Notes</span>
        <button type="button" className="value nodrag proto-link" onClick={openPanel}>{notes.length} · {range(notes)} · edit</button>
      </div>
      <Contour notes={notes} />
    </div>,
  ];
}

/** An oscillator's Frequency row while its Pitch input is connected. */
export function PitchRow({ from }: { from: string }) {
  const label = useNodeLabel();
  const nodes = useDocNodes();
  return (
    <div className="param modulated proto-pitch-row">
      <div className="param-top">
        <span>Pitch</span>
        <span className="value">{range(playable(nodes.get(from)))}</span>
      </div>
      <p className="param-note proto-gate-note">from {label(nodes.get(from), from)} · overrides Frequency</p>
    </div>
  );
}

// ---------------------------------------------------------------- piano roll

const ROWS = 13;

/** One note per column. `mask` dims inactive steps; clicking one activates it. */
export function PianoRoll({ notes, mask, onSet, labels }: {
  notes: number[];
  mask?: number[];
  onSet: (column: number, note: number) => void;
  labels?: (column: number) => string;
}) {
  const active = notes.filter((_, index) => !mask || mask[index]);
  // Centre the 13-row window on the notes in use; ▲/▼ move it by an octave.
  const [base, setBase] = useState(() => {
    const used = active.length ? active : [48];
    const low = Math.min(...used), high = Math.max(...used);
    return Math.max(0, low - Math.max(0, Math.floor((ROWS - 1 - (high - low)) / 2)));
  });
  const rows = Array.from({ length: ROWS }, (_, row) => base + ROWS - 1 - row);
  return (
    <div className="proto-roll">
      <div className="proto-roll-keys">
        <button type="button" aria-label="Octave up" onClick={() => setBase(Math.min(108, base + 12))}>▲</button>
        {rows.map((note) => (
          <span key={note} className={`${noteName(note).includes("#") ? "black" : ""}${note % 12 === 0 ? " c" : ""}`}>
            {note % 12 === 0 || note === rows[0] || note === rows[ROWS - 1] ? noteName(note) : ""}
          </span>
        ))}
        <button type="button" aria-label="Octave down" onClick={() => setBase(Math.max(0, base - 12))}>▼</button>
      </div>
      <div className="proto-roll-grid" style={{ gridTemplateColumns: `repeat(${notes.length}, 1fr)` }} role="grid" aria-label="Notes">
        {rows.map((note) => notes.map((current, column) => (
          <button key={`${note}-${column}`} type="button"
            className={`proto-roll-cell${current === note ? (mask && !mask[column] ? " held" : " on") : ""}${noteName(note).includes("#") ? " black" : ""}${column % 4 === 0 ? " downbeat" : ""}`}
            aria-label={`${labels?.(column) ?? `Step ${column + 1}`}: ${noteName(note)}`}
            onClick={() => onSet(column, note)} />
        )))}
      </div>
    </div>
  );
}
