export const PITCH_FLOOR_HZ = 20;
export const DETUNE_STEP = 0.1;
export const FREQUENCY_MODES = [["hz", "Hz"], ["note", "Note"], ["bpm", "BPM"]] as const;
export type FrequencyMode = typeof FREQUENCY_MODES[number][0];

const SECONDS_PER_MINUTE = 60;

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

export function defaultFrequencyMode(hz: number, feedsControl: boolean): FrequencyMode {
  return feedsControl || !(hz >= PITCH_FLOOR_HZ) ? "hz" : "note";
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

/** Bare numbers are Hz except in BPM mode. Note names can include a cents offset. */
export function parseFrequency(text: string, mode: FrequencyMode = "hz"): number {
  if (mode === "bpm") {
    const bpm = text.trim().replace(/\s*bpm$/i, "");
    return bpm ? Number(bpm) / SECONDS_PER_MINUTE : NaN;
  }
  const match = /^([A-G])([#b♯♭]?)(-?\d+)(?:\s*([+-]\d+(?:\.\d+)?)\s*[c¢])?$/i.exec(text.trim());
  if (!match) return Number(text);
  const [, letter, accidental, octave, detune] = match;
  const natural = NATURAL_STEPS[NATURALS.indexOf(letter.toUpperCase())];
  const adjustment = accidental === "#" || accidental === "♯" ? 1 : accidental ? -1 : 0;
  const hz = pitchToHz({
    midi: (Number(octave) + 1) * 12 + natural + adjustment,
    cents: Number(detune ?? 0),
  });
  return hz > 0 ? hz : NaN;
}
