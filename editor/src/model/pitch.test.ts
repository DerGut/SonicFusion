import { describe, expect, it } from "vitest";

import { defaultFrequencyMode, formatCents, noteName, parseFrequency, pitchFromHz, pitchToHz, snapSliderHz } from "./pitch";

describe("frequency and pitch", () => {
  it("defaults audible audio signals to notes and control or subaudio signals to Hz", () => {
    expect(defaultFrequencyMode(220, false)).toBe("note");
    expect(defaultFrequencyMode(440, true)).toBe("hz");
    expect(defaultFrequencyMode(0.5, false)).toBe("hz");
  });

  it("steps by semitone and snaps within the editor range", () => {
    expect(snapSliderHz(225, 0.01, 20_000)).toBe(220);
    expect(snapSliderHz(225, 0.01, 20_000, 1)).toBe(parseFrequency("A#3"));
    expect(snapSliderHz(0.5, 0.01, 20_000)).toBe(0.5);
    expect(snapSliderHz(220, 20, 219)).toBe(parseFrequency("G#3"));
    expect(snapSliderHz(20, 20, 20_000)).toBeGreaterThanOrEqual(20);
  });

  it("uses A4 = 440 Hz and C4 = MIDI 60", () => {
    expect(parseFrequency("A4")).toBe(440);
    expect(parseFrequency("A3")).toBe(220);
    expect(noteName(60)).toBe("C4");
    expect(parseFrequency("C#4")).toBeCloseTo(277.18263, 5);
  });

  it("accepts frequency, enharmonic spellings and typed detune", () => {
    expect(parseFrequency("220.1234")).toBe(220.1234);
    expect(parseFrequency(" db4 ")).toBe(parseFrequency("C♯4"));
    expect(parseFrequency("Cb4")).toBe(parseFrequency("B3"));
    expect(parseFrequency("A4 +25.5¢")).toBeCloseTo(440 * 2 ** (25.5 / 1200), 10);
    expect(parseFrequency("not a note")).toBeNaN();
  });

  it.each([0.01, 0.125, 1, 220, 443.567, 20_000])("round-trips %f Hz through note and cents", (hz) => {
    const pitch = pitchFromHz(hz)!;
    const centsText = formatCents(pitch.cents);
    const text = noteName(pitch.midi) + (centsText === "0¢" ? "" : " " + centsText);
    expect(pitchToHz(pitch)).toBeCloseTo(hz, 9);
    // The text is rounded for display, including low pitches below MIDI note 0.
    expect(Math.abs(1200 * Math.log2(parseFrequency(text) / hz))).toBeLessThan(0.051);
  });

  it("keeps the selected semitone at both detune slider endpoints", () => {
    for (const cents of [-50, 50]) {
      const pitch = pitchFromHz(pitchToHz({ midi: 69, cents }), 69)!;
      expect(pitch.midi).toBe(69);
      expect(pitch.cents).toBeCloseTo(cents, 9);
    }
  });

  it("has no note for DC or invalid frequencies", () => {
    for (const value of [0, -1, Infinity, NaN]) {
      expect(pitchFromHz(value)).toBeNull();
    }
  });
});
