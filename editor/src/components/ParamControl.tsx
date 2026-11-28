import { useState } from "react";

import type { ParamDescriptor } from "../api/types";
import { formatHz } from "../model/signal";
import { ParameterRange } from "./ParameterRange";

export const SLIDER_STEPS = 1000;

export function toSlider(descriptor: ParamDescriptor, value: number): number {
  const { min, max, scale } = descriptor;
  const clamped = Math.min(max, Math.max(min, value));
  const fraction = scale === "log" ? Math.log(clamped / min) / Math.log(max / min) : (clamped - min) / (max - min);
  return fraction * SLIDER_STEPS;
}

export function fromSlider(descriptor: ParamDescriptor, position: number): number {
  const { min, max, scale } = descriptor;
  const fraction = position / SLIDER_STEPS;
  const value = scale === "log" ? min * Math.pow(max / min, fraction) : min + (max - min) * fraction;
  return Number(value.toPrecision(4));
}

export function formatValue(descriptor: ParamDescriptor, value: number): string {
  if (descriptor.unit === "s") return `${Number(value.toPrecision(4))} s`;
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
  const commit = () => {
    const parsed = Number(typing);
    if (typing !== null && typing.trim() !== "" && Number.isFinite(parsed) && parsed !== value) onChange(parsed, false);
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
      <ParameterRange id={id} min={0} max={SLIDER_STEPS} step="any" position={position}
        valueText={formatValue(descriptor, value)} onBeginContinuous={onBeginContinuous}
        onMove={(position) => onChange(fromSlider(descriptor, position), true)} />
    </div>
  );
}
