import { describe, expect, it } from "vitest";

import { formatPitch, noteName, parseFrequency, pitchFromHz, pitchToHz } from "./pitch";

describe("frequency and pitch", () => {
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
    const text = formatPitch(pitch);
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
