export const PITCH_FLOOR_HZ = 20;
export type FrequencyMode = "hz" | "note";

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

export function noteName(midi: number): string {
  const step = ((midi % 12) + 12) % 12;
  return SEMITONES[step] + (Math.floor(midi / 12) - 1);
}

export function formatCents(cents: number): string {
  const rounded = Number(cents.toFixed(1));
  return (rounded > 0 ? "+" : "") + rounded + "¢";
}

/** Numeric input remains Hz in either mode. Notes can include an explicit cents offset. */
export function parseFrequency(text: string): number {
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
