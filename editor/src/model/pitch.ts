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

export function noteName(midi: number): string {
  const step = ((midi % 12) + 12) % 12;
  return SEMITONES[step] + (Math.floor(midi / 12) - 1);
}

export function formatCents(cents: number): string {
  const rounded = Number(cents.toFixed(1));
  return (rounded > 0 ? "+" : "") + rounded + "¢";
}

export function formatPitch(pitch: Pitch): string {
  const name = noteName(pitch.midi);
  return Math.abs(pitch.cents) < 0.05 ? name : name + " " + formatCents(pitch.cents);
}

/** Numeric input remains Hz in either mode. Notes can include an explicit cents offset. */
export function parseFrequency(text: string): number {
  const match = /^([A-G])([#b♯♭]?)(-?\d+)(?:\s*([+-]\d+(?:\.\d+)?)\s*[c¢])?$/i.exec(text.trim());
  if (!match) return Number(text);
  const [, letter, accidental, octave, detune] = match;
  const natural = NATURAL_STEPS[NATURALS.indexOf(letter.toUpperCase())];
  const adjustment = accidental === "#" || accidental === "♯" ? 1 : accidental ? -1 : 0;
  return pitchToHz({
    midi: (Number(octave) + 1) * 12 + natural + adjustment,
    cents: Number(detune ?? 0),
  });
}
