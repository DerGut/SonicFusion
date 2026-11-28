import type { ParamValue } from "../api/types";

/** Labels of the four 16th steps in a beat. */
export const BEAT_STEP_LABELS = ["1", "e", "+", "a"] as const;
export const STEPS_PER_BEAT = 4;

/**
 * A saved pattern as on/off steps. Anything that isn't a list of `length`
 * zeros and ones reads as empty, so a malformed patch still renders while
 * validation reports the problem.
 */
export function readSteps(value: ParamValue | undefined, length: number): boolean[] {
  if (!Array.isArray(value) || value.length !== length) return Array.from({ length }, () => false);
  return value.map((step) => step === 1);
}

export function writeSteps(steps: readonly boolean[]): number[] {
  return steps.map((on) => (on ? 1 : 0));
}

export function setStep(steps: readonly boolean[], index: number, on: boolean): boolean[] {
  return steps.map((current, step) => (step === index ? on : current));
}

/** Move every step later by `by` (negative moves earlier), wrapping around the bar. */
export function shiftSteps(steps: readonly boolean[], by: number): boolean[] {
  const length = steps.length;
  return steps.map((_, step) => steps[(((step - by) % length) + length) % length]);
}

export function invertSteps(steps: readonly boolean[]): boolean[] {
  return steps.map((on) => !on);
}

export function activeSteps(steps: readonly boolean[]): number {
  return steps.filter(Boolean).length;
}

/** "Beat 2, e" for the second step of the second beat. */
export function stepName(index: number): string {
  return `Beat ${Math.floor(index / STEPS_PER_BEAT) + 1}, ${BEAT_STEP_LABELS[index % STEPS_PER_BEAT]}`;
}
