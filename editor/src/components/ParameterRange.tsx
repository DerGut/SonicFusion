import { useRef } from "react";

const VALUE_KEYS = new Set(["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "PageUp", "PageDown", "Home", "End"]);

export function ParameterRange({ id, min, max, step, position, valueText, onBeginContinuous, onMove, onStep }: {
  id: string;
  min: number;
  max: number;
  step: number | "any";
  position: number;
  valueText: string;
  onBeginContinuous: () => void;
  onMove: (position: number, free: boolean) => void;
  onStep?: (direction: number) => void;
}) {
  // The input event from a native slider does not carry modifier keys.
  const free = useRef(false);
  const clamped = Math.min(max, Math.max(min, position));
  const percent = max > min ? 100 * ((clamped - min) / (max - min)) : 0;
  const background = `linear-gradient(90deg, var(--fill) 0 ${percent}%, var(--track) ${percent}% 100%)`;
  return (
    <input id={id} className="range nodrag" type="range" min={min} max={max} step={step} value={clamped}
      style={{ background }} aria-valuetext={valueText}
      onPointerDown={(event) => { free.current = event.altKey; onBeginContinuous(); }}
      onPointerMove={(event) => { free.current = event.altKey; }}
      onKeyUp={(event) => { free.current = event.altKey; }}
      onKeyDown={(event) => {
        free.current = event.altKey;
        if (!VALUE_KEYS.has(event.key)) return;
        onBeginContinuous();
        if (onStep && !event.altKey && event.key.startsWith("Arrow")) {
          event.preventDefault();
          onStep(event.key === "ArrowUp" || event.key === "ArrowRight" ? 1 : -1);
        }
      }}
      onChange={(event) => onMove(Number(event.target.value), free.current)} />
  );
}
