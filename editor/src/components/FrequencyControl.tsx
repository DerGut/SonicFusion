import { useCallback, useRef, useState } from "react";

import type { ParamDescriptor } from "../api/types";
import { defaultFrequencyMode, PITCH_FLOOR_HZ, noteName, pitchFromHz, pitchToHz, snapSliderHz, type FrequencyMode, type Pitch } from "../model/pitch";
import { formatHz } from "../model/signal";
import { FrequencyPopover } from "./FrequencyPopover";
import { fromSlider, SLIDER_STEPS, toSlider } from "./ParamControl";
import { ParameterRange } from "./ParameterRange";

function compactHz(hz: number): string {
  const kilo = hz >= 1_000;
  return Number((kilo ? hz / 1_000 : hz).toPrecision(3)) + (kilo ? "\u202fkHz" : "\u202fHz");
}

function compactPitch({ midi, cents }: Pitch): string {
  const rounded = Math.round(cents);
  const detune = rounded === 0 ? "" : " " + (rounded > 0 ? "+" : "−") + Math.abs(rounded) + "¢";
  return noteName(midi) + detune;
}

export function FrequencyControl({ id, descriptor, value, invalid, sampleRate, feedsControl, override, onOverride,
  onChange, onBeginContinuous }: {
  id: string;
  descriptor: ParamDescriptor;
  value: number;
  invalid?: string;
  sampleRate: number;
  feedsControl: boolean;
  override?: FrequencyMode;
  onOverride: (mode?: FrequencyMode) => void;
  onChange: (value: number, continuous: boolean) => void;
  onBeginContinuous: () => void;
}) {
  const anchor = useRef<HTMLButtonElement>(null);
  const [open, setOpen] = useState(false);
  const close = useCallback(() => setOpen(false), []);
  const [reference, setReference] = useState<number>();
  const defaultMode = defaultFrequencyMode(value, feedsControl);
  const pitch = value >= PITCH_FLOOR_HZ ? pitchFromHz(value, reference) : null;
  const noteLed = pitch !== null && (override ?? defaultMode) === "note";
  const unit = noteLed ? "Hz" : "Note";
  const label = noteLed && descriptor.frequency === "pitch" ? "Pitch" : descriptor.label;
  const primary = noteLed ? compactPitch(pitch) : value > 0 && value < 0.01 ? compactHz(value) : formatHz(value);
  const secondary = pitch && (noteLed ? compactHz(value) : (Math.abs(pitch.cents) < 0.5 ? "" : "≈") + noteName(pitch.midi));
  // The controls use the catalog's editing range and the device's strict Nyquist limit.
  const max = Math.min(descriptor.max, sampleRate / 2 * (1 - Number.EPSILON));
  const range = { ...descriptor, max };
  const move = (position: number, free: boolean) => {
    const hz = Math.min(max, Math.max(descriptor.min, fromSlider(range, position)));
    setReference(undefined);
    onChange(noteLed && !free ? snapSliderHz(hz, descriptor.min, max) : hz, true);
  };

  return (
    <div className={`param frequency-row${invalid ? " invalid" : ""}`} title={invalid}>
      <div className="param-top">
        <label htmlFor={id}>{label}</label>
        <span className="frequency-values">
          <button type="button" className="value nodrag" ref={anchor}
            aria-haspopup="dialog" aria-expanded={open} aria-controls={open ? id + "-editor" : undefined}
            aria-label={descriptor.label + ": " + primary + ". Edit value"}
            title="Edit Hz or enter a note" onClick={() => setOpen(!open)}>
            {primary}
          </button>
          {secondary ? (
            <button type="button" className={`frequency-swap nodrag${override ? " overridden" : ""}`}
              aria-label={"Lead with " + unit} title={"Lead with " + unit + (override ? " (display override)" : "")}
              onClick={() => {
                const next = noteLed ? "hz" : "note";
                onOverride(next === defaultMode ? undefined : next);
              }}>
              {secondary}
            </button>
          ) : value >= 0 && value < PITCH_FLOOR_HZ ? <span className="frequency-secondary">rate</span> : null}
        </span>
      </div>
      <ParameterRange id={id} min={0} max={SLIDER_STEPS} step="any" position={toSlider(range, value)}
        valueText={primary} onBeginContinuous={onBeginContinuous} onMove={move}
        onStep={noteLed ? (direction) => {
          setReference(undefined);
          onChange(snapSliderHz(value, descriptor.min, max, direction), true);
        } : undefined} />
      {open && <FrequencyPopover id={id + "-editor"} anchor={anchor} label={descriptor.label}
        value={value} pitch={pitch} min={descriptor.min} max={max}
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
