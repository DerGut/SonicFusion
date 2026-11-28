import { useEffect, useRef } from "react";

import { BEAT_STEP_LABELS, STEPS_PER_BEAT, activeSteps, invertSteps, setStep, shiftSteps, stepName } from "../model/pattern";

/**
 * The sequencer's step grid in the probe panel. Click a step to toggle it, or
 * drag across steps to paint the state the first one changed to. Space or
 * Enter toggles the focused step and the arrow keys move between steps.
 */
export function PatternEditor({ steps, gateLength, onChange, onBeginContinuous }: {
  steps: boolean[];
  gateLength: number;
  /** `continuous` while painting, so one drag is one undo step. */
  onChange: (steps: boolean[], continuous: boolean) => void;
  onBeginContinuous: () => void;
}) {
  const grid = useRef<HTMLDivElement>(null);
  const paint = useRef<boolean | null>(null);
  // Later paints in one drag build on earlier ones before the document re-renders.
  const latest = useRef(steps);
  latest.current = steps;

  const apply = (index: number, on: boolean, continuous: boolean) => {
    if (latest.current[index] === on) return;
    latest.current = setStep(latest.current, index, on);
    onChange(latest.current, continuous);
  };
  const applyRef = useRef(apply);
  applyRef.current = apply;

  useEffect(() => {
    // Hit-test on move so painting also works with touch's implicit pointer capture.
    const move = (event: PointerEvent) => {
      if (paint.current === null) return;
      const cell = document.elementFromPoint(event.clientX, event.clientY)?.closest<HTMLElement>("[data-step]");
      if (cell && grid.current?.contains(cell)) applyRef.current(Number(cell.dataset.step), paint.current, true);
    };
    const stop = () => { paint.current = null; };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", stop);
    window.addEventListener("pointercancel", stop);
    return () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", stop);
      window.removeEventListener("pointercancel", stop);
    };
  }, []);

  const focusStep = (index: number) => {
    grid.current?.querySelector<HTMLButtonElement>(`[data-step="${(index + steps.length) % steps.length}"]`)?.focus();
  };

  return (
    <div className="pattern-editor">
      <div className="pattern-grid" ref={grid} role="group" aria-label={`Pattern, ${activeSteps(steps)} of ${steps.length} steps on`}>
        {steps.map((on, index) => (
          <button key={index} type="button" data-step={index} aria-pressed={on} aria-label={stepName(index)}
            className={`pattern-step${on ? " on" : ""}${index % STEPS_PER_BEAT === 0 ? " downbeat" : ""}`}
            onPointerDown={(event) => {
              if (event.button !== 0) return;
              event.preventDefault();
              onBeginContinuous();
              paint.current = !latest.current[index];
              apply(index, paint.current, true);
            }}
            onClick={(event) => {
              // Pointer presses toggle on pointerdown; this handles Space and Enter.
              if (event.detail === 0) apply(index, !latest.current[index], false);
            }}
            onKeyDown={(event) => {
              if (event.key === "ArrowRight" || event.key === "ArrowLeft") {
                event.preventDefault();
                focusStep(index + (event.key === "ArrowRight" ? 1 : -1));
              }
            }}>
            <span className="pattern-step-label">
              {index % STEPS_PER_BEAT === 0 ? index / STEPS_PER_BEAT + 1 : BEAT_STEP_LABELS[index % STEPS_PER_BEAT]}
            </span>
            {on && <span className="pattern-step-gate" style={{ width: `${Math.max(0, Math.min(1, gateLength)) * 100}%` }} />}
          </button>
        ))}
      </div>
      <div className="pattern-tools">
        <button type="button" onClick={() => onChange(steps.map(() => false), false)}>Clear</button>
        <button type="button" aria-label="Shift the pattern one step earlier" onClick={() => onChange(shiftSteps(steps, -1), false)}>← Shift</button>
        <button type="button" aria-label="Shift the pattern one step later" onClick={() => onChange(shiftSteps(steps, 1), false)}>Shift →</button>
        <button type="button" onClick={() => onChange(invertSteps(steps), false)}>Invert</button>
        <span className="hint">Click or drag to paint · Space toggles the focused step</span>
      </div>
    </div>
  );
}

/** A read-only strip of the pattern for the node itself. */
export function PatternStrip({ steps }: { steps: boolean[] }) {
  return (
    <div className="pattern-strip" aria-hidden="true">
      {steps.map((on, index) => (
        <span key={index} className={`${on ? "on" : ""}${index % STEPS_PER_BEAT === 0 ? " downbeat" : ""}`} />
      ))}
    </div>
  );
}
