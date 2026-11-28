export const PITCH_FLOOR_HZ = 20;
export const DETUNE_STEP = 0.1;
export const FREQUENCY_MODES = [["hz", "Hz"], ["note", "Note"], ["bpm", "BPM"]] as const;
export type FrequencyMode = typeof FREQUENCY_MODES[number][0];

const SECONDS_PER_MINUTE = 60;
export const TEMPO_CEILING_BPM = PITCH_FLOOR_HZ * SECONDS_PER_MINUTE;
export type FrequencyRole = "audio" | "control" | "gate";

const A4_HZ = 440;
const A4_MIDI = 69;
const SEMITONES = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
const NATURALS = ["C", "D", "E", "F", "G", "A", "B"];
const NATURAL_STEPS = [0, 2, 4, 5, 7, 9, 11];

export interface Pitch {
  midi: number;
  cents: number;
}

export function hzToMidi(hz: number): number {
  return A4_MIDI + 12 * (Math.log2(hz) - Math.log2(A4_HZ));
}

export function pitchToHz({ midi, cents }: Pitch): number {
  return A4_HZ * 2 ** ((midi - A4_MIDI + cents / 100) / 12);
}

/** Keep the chosen note at the ±50 cent boundary while moving Detune. */
export function pitchFromHz(hz: number, reference?: number): Pitch | null {
  if (!(hz > 0) || !Number.isFinite(hz)) return null;
  const semitone = hzToMidi(hz);
  const midi = reference !== undefined && Math.abs(semitone - reference) < 0.5 + 1e-9
    ? reference
    : Math.round(semitone);
  return { midi, cents: (semitone - midi) * 100 };
}

export function isTempoFrequency(hz: number): boolean {
  return hz >= 0 && hz < PITCH_FLOOR_HZ;
}

export function defaultFrequencyMode(hz: number, role: FrequencyRole, oscillator: boolean): FrequencyMode {
  if (oscillator && role === "gate" && isTempoFrequency(hz)) return "bpm";
  return role === "audio" && hz >= PITCH_FLOOR_HZ ? "note" : "hz";
}

/** A semitone within the slider range, or free Hz for subaudio rates. */
export function snapSliderHz(hz: number, min: number, max: number, direction = 0): number {
  if (!(hz >= PITCH_FLOOR_HZ) && direction === 0) return hz;
  const first = Math.ceil(hzToMidi(Math.max(min, PITCH_FLOOR_HZ)));
  const last = Math.floor(hzToMidi(max));
  if (first > last) return hz;
  const nearest = Math.round(hzToMidi(hz)) + direction;
  return pitchToHz({ midi: Math.min(last, Math.max(first, nearest)), cents: 0 });
}

/** Slider bounds rounded inward to the resolutions the range can emit. */
export function detuneBounds(midi: number, minHz: number, maxHz: number): { min: number; max: number } | null {
  const center = pitchToHz({ midi, cents: 0 });
  const minCents = Math.max(-50, 1200 * Math.log2(minHz / center));
  const maxCents = Math.min(50, 1200 * Math.log2(maxHz / center));
  const ticksPerCent = 1 / DETUNE_STEP;
  let minTick = Math.ceil(minCents * ticksPerCent);
  let maxTick = Math.floor(maxCents * ticksPerCent);
  // The conversion back to Hz can round across an exact boundary.
  if (pitchToHz({ midi, cents: minTick / ticksPerCent }) < minHz) minTick++;
  if (pitchToHz({ midi, cents: maxTick / ticksPerCent }) > maxHz) maxTick--;
  return minTick <= maxTick ? { min: minTick / ticksPerCent, max: maxTick / ticksPerCent } : null;
}

export function noteName(midi: number): string {
  const step = ((midi % 12) + 12) % 12;
  return SEMITONES[step] + (Math.floor(midi / 12) - 1);
}

export function formatCents(cents: number): string {
  const rounded = Number(cents.toFixed(1));
  return (rounded > 0 ? "+" : "") + rounded + "¢";
}

export function hzToBpm(hz: number): number {
  return hz * SECONDS_PER_MINUTE;
}

export function bpmToHz(bpm: number): number {
  return bpm / SECONDS_PER_MINUTE;
}

export function snapTempoHz(hz: number, min: number, max: number, direction = 0): number {
  let first = Math.ceil(hzToBpm(min));
  let last = Math.min(TEMPO_CEILING_BPM - 1, Math.floor(hzToBpm(max)));
  if (bpmToHz(first) < min) first++;
  if (bpmToHz(last) > max) last--;
  if (first > last) return hz;
  const bpm = hzToBpm(hz);
  const nearest = Math.round(bpm);
  const basis = bpmToHz(nearest) === hz ? nearest : bpm;
  const next = direction > 0 ? Math.floor(basis) + 1 : direction < 0 ? Math.ceil(basis) - 1 : nearest;
  if (direction > 0 && next > last) return max >= PITCH_FLOOR_HZ ? PITCH_FLOOR_HZ : hz;
  if (direction < 0 && next < first) return hz;
  return bpmToHz(Math.min(last, Math.max(first, next)));
}

/** Bare numbers follow the displayed unit. Explicit units and notes override it. */
export function parseFrequency(text: string, mode: FrequencyMode = "hz", allowBpm = mode === "bpm"): number {
  const value = text.trim();
  const suffix = /(?:bpm|hz)$/i.exec(value);
  if (suffix) {
    const number = value.slice(0, suffix.index).trim();
    const bpm = suffix[0].toLowerCase() === "bpm";
    if (!number || (bpm && !allowBpm)) return NaN;
    const hz = bpm ? bpmToHz(Number(number)) : Number(number);
    return bpm && !isTempoFrequency(hz) ? NaN : hz;
  }
  const match = /^([A-G])([#b♯♭]?)(-?\d+)(?:\s*([+-]\d+(?:\.\d+)?)\s*[c¢])?$/i.exec(value);
  if (!match) return mode === "bpm" ? bpmToHz(Number(value)) : Number(value);
  const [, letter, accidental, octave, detune] = match;
  const natural = NATURAL_STEPS[NATURALS.indexOf(letter.toUpperCase())];
  const adjustment = accidental === "#" || accidental === "♯" ? 1 : accidental ? -1 : 0;
  const hz = pitchToHz({
    midi: (Number(octave) + 1) * 12 + natural + adjustment,
    cents: Number(detune ?? 0),
  });
  return hz > 0 ? hz : NaN;
}
