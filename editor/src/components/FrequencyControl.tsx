import { useCallback, useRef, useState } from "react";

import type { ParamDescriptor } from "../api/types";
import { defaultFrequencyMode, FREQUENCY_MODES, hzToBpm, isTempoFrequency, noteName, PITCH_FLOOR_HZ, pitchFromHz, pitchToHz, snapSliderHz, snapTempoHz, TEMPO_CEILING_BPM, type FrequencyMode, type FrequencyRole, type Pitch } from "../model/pitch";
import { formatHz } from "../model/signal";
import { FrequencyPopover } from "./FrequencyPopover";
import { fromSlider, SLIDER_STEPS, toSlider } from "./ParamControl";
import { ParameterRange } from "./ParameterRange";

function compactHz(hz: number): string {
  const kilo = hz >= 1_000;
  return Number((kilo ? hz / 1_000 : hz).toPrecision(3)) + (kilo ? "\u202fkHz" : "\u202fHz");
}

function compactBpm(hz: number): string {
  const rounded = Number(hzToBpm(hz).toPrecision(5));
  return (rounded >= TEMPO_CEILING_BPM ? "<" + TEMPO_CEILING_BPM : rounded) + " BPM";
}

function compactPitch({ midi, cents }: Pitch): string {
  const rounded = Math.round(cents);
  const detune = rounded === 0 ? "" : " " + (rounded > 0 ? "+" : "−") + Math.abs(rounded) + "¢";
  return noteName(midi) + detune;
}

export function FrequencyControl({ id, descriptor, value, invalid, sampleRate, role, override, onOverride,
  onChange, onBeginContinuous }: {
  id: string;
  descriptor: ParamDescriptor;
  value: number;
  invalid?: string;
  sampleRate: number;
  role: FrequencyRole;
  override?: FrequencyMode;
  onOverride: (mode?: FrequencyMode) => void;
  onChange: (value: number, continuous: boolean) => void;
  onBeginContinuous: () => void;
}) {
  const anchor = useRef<HTMLButtonElement>(null);
  const [open, setOpen] = useState(false);
  const close = useCallback(() => setOpen(false), []);
  const [reference, setReference] = useState<number>();
  const oscillator = descriptor.frequency === "pitch";
  const defaultMode = defaultFrequencyMode(value, role, oscillator);
  const pitch = value >= PITCH_FLOOR_HZ ? pitchFromHz(value, reference) : null;
  const rhythmic = oscillator && isTempoFrequency(value);
  const overrideAvailable = override === "hz" || (override === "note" && pitch !== null) ||
    (override === "bpm" && rhythmic);
  const preferredMode = overrideAvailable ? override : defaultMode;
  const bpmLed = rhythmic && preferredMode === "bpm";
  const noteLed = pitch !== null && preferredMode === "note";
  const mode = bpmLed ? "bpm" : noteLed ? "note" : "hz";
  const nextMode = noteLed || bpmLed ? "hz" : rhythmic ? "bpm" : "note";
  const nextUnit = FREQUENCY_MODES.find(([unit]) => unit === nextMode)![1];
  const label = bpmLed ? "Tempo" : noteLed && oscillator ? "Pitch" : descriptor.label;
  const primary = bpmLed ? compactBpm(value)
    : noteLed ? compactPitch(pitch)
    : value > 0 && value < 0.01 ? compactHz(value) : formatHz(value);
  const secondary = noteLed || bpmLed ? compactHz(value) : rhythmic ? compactBpm(value)
    : pitch && (Math.abs(pitch.cents) < 0.5 ? "" : "≈") + noteName(pitch.midi);
  // The controls use the catalog's editing range and the device's strict Nyquist limit.
  const max = Math.min(descriptor.max, sampleRate / 2 * (1 - Number.EPSILON));
  const range = { ...descriptor, max };
  const move = (position: number, free: boolean) => {
    const hz = Math.min(max, Math.max(descriptor.min, fromSlider(range, position)));
    setReference(undefined);
    const snapped = free ? hz : noteLed ? snapSliderHz(hz, descriptor.min, max)
      : bpmLed && isTempoFrequency(hz) ? snapTempoHz(hz, descriptor.min, max) : hz;
    onChange(snapped, true);
  };

  return (
    <div className={`param frequency-row${invalid ? " invalid" : ""}`} title={invalid}>
      <div className="param-top">
        <label htmlFor={id}>{label}</label>
        <span className="frequency-values">
          <button type="button" className="value nodrag" ref={anchor}
            aria-haspopup="dialog" aria-expanded={open} aria-controls={open ? id + "-editor" : undefined}
            aria-label={label + ": " + primary + ". Edit value"}
            title={bpmLed ? "Edit tempo" : rhythmic ? "Edit Hz or BPM" : "Edit Hz or enter a note"} onClick={() => setOpen(!open)}>
            {primary}
          </button>
          {secondary ? (
            <button type="button" className={`frequency-swap nodrag${override ? " overridden" : ""}`}
              aria-label={"Lead with " + nextUnit} title={"Lead with " + nextUnit + (override ? " (display override)" : "")}
              onClick={() => onOverride(!rhythmic && nextMode === defaultMode ? undefined : nextMode)}>
              {secondary}
            </button>
          ) : value >= 0 && value < PITCH_FLOOR_HZ ? <span className="frequency-secondary">rate</span> : null}
        </span>
      </div>
      <ParameterRange id={id} min={0} max={SLIDER_STEPS} step="any" position={toSlider(range, value)}
        valueText={primary} onBeginContinuous={onBeginContinuous} onMove={move}
        onStep={noteLed || bpmLed ? (direction) => {
          setReference(undefined);
          const snap = bpmLed ? snapTempoHz : snapSliderHz;
          onChange(snap(value, descriptor.min, max, direction), true);
        } : undefined} />
      {open && <FrequencyPopover id={id + "-editor"} anchor={anchor} label={bpmLed ? label : descriptor.label}
        value={value} pitch={pitch} min={descriptor.min} max={max} mode={mode} rhythmic={rhythmic}
        preferredMode={override} onModeChange={oscillator ? onOverride : undefined}
        onClose={close} onBeginContinuous={onBeginContinuous}
        onFrequency={(hz) => {
          setReference(undefined);
          if (hz !== value) onChange(hz, false);
        }}
        onPitch={(next, continuous) => {
          setReference(next.midi);
          onChange(pitchToHz(next), continuous);
        }} />}
    </div>
  );
}
