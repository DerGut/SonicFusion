import { useRef, useState } from "react";

import type { ParamDescriptor } from "../api/types";
import { formatCents, formatPitch, hzToMidi, noteName, parseFrequency, pitchFromHz, pitchToHz } from "../model/pitch";
import { formatHz } from "../model/signal";

const STEPS = 1000;
/** Keys that move a range input's value. */
const VALUE_KEYS = new Set(["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "PageUp", "PageDown", "Home", "End"]);

export function toSlider(descriptor: ParamDescriptor, value: number): number {
  const { min, max, scale } = descriptor;
  const clamped = Math.min(max, Math.max(min, value));
  const fraction = scale === "log" ? Math.log(clamped / min) / Math.log(max / min) : (clamped - min) / (max - min);
  return fraction * STEPS;
}

export function fromSlider(descriptor: ParamDescriptor, position: number): number {
  const { min, max, scale } = descriptor;
  const fraction = position / STEPS;
  const value = scale === "log" ? min * Math.pow(max / min, fraction) : min + (max - min) * fraction;
  return Number(value.toPrecision(4));
}

export function formatValue(descriptor: ParamDescriptor, value: number): string {
  if (descriptor.unit === "Hz") return formatHz(value);
  // PROTOTYPE #34/#39
  if (descriptor.unit === "s") return value < 1 ? `${Number((value * 1000).toPrecision(3))} ms` : `${Number(value.toPrecision(3))} s`;
  if (descriptor.unit === "%") return `${Math.round(value * 100)}% wet`;
  return value.toFixed(2);
}

export type FrequencyMode = "hz" | "note";

export function FrequencyModeSelect({ label, mode, onChange }: {
  label: string;
  mode: FrequencyMode;
  onChange: (mode: FrequencyMode) => void;
}) {
  return (
    <select className="frequency-mode nodrag" aria-label={label + " display"} value={mode}
      onChange={(event) => onChange(event.target.value === "note" ? "note" : "hz")}>
      <option value="hz">Hz</option>
      <option value="note">Note</option>
    </select>
  );
}

interface Props {
  id: string;
  descriptor: ParamDescriptor;
  label?: string;
  value: number;
  invalid?: string;
  frequency?: {
    mode: FrequencyMode;
    onModeChange: (mode: FrequencyMode) => void;
    semitones: boolean;
  };
  /** `continuous` while dragging, so the edit history records one step. */
  onChange: (value: number, continuous: boolean) => void;
  onBeginContinuous: () => void;
}

/** Label, value (click to type) and slider for one numeric parameter. */
export function ParamControl({ id, descriptor, label = descriptor.label, value, invalid, frequency,
  onChange, onBeginContinuous }: Props) {
  const [typing, setTyping] = useState<string | null>(null);
  const [referenceNote, setReferenceNote] = useState<number>();
  const noteMode = frequency?.mode === "note";
  const pitch = noteMode ? pitchFromHz(value, referenceNote) : null;
  const noteSlider = noteMode && frequency.semitones;
  const oscillatorPitch = noteSlider ? pitch : null;
  const minNote = noteSlider ? Math.ceil(hzToMidi(descriptor.min)) : 0;
  const readout = oscillatorPitch ? noteName(oscillatorPitch.midi) : pitch ? formatPitch(pitch) : formatValue(descriptor, value);
  const editValue = pitch ? formatPitch(pitch) : String(value);
  const commit = () => {
    if (typing !== null && typing.trim() !== "" && typing !== editValue) {
      const parsed = frequency ? parseFrequency(typing) : Number(typing);
      if (Number.isFinite(parsed) && parsed !== value) {
        setReferenceNote(undefined);
        onChange(parsed, false);
      }
    }
    setTyping(null);
  };
  const setPitch = (midi: number, cents: number) => {
    setReferenceNote(midi);
    onChange(pitchToHz({ midi, cents }), true);
  };

  return (
    <>
      <div className={`param${invalid ? " invalid" : ""}`} title={invalid}>
        <div className="param-top">
          <div className="param-label">
            <label htmlFor={id}>{label}</label>
            {frequency && <FrequencyModeSelect label={label} mode={frequency.mode} onChange={frequency.onModeChange} />}
          </div>
          {typing === null ? (
            <button type="button" className="value nodrag"
              title={frequency ? "Click to type a frequency or note (C#4)" : "Click to type a value"}
              aria-label={label + ": " + readout + ". Type a value"}
              onClick={() => setTyping(editValue)}>
              {readout}
            </button>
          ) : (
            <input className="value-input nodrag" autoFocus aria-label={label + " value"}
              onFocus={(event) => event.target.select()}
              value={typing} onChange={(event) => setTyping(event.target.value)} onBlur={commit}
              onKeyDown={(event) => {
                if (event.key === "Enter") commit();
                if (event.key === "Escape") setTyping(null);
              }} />
          )}
        </div>
        {noteSlider ? (
          <ParameterRange id={id} min={minNote} max={Math.floor(hzToMidi(descriptor.max))}
            step={1} position={oscillatorPitch?.midi ?? minNote} valueText={readout} onBeginContinuous={onBeginContinuous}
            onMove={(midi) => setPitch(midi, oscillatorPitch?.cents ?? 0)} />
        ) : (
          <ParameterRange id={id} min={0} max={STEPS} step="any" position={toSlider(descriptor, value)} valueText={readout}
            onBeginContinuous={onBeginContinuous} onMove={(position) => onChange(fromSlider(descriptor, position), true)} />
        )}
      </div>
      {oscillatorPitch && (
        <div className={`param${invalid ? " invalid" : ""}`}>
          <div className="param-top">
            <label htmlFor={id + "-detune"}>Detune</label>
            <span className="value">{formatCents(oscillatorPitch.cents)}</span>
          </div>
          <ParameterRange id={id + "-detune"} min={-50} max={50} step={0.1} position={oscillatorPitch.cents}
            valueText={formatCents(oscillatorPitch.cents)} onBeginContinuous={onBeginContinuous}
            onMove={(cents) => setPitch(oscillatorPitch.midi, cents)} />
        </div>
      )}
    </>
  );
}

export function ParameterRange({ id, min, max, step, position, valueText, onBeginContinuous, onMove }: {
  id: string;
  min: number;
  max: number;
  step: number | "any";
  position: number;
  valueText: string;
  onBeginContinuous: () => void;
  onMove: (position: number, event: { altKey: boolean }) => void;
}) {
  const clamped = Math.min(max, Math.max(min, position));
  // PROTOTYPE #33: a range input's change event carries no modifier keys.
  const alt = useRef(false);
  const percent = 100 * ((clamped - min) / (max - min));
  const background = `linear-gradient(90deg, var(--fill) 0 ${percent}%, var(--track) ${percent}% 100%)`;
  return (
    <input id={id} className="range nodrag" type="range" min={min} max={max} step={step} value={clamped}
      style={{ background }} aria-valuetext={valueText}
      onPointerDown={(event) => { alt.current = event.altKey; onBeginContinuous(); }}
      onPointerMove={(event) => { alt.current = event.altKey; }}
      onKeyDown={(event) => { alt.current = event.altKey; if (VALUE_KEYS.has(event.key)) onBeginContinuous(); }}
      onChange={(event) => onMove(Number(event.target.value), { altKey: alt.current })} />
  );
}
