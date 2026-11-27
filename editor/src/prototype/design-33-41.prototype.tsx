/**
 * PROTOTYPE — throwaway. Design variants for #33 (frequencies as notes) and
 * #41 (node display names), mounted on the real editor page.
 *
 *   npm --prefix editor run dev  →  http://localhost:5173/?scenario=edge&pitch=B&name=B
 *
 * `?pitch=` A (current) · B (patch-wide unit, dual readout) · C (keyboard popover)
 *           D (per-node unit, default from role, dual readout + popover)
 * `?name=`  A (current) · B (in-place rename, ID chip) · C (stacked ID, rename in panel)
 * `?scenario=edge` seeds duplicate, long and named nodes plus an off-semitone oscillator.
 *
 * Pitch params are faked as "frequency_hz" / "cutoff_hz" at ≥ 20 Hz; the issue
 * proposes a catalog flag instead.
 */
import { createContext, useContext, useEffect, useRef, useState, useSyncExternalStore } from "react";

import type { ParamDescriptor } from "../api/types";
import type { DocNode, EditorDocument } from "../model/document";
import { formatCents, formatPitch, hzToMidi, noteName, parseFrequency, pitchFromHz, pitchToHz } from "../model/pitch";
import { formatHz } from "../model/signal";
import { Icon } from "../components/icons";
import { NodeTitle } from "../components/NodeTitle";
import { ParameterRange, fromSlider, toSlider } from "../components/ParamControl";

// ---------------------------------------------------------------- URL state

type Key = "pitch" | "name";
const VARIANTS: Record<Key, { id: string; label: string }[]> = {
  pitch: [
    { id: "A", label: "Current: per-param Hz/Note select" },
    { id: "B", label: "Patch-wide unit, dual readout" },
    { id: "C", label: "Both units, keyboard popover" },
    { id: "D", label: "Per-node unit, role default" },
  ],
  name: [
    { id: "A", label: "Current: pencil button" },
    { id: "B", label: "Double-click title, ID chip" },
    { id: "C", label: "Stacked ID, rename in panel" },
  ],
};
const listeners = new Set<() => void>();
const subscribe = (listener: () => void) => {
  listeners.add(listener);
  return () => listeners.delete(listener);
};
const search = () => window.location.search;

export function useVariant(key: Key): string {
  const query = useSyncExternalStore(subscribe, search);
  return new URLSearchParams(query).get(key) ?? "A";
}

function setVariant(key: Key, id: string) {
  const params = new URLSearchParams(window.location.search);
  params.set(key, id);
  window.history.replaceState(null, "", "?" + params.toString());
  listeners.forEach((listener) => listener());
}

/** Seed the cases that tell the designs apart. */
export function edgeScenario(doc: EditorDocument): EditorDocument {
  if (new URLSearchParams(window.location.search).get("scenario") !== "edge") return doc;
  const names: Record<string, string> = { osc_a: "Osc", osc_b: "Osc", lfo: "LFO", gain: "Master gain before the speaker output" };
  return {
    ...doc,
    nodes: doc.nodes.map((node) => ({
      ...node,
      ...(names[node.id] ? { displayName: names[node.id] } : {}),
      ...(node.id === "osc_a" ? { params: { ...node.params, frequency_hz: 225 } } : {}),
    })),
  };
}

// ---------------------------------------------------------------- switcher

export function PrototypeSwitcher() {
  const pitch = useVariant("pitch");
  const name = useVariant("name");
  if (!import.meta.env.DEV) return null;
  const group = (key: Key, current: string, issue: string) => {
    const list = VARIANTS[key];
    const index = Math.max(0, list.findIndex((variant) => variant.id === current));
    const step = (delta: number) => setVariant(key, list[(index + delta + list.length) % list.length].id);
    return (
      <div className="proto-group">
        <span className="proto-issue">{issue}</span>
        <button type="button" aria-label={`Previous ${key} variant`} onClick={() => step(-1)}>‹</button>
        <span className="proto-label"><b>{list[index].id}</b> {list[index].label}</span>
        <button type="button" aria-label={`Next ${key} variant`} onClick={() => step(1)}>›</button>
      </div>
    );
  };
  return (
    <div className="proto-switcher" role="toolbar" aria-label="Prototype variants">
      {group("pitch", pitch, "#33 pitch")}
      {group("name", name, "#41 name")}
    </div>
  );
}

// ---------------------------------------------------------------- #41 names

const DocNodesContext = createContext<Map<string, DocNode>>(new Map());
export const DocNodesProvider = DocNodesContext.Provider;
export const useDocNodes = () => useContext(DocNodesContext);

/** How a node is referred to outside its own header. */
export function useNodeLabel(): (node: DocNode | undefined, id: string) => string {
  const variant = useVariant("name");
  return (node, id) => {
    const name = node?.displayName?.trim();
    if (variant === "A" || !name) return id;
    return variant === "B" ? `${name} (${id})` : name;
  };
}

export function PrototypeTitle({ node, kind, onRename }: {
  node: DocNode;
  kind: string;
  onRename: (id: string, name: string) => void;
}) {
  const variant = useVariant("name");
  const focusPanel = useContext(RenameFocusContext);
  if (variant === "A") {
    return (
      <>
        <NodeTitle node={node} onRename={onRename} />
        <span className="sf-node-kind">{kind}</span>
      </>
    );
  }
  const name = node.displayName?.trim();
  if (variant === "B") return <InPlaceTitle node={node} kind={kind} onRename={onRename} />;
  return (
    <>
      <span className="proto-stack" title={name ? `${name} — id ${node.id}` : node.id}
        onDoubleClick={() => focusPanel(node.id)}>
        <span className="proto-stack-name">{name || node.id}</span>
        {name && <span className="proto-stack-id">{node.id}</span>}
      </span>
      <span className="sf-node-kind">{kind}</span>
    </>
  );
}

function InPlaceTitle({ node, kind, onRename }: { node: DocNode; kind: string; onRename: (id: string, name: string) => void }) {
  const [editing, setEditing] = useState(false);
  const name = node.displayName?.trim();
  if (editing) {
    return (
      <input className="sf-node-title sf-node-title-input nodrag" autoFocus aria-label="Node display name"
        defaultValue={name ?? ""} placeholder={node.id}
        onFocus={(event) => event.currentTarget.select()}
        onBlur={(event) => { onRename(node.id, event.currentTarget.value); setEditing(false); }}
        onKeyDown={(event) => {
          event.stopPropagation();
          if (event.key === "Enter") event.currentTarget.blur();
          if (event.key === "Escape") setEditing(false);
        }} />
    );
  }
  return (
    <>
      <span className={`sf-node-title${name ? "" : " proto-unnamed"}`} title={`${name ?? node.id} — double-click to rename`}
        onDoubleClick={() => setEditing(true)}>
        {name || node.id}
      </span>
      {name && <span className="proto-id-chip" title="Stable node ID, used in wires and export">{node.id}</span>}
      <span className="sf-node-kind">{kind}</span>
    </>
  );
}

/** Variant C renames in the bottom panel; a double-click on the canvas title focuses that field. */
const RenameFocusContext = createContext<(id: string) => void>(() => {});
export const RenameFocusProvider = RenameFocusContext.Provider;

export function PanelRename({ node, focusToken, onRename }: {
  node: DocNode;
  focusToken: number;
  onRename: (id: string, name: string) => void;
}) {
  const variant = useVariant("name");
  const input = useRef<HTMLInputElement>(null);
  const [text, setText] = useState(node.displayName ?? "");
  useEffect(() => setText(node.displayName ?? ""), [node.displayName]);
  useEffect(() => {
    if (focusToken) input.current?.select();
  }, [focusToken]);
  if (variant !== "C") return null;
  return (
    <span className="proto-panel-rename">
      <input ref={input} aria-label="Node display name" value={text} placeholder={`Name (${node.id})`}
        onChange={(event) => setText(event.target.value)}
        onBlur={() => onRename(node.id, text)}
        onKeyDown={(event) => {
          if (event.key === "Enter") event.currentTarget.blur();
          if (event.key === "Escape") { setText(node.displayName ?? ""); event.currentTarget.blur(); }
        }} />
      <code>id {node.id}</code>
    </span>
  );
}

// ---------------------------------------------------------------- #33 pitch

/** B: one unit for the whole patch; would be saved with the project or as a preference. */
const unitListeners = new Set<() => void>();
let patchUnit: "hz" | "note" = "note";
const subscribeUnit = (listener: () => void) => {
  unitListeners.add(listener);
  return () => unitListeners.delete(listener);
};
export function usePatchUnit(): ["hz" | "note", (unit: "hz" | "note") => void] {
  const unit = useSyncExternalStore(subscribeUnit, () => patchUnit);
  return [unit, (next) => { patchUnit = next; unitListeners.forEach((listener) => listener()); }];
}

export function PatchUnitToggle() {
  const variant = useVariant("pitch");
  const [unit, setUnit] = usePatchUnit();
  if (variant !== "B") return null;
  return (
    <div className="proto-segmented" role="radiogroup" aria-label="Pitch display">
      {(["hz", "note"] as const).map((option) => (
        <button key={option} type="button" role="radio" aria-checked={unit === option} onClick={() => setUnit(option)}>
          {option === "hz" ? "Hz" : "♪ Note"}
        </button>
      ))}
    </div>
  );
}

const PITCH_FLOOR_HZ = 20;
const isPitch = (hz: number) => hz >= PITCH_FLOOR_HZ;

/** Modulated cutoff readout. */
export function useModulatedReadout(): ((baseHz: number, depthHz: number, current: string) => string) {
  const variant = useVariant("pitch");
  const [unit] = usePatchUnit();
  return (base, depth, current) => {
    if (variant === "A") return current;
    const range = `${noteName(pitchFromHz(base - depth)!.midi)}–${noteName(pitchFromHz(base + depth)!.midi)}`;
    const hz = `${formatHz(base)} ± ${Math.round(depth)} Hz`;
    if (variant === "B") return unit === "note" ? range : hz;
    if (variant === "D") return hz;
    return `${hz} · ${range}`;
  };
}

/**
 * D: the unit belongs to the node. It defaults from the node's role and is
 * stored only when the user overrides it (in the real version, on DocNode
 * next to `displayName`; here in memory).
 */
type Unit = "hz" | "note";
const unitOverrides = new Map<string, Unit>();
const overrideListeners = new Set<() => void>();
let overrideVersion = 0;
const subscribeOverrides = (listener: () => void) => {
  overrideListeners.add(listener);
  return () => overrideListeners.delete(listener);
};
function setOverride(key: string, unit: Unit, fallback: Unit) {
  if (unit === fallback) unitOverrides.delete(key);
  else unitOverrides.set(key, unit);
  overrideVersion += 1;
  overrideListeners.forEach((listener) => listener());
}
function useUnitOverride(key: string): Unit | undefined {
  useSyncExternalStore(subscribeOverrides, () => overrideVersion);
  return unitOverrides.get(key);
}
if (new URLSearchParams(window.location.search).get("scenario") === "edge") unitOverrides.set("osc_b:frequency_hz", "hz");

/** Replaces ParamControl for frequency params in variants B–D. Returns null for A. */
export function PrototypeFrequency({ id, nodeId, modulator, descriptor, value, invalid, onChange, onBeginContinuous }: {
  id: string;
  nodeId: string;
  /** The node feeds a control port, so its frequency is a rate. */
  modulator: boolean;
  descriptor: ParamDescriptor;
  value: number;
  invalid?: string;
  onChange: (value: number, continuous: boolean) => void;
  onBeginContinuous: () => void;
}) {
  const variant = useVariant("pitch");
  const [unit] = usePatchUnit();
  const [typing, setTyping] = useState<string | null>(null);
  const [open, setOpen] = useState(false);
  const overrideKey = `${nodeId}:${descriptor.name}`;
  const override = useUnitOverride(overrideKey);
  if (variant === "A") return null;
  const pitch = isPitch(value) ? pitchFromHz(value) : null;
  const roleUnit: Unit = modulator || !pitch ? "hz" : "note";
  const nodeUnit = override ?? roleUnit;
  const notePrimary = variant === "D" ? nodeUnit === "note" && pitch !== null : variant === "C" || unit === "note";

  const primary = pitch && notePrimary ? formatPitch(pitch) : formatHz(value);
  const rate = !pitch;
  const secondary = variant === "D" && rate ? "rate"
    : pitch ? (notePrimary ? formatHz(value) : formatPitch(pitch)) : variant === "B" && unit === "note" ? "rate" : null;
  const commit = () => {
    if (typing !== null && typing.trim() !== "") {
      const parsed = parseFrequency(typing);
      if (Number.isFinite(parsed) && parsed > 0) onChange(parsed, false);
    }
    setTyping(null);
  };
  // B in Note mode and D on note-led nodes snap the drag to semitones; ⌥ drags freely.
  const snap = (variant === "B" || variant === "D") && notePrimary && pitch !== null;
  const popover = variant === "C" || (variant === "D" && notePrimary);
  const move = (position: number, event?: { altKey: boolean }) => {
    const hz = fromSlider(descriptor, position);
    if (!snap || event?.altKey || !isPitch(hz)) return onChange(hz, true);
    onChange(Number(pitchToHz({ midi: Math.round(hzToMidi(hz)), cents: 0 }).toPrecision(6)), true);
  };

  return (
    <div className={`param proto-freq${invalid ? " invalid" : ""}`} title={invalid}>
      <div className="param-top">
        <label htmlFor={id}>{descriptor.label}</label>
        {typing === null && variant === "D" ? (
          <span className="proto-dual">
            <button type="button" className="value nodrag"
              title={popover ? "Click for keyboard, detune and exact value" : "Click to type Hz or a note (C#4, A3 +12c)"}
              onClick={() => (popover ? setOpen(!open) : setTyping(String(value)))}>
              {primary}
            </button>
            {secondary && (rate ? <span className="proto-secondary">{secondary}</span> : (
              <button type="button" className={`proto-secondary proto-swap nodrag${override ? " overridden" : ""}`}
                title={`Lead with ${notePrimary ? "Hz" : "the note"} on this node${override ? "" : ` (default for ${modulator ? "a modulator" : "an audio source"})`}`}
                onClick={() => { setOpen(false); setOverride(overrideKey, notePrimary ? "hz" : "note", roleUnit); }}>
                {secondary}
              </button>
            ))}
          </span>
        ) : typing === null ? (
          <button type="button" className="value proto-dual nodrag"
            title={variant === "C" ? "Click for keyboard, detune and Hz" : "Click to type Hz or a note (C#4, A3 +12c)"}
            onClick={() => (variant === "C" && pitch ? setOpen(!open) : setTyping(pitch && notePrimary ? formatPitch(pitch) : String(value)))}>
            <span>{primary}</span>
            {secondary && <span className="proto-secondary">{secondary}</span>}
          </button>
        ) : (
          <input className="value-input nodrag" autoFocus value={typing} aria-label={descriptor.label + " value"}
            onFocus={(event) => event.target.select()} onChange={(event) => setTyping(event.target.value)} onBlur={commit}
            onKeyDown={(event) => { if (event.key === "Enter") commit(); if (event.key === "Escape") setTyping(null); }} />
        )}
      </div>
      <ParameterRange id={id} min={0} max={1000} step="any" position={toSlider(descriptor, value)}
        valueText={primary} onBeginContinuous={onBeginContinuous} onMove={move} />
      {open && pitch && (
        <KeyboardPopover descriptor={descriptor} value={value} onClose={() => setOpen(false)}
          onBeginContinuous={onBeginContinuous} onChange={onChange} />
      )}
    </div>
  );
}

const WHITE = [0, 2, 4, 5, 7, 9, 11];
const BLACK: [number, number][] = [[1, 0], [3, 1], [6, 3], [8, 4], [10, 5]];

function KeyboardPopover({ descriptor, value, onChange, onBeginContinuous, onClose }: {
  descriptor: ParamDescriptor;
  value: number;
  onChange: (value: number, continuous: boolean) => void;
  onBeginContinuous: () => void;
  onClose: () => void;
}) {
  const pitch = pitchFromHz(value)!;
  const [octave, setOctave] = useState(Math.floor(pitch.midi / 12) - 1);
  const [hzText, setHzText] = useState<string | null>(null);
  const lowest = Math.ceil(hzToMidi(Math.max(descriptor.min, PITCH_FLOOR_HZ)));
  const highest = Math.floor(hzToMidi(descriptor.max));
  const play = (midi: number) => {
    onBeginContinuous();
    onChange(pitchToHz({ midi, cents: pitch.cents }), false);
  };
  const key = (midi: number, className: string, style?: React.CSSProperties) => (
    <button key={midi} type="button" className={`${className}${midi === pitch.midi ? " on" : ""}`} style={style}
      disabled={midi < lowest || midi > highest} title={noteName(midi)} aria-label={noteName(midi)}
      onClick={() => play(midi)} />
  );
  const base = (octave + 1) * 12;
  return (
    <div className="proto-popover nodrag nowheel" onKeyDown={(event) => event.key === "Escape" && onClose()}>
      <div className="proto-pop-head">
        <button type="button" aria-label="Octave down" onClick={() => setOctave(octave - 1)}>‹</button>
        <span>C{octave} – B{octave + 1}</span>
        <button type="button" aria-label="Octave up" onClick={() => setOctave(octave + 1)}>›</button>
        <span className="spacer" />
        <button type="button" aria-label="Close" onClick={onClose}><Icon name="close" size={12} /></button>
      </div>
      <div className="proto-keys">
        {[0, 1].flatMap((o) => WHITE.map((step) => key(base + o * 12 + step, "white")))}
        {[0, 1].flatMap((o) => BLACK.map(([step, after]) =>
          key(base + o * 12 + step, "black", { left: `calc(${(o * 7 + after + 1) * (100 / 14)}% - 6px)` })))}
      </div>
      <div className="param-top">
        <label>Detune</label>
        <span className="value">{formatCents(pitch.cents)}</span>
      </div>
      <ParameterRange id="proto-detune" min={-50} max={50} step={1} position={pitch.cents} valueText={formatCents(pitch.cents)}
        onBeginContinuous={onBeginContinuous} onMove={(cents) => onChange(pitchToHz({ midi: pitch.midi, cents }), true)} />
      <div className="param-top">
        <label>Exact</label>
        <input className="value-input" value={hzText ?? formatHz(value)} aria-label="Frequency in Hz"
          onFocus={(event) => { setHzText(String(value)); event.target.select(); }}
          onChange={(event) => setHzText(event.target.value)}
          onBlur={() => {
            const parsed = hzText === null ? NaN : parseFrequency(hzText);
            if (Number.isFinite(parsed) && parsed > 0) onChange(parsed, false);
            setHzText(null);
          }}
          onKeyDown={(event) => event.key === "Enter" && event.currentTarget.blur()} />
      </div>
    </div>
  );
}
