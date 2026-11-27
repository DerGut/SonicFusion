import { useState } from "react";

import type { ParamDescriptor } from "../api/types";
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
  return value.toFixed(2);
}

interface Props {
  id: string;
  descriptor: ParamDescriptor;
  label?: string;
  value: number;
  invalid?: string;
  /** `continuous` while dragging, so the edit history records one step. */
  onChange: (value: number, continuous: boolean) => void;
  onBeginContinuous: () => void;
}

/** Label, value (click to type) and slider for one numeric parameter. */
export function ParamControl({ id, descriptor, label, value, invalid, onChange, onBeginContinuous }: Props) {
  const [typing, setTyping] = useState<string | null>(null);
  const position = toSlider(descriptor, value);
  const fill = `linear-gradient(90deg, var(--fill) 0 ${position / 10}%, var(--track) ${position / 10}% 100%)`;
  const commit = () => {
    const parsed = Number(typing);
    if (typing !== null && typing.trim() !== "" && Number.isFinite(parsed)) onChange(parsed, false);
    setTyping(null);
  };
  return (
    <div className={`param${invalid ? " invalid" : ""}`} title={invalid}>
      <div className="param-top">
        <label htmlFor={id}>{label ?? descriptor.label}</label>
        {typing === null ? (
          <button type="button" className="value nodrag" title="Click to type a value"
            aria-label={`${label ?? descriptor.label}: ${formatValue(descriptor, value)}. Type a value`}
            onClick={() => setTyping(String(value))}>
            {formatValue(descriptor, value)}
          </button>
        ) : (
          <input className="value-input nodrag" autoFocus aria-label={`${label ?? descriptor.label} value`}
            onFocus={(event) => event.target.select()}
            value={typing} onChange={(event) => setTyping(event.target.value)} onBlur={commit}
            onKeyDown={(event) => {
              if (event.key === "Enter") commit();
              if (event.key === "Escape") setTyping(null);
            }} />
        )}
      </div>
      <input id={id} className="range nodrag" type="range" min={0} max={STEPS} step="any"
        value={position} style={{ background: fill }}
        aria-valuetext={formatValue(descriptor, value)}
        onPointerDown={onBeginContinuous}
        onKeyDown={(event) => VALUE_KEYS.has(event.key) && onBeginContinuous()}
        onChange={(event) => onChange(fromSlider(descriptor, Number(event.target.value)), true)} />
    </div>
  );
}
