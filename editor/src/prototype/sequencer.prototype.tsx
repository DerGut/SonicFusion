/**
 * PROTOTYPE — throwaway. Three designs for a one-bar step sequencer node.
 *
 * `?seq=` A (wide node, inline step grid) · B (compact node, pattern edited in the panel)
 *         C (Euclidean ring: hits and rotation generate the steps, dots toggle)
 * `?scenario=seq` seeds sequencer → envelope gate, plus an unconnected Clock.
 * `?scenario=seq-clocked` connects the Clock to the sequencer.
 *
 * Semantics: one bar of 16 steps. With a clock, each rising edge (≤ 0 → > 0)
 * advances one step; without one, the internal tempo runs four steps per beat.
 * The output is a 0/1 gate that stays open for `gate` of each active step.
 */
import { useEffect, useRef, useState } from "react";

import type { NodeDescriptor } from "../api/types";
import type { DocNode, EditorDocument } from "../model/document";
import { ParameterRange } from "../components/ParamControl";
import { useDocNodes, useNodeLabel, useVariant } from "./design-33-41.prototype";
import { NotesRow, PianoRoll, useNotesToggle, useSequencerNotes } from "./notes.prototype";

export const STEPS = 16;
const BEAT_LABELS = ["1", "e", "+", "a"];
const TRESILLO = [1, 0, 0, 1, 0, 0, 1, 0, 0, 0, 1, 0, 1, 0, 0, 0];

/** Evenly spreads `hits` over the bar, then rotates. */
export function euclid(hits: number, rotate: number, steps = STEPS): number[] {
  const base = Array.from({ length: steps }, (_, index) => ((index * hits) % steps < hits ? 1 : 0));
  return base.map((_, index) => base[(index - rotate + steps * 4) % steps]);
}

export function sequencerScenario(doc: EditorDocument, clocked: boolean, variant: string): EditorDocument {
  const node = (id: string, kind: DocNode["kind"], params: DocNode["params"], x: number, y: number, displayName?: string): DocNode =>
    ({ id, kind, params, position: { x, y }, ...(displayName ? { displayName } : {}) });
  const steps = variant === "C" ? euclid(5, 0) : TRESILLO;
  const seqX = 32;
  const envX = variant === "A" ? 472 : 272;
  return {
    ...doc,
    name: "seq-sketch",
    nodes: [
      ...(clocked ? [node("clock", "square", { frequency_hz: 8, pulse_width: 0.5 }, seqX, 40, "Clock")] : []),
      node("seq", "sequencer", { tempo_bpm: 120, gate: 0.5, steps, hits: 5, rotate: 0 }, seqX, 250, "Rhythm"),
      node("bass", "sawtooth", { frequency_hz: 55 }, envX, 90, "Bass"),
      node("env", "envelope", { attack_s: 0.005, decay_s: 0.12, sustain: 0.4, release_s: 0.08 }, envX + 240, 90),
      node("out", "output", {}, envX + 480, 90),
    ],
    wires: [
      ...(clocked ? [{ from: "clock", to: { node: "seq", port: "clock" } }] : []),
      { from: "seq", to: { node: "env", port: "gate" } },
      { from: "bass", to: { node: "env", port: "in" } },
      { from: "env", to: { node: "out", port: "in" } },
    ],
  };
}

interface BodyProps {
  node: DocNode;
  descriptor: NodeDescriptor;
  clockFrom: string | undefined;
  clockHandle: (top: number) => React.ReactNode;
  pitchHandle: (top: number) => React.ReactNode;
  rowTop: (index: number) => number;
  setParam: (name: string, value: number | number[], continuous: boolean) => void;
  beginContinuous: () => void;
}

export function useSequencerWide(node: DocNode): boolean {
  return useVariant("seq") === "A" && node.kind === "sequencer";
}

/** The clock row comes first so its port sits at a fixed height. */
export function SequencerBody(props: BodyProps & { notes: boolean }): { rows: React.ReactNode[]; handles: React.ReactNode[] } {
  // Rows: Clock (0), Pattern (1), Notes (2) — the pitch output sits on the Notes row.
  return {
    rows: [<ClockRow key="clock" {...props} />, <SequencerControls key="controls" {...props} />],
    handles: [props.clockHandle(props.rowTop(0)), ...(props.notes ? [props.pitchHandle(props.rowTop(2))] : [])],
  };
}

const steps = (node: DocNode) => {
  const value = node.params.steps;
  return Array.isArray(value) && value.length === STEPS ? value : TRESILLO;
};

function ClockRow({ node, clockFrom, setParam, beginContinuous }: BodyProps) {
  const label = useNodeLabel();
  const nodes = useDocNodes();
  const bpm = Number(node.params.tempo_bpm ?? 120);
  return (
    <div className="param proto-clock">
      <div className="param-top">
        <span className="proto-gate-label">Clock</span>
        <span className="value">{clockFrom ? "1 step / edge" : `internal · ${bpm} BPM`}</span>
      </div>
      {clockFrom ? (
        <p className="param-note proto-gate-note">from {label(nodes.get(clockFrom), clockFrom)} · tempo follows it</p>
      ) : (
        <ParameterRange id={`${node.id}-tempo`} min={20} max={300} step={1} position={bpm} valueText={`${bpm} BPM`}
          onBeginContinuous={beginContinuous} onMove={(value) => setParam("tempo_bpm", value, true)} />
      )}
    </div>
  );
}

function SequencerControls(props: BodyProps) {
  const variant = useVariant("seq");
  if (variant === "B") return <CompactPattern {...props} />;
  if (variant === "C") return <EuclidRing {...props} />;
  return <InlineGrid {...props} />;
}

function GateLength({ node, setParam, beginContinuous }: BodyProps) {
  const gate = Number(node.params.gate ?? 0.5);
  return (
    <div className="param">
      <div className="param-top">
        <label htmlFor={`${node.id}-gate`}>Gate length</label>
        <span className="value">{Math.round(gate * 100)}% of a step</span>
      </div>
      <ParameterRange id={`${node.id}-gate`} min={0.05} max={1} step={0.05} position={gate} valueText={`${Math.round(gate * 100)}%`}
        onBeginContinuous={beginContinuous} onMove={(value) => setParam("gate", Number(value.toFixed(2)), true)} />
    </div>
  );
}

/**
 * Click a step to toggle it; drag across steps to paint the same state. Painting
 * hit-tests on pointer move, so it also works with touch's implicit capture.
 */
let paintGroups = 0;
function usePaint(pattern: number[], write: (next: number[]) => void) {
  const paint = useRef<number | null>(null);
  const latest = useRef(pattern);
  latest.current = pattern;
  const [group] = useState(() => `seq-${++paintGroups}`);
  const set = (index: number, value: number) => {
    if (latest.current[index] === value) return;
    const next = [...latest.current];
    next[index] = value;
    // Keep later paints in the same drag from overwriting this one before React re-renders.
    latest.current = next;
    write(next);
  };
  const setRef = useRef(set);
  setRef.current = set;
  useEffect(() => {
    const move = (event: PointerEvent) => {
      if (paint.current === null) return;
      const cell = document.elementFromPoint(event.clientX, event.clientY)?.closest<HTMLElement | SVGElement>(`[data-paint="${group}"]`);
      if (cell) setRef.current(Number(cell.dataset.step), paint.current);
    };
    const stop = () => { paint.current = null; };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", stop);
    return () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", stop);
    };
  }, [group]);
  return (index: number) => ({
    "data-paint": group,
    "data-step": index,
    onPointerDown: (event: React.PointerEvent) => {
      event.stopPropagation();
      event.preventDefault();
      paint.current = latest.current[index] ? 0 : 1;
      set(index, paint.current);
    },
    onKeyDown: (event: React.KeyboardEvent) => {
      if (event.key === " " || event.key === "Enter") { event.preventDefault(); set(index, latest.current[index] ? 0 : 1); }
    },
  });
}

// ---------------------------------------------------------------- A

function InlineGrid(props: BodyProps) {
  const { node, setParam } = props;
  const pattern = steps(node);
  const handlers = usePaint(pattern, (next) => setParam("steps", next, false));
  const gate = Number(node.params.gate ?? 0.5);
  return (
    <>
      <div className="proto-grid-row nodrag">
        <div className="proto-grid-beats">
          {[1, 2, 3, 4].map((beat) => <span key={beat}>{beat}</span>)}
        </div>
        <div className="proto-grid" role="group" aria-label="Steps">
          {pattern.map((on, index) => (
            <button key={index} type="button" className={`proto-step${on ? " on" : ""}${index % 4 === 0 ? " downbeat" : ""}`}
              aria-pressed={!!on} aria-label={`Step ${index + 1} (${Math.floor(index / 4) + 1} ${BEAT_LABELS[index % 4]})`}
              {...handlers(index)}>
              {on ? <span className="proto-step-gate" style={{ width: `${gate * 100}%` }} /> : null}
            </button>
          ))}
        </div>
      </div>
      <GateLength {...props} />
    </>
  );
}

// ---------------------------------------------------------------- B

function CompactPattern(props: BodyProps) {
  const pattern = steps(props.node);
  const openPanel = usePatternPanel();
  const notesVariant = useVariant("mel");
  const notesDesign = new URLSearchParams(window.location.search).get("scenario") === "mel" && notesVariant !== "C";
  return (
    <>
      <div className="param proto-mini-row">
        <div className="param-top">
          <span>Pattern</span>
          <button type="button" className="value nodrag proto-link" onClick={() => openPanel(props.node.id)}>
            {pattern.filter(Boolean).length} of {STEPS} · edit
          </button>
        </div>
        <div className="proto-mini" aria-hidden="true" onDoubleClick={() => openPanel(props.node.id)}>
          {pattern.map((on, index) => <span key={index} className={`${on ? "on" : ""}${index % 4 === 0 ? " downbeat" : ""}`} />)}
        </div>
      </div>
      {notesDesign && <NotesRow node={props.node} setParam={props.setParam} />}
      <GateLength {...props} />
    </>
  );
}

/** B opens the bottom panel on its Pattern tab. */
let openListener: ((id: string) => void) | null = null;
export function onOpenPattern(listener: (id: string) => void) { openListener = listener; }
function usePatternPanel() {
  return openPattern;
}
export function openPattern(id: string) { openListener?.(id); }

/** The large editor shown in the panel for variant B. */
export function PatternEditor({ node, onSteps, onParam }: {
  node: DocNode;
  onSteps: (next: number[]) => void;
  onParam: (name: string, value: number | number[]) => void;
}) {
  const pattern = steps(node);
  const handlers = usePaint(pattern, onSteps);
  const gate = Number(node.params.gate ?? 0.5);
  const notesOn = useSequencerNotes(node);
  const toggle = useNotesToggle();
  const notes = Array.isArray(node.params.notes) ? (node.params.notes as number[]) : pattern.map(() => 48);
  return (
    <div className={`proto-pattern${notesOn ? " with-notes" : ""}`}>
      <div className="proto-pattern-grid" role="group" aria-label="Steps">
        {pattern.map((on, index) => (
          <button key={index} type="button" className={`proto-cell${on ? " on" : ""}${index % 4 === 0 ? " downbeat" : ""}`}
            aria-pressed={!!on} aria-label={`Step ${index + 1}`} {...handlers(index)}>
            <span className="proto-cell-label">{index % 4 === 0 ? Math.floor(index / 4) + 1 : BEAT_LABELS[index % 4]}</span>
            {on ? <span className="proto-cell-gate" style={{ width: `${gate * 100}%` }} /> : null}
          </button>
        ))}
      </div>
      {notesOn && (
        <PianoRoll notes={notes} mask={pattern} onSet={(column, note) => {
          const next = [...notes];
          next[column] = note;
          onParam("notes", next);
          if (!pattern[column]) onSteps(pattern.map((on, index) => (index === column ? 1 : on)));
        }} />
      )}
      <div className="proto-pattern-tools">
        <button type="button" onClick={() => onSteps(pattern.map(() => 0))}>Clear</button>
        <button type="button" onClick={() => onSteps([...pattern.slice(-1), ...pattern.slice(0, -1)])}>Shift →</button>
        <button type="button" onClick={() => onSteps([...pattern.slice(1), pattern[0]])}>← Shift</button>
        <button type="button" onClick={() => onSteps(pattern.map((on) => (on ? 0 : 1)))}>Invert</button>
        {toggle && (
          <label className="proto-inline-toggle">
            <input type="checkbox" checked={notesOn} onChange={() => onParam("notes_on", notesOn ? 0 : 1)} /> Notes + pitch output
          </label>
        )}
        <span className="hint">Click or drag to paint · Space toggles the focused step</span>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------- C

function EuclidRing(props: BodyProps) {
  const { node, setParam, beginContinuous } = props;
  const pattern = steps(node);
  const hits = Number(node.params.hits ?? 5);
  const rotate = Number(node.params.rotate ?? 0);
  const generated = euclid(hits, rotate);
  const custom = generated.some((on, index) => on !== pattern[index]);
  const handlers = usePaint(pattern, (next) => setParam("steps", next, false));
  const regenerate = (nextHits: number, nextRotate: number) => {
    setParam("hits", nextHits, true);
    setParam("rotate", nextRotate, true);
    setParam("steps", euclid(nextHits, nextRotate), true);
  };
  const size = 112;
  const radius = 44;
  return (
    <>
      <div className="proto-ring-row nodrag">
        <svg width={size} height={size} viewBox={`0 0 ${size} ${size}`} role="group" aria-label="Steps">
          <circle cx={size / 2} cy={size / 2} r={radius} className="proto-ring-track" />
          {pattern.map((on, index) => {
            const angle = (index / STEPS) * 2 * Math.PI - Math.PI / 2;
            const x = size / 2 + radius * Math.cos(angle);
            const y = size / 2 + radius * Math.sin(angle);
            return (
              <circle key={index} cx={x} cy={y} r={index % 4 === 0 ? 6 : 5} tabIndex={0} role="button" aria-pressed={!!on}
                aria-label={`Step ${index + 1}`} className={`proto-ring-dot${on ? " on" : ""}${index === 0 ? " first" : ""}`} {...handlers(index)} />
            );
          })}
          <text x={size / 2} y={size / 2 - 2} className="proto-ring-count">{pattern.filter(Boolean).length}/{STEPS}</text>
          <text x={size / 2} y={size / 2 + 12} className="proto-ring-mode">{custom ? "edited" : "euclidean"}</text>
        </svg>
      </div>
      <div className="proto-pair">
        <div className="proto-compact">
          <div className="param-top"><label htmlFor={`${node.id}-hits`}>Hits</label><span className="value">{hits}</span></div>
          <ParameterRange id={`${node.id}-hits`} min={0} max={STEPS} step={1} position={hits} valueText={`${hits} hits`}
            onBeginContinuous={beginContinuous} onMove={(value) => regenerate(value, rotate)} />
        </div>
        <div className="proto-compact">
          <div className="param-top"><label htmlFor={`${node.id}-rotate`}>Rotate</label><span className="value">{rotate}</span></div>
          <ParameterRange id={`${node.id}-rotate`} min={0} max={STEPS - 1} step={1} position={rotate} valueText={`rotated ${rotate}`}
            onBeginContinuous={beginContinuous} onMove={(value) => regenerate(hits, value)} />
        </div>
      </div>
      <GateLength {...props} />
    </>
  );
}

// ---------------------------------------------------------------- panel tab

export function usePatternTab(node: DocNode): [boolean, (open: boolean) => void] {
  const variant = useVariant("seq");
  const [open, setOpen] = useState(variant === "B" && node.kind === "sequencer");
  useEffect(() => setOpen(variant === "B" && node.kind === "sequencer"), [variant, node.id, node.kind]);
  return [variant === "B" && node.kind === "sequencer" && open, setOpen];
}
