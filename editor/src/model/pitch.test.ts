import { describe, expect, it } from "vitest";

import { defaultFrequencyMode, detuneBounds, formatCents, hzToBpm, noteName, parseFrequency, pitchFromHz, pitchToHz, snapSliderHz } from "./pitch";

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

  it("limits detune so the edited pitch stays below Nyquist and above the pitch floor", () => {
    const high = pitchFromHz(parseFrequency("B9"))!.midi;
    const limited = detuneBounds(high, 20, 16_000 * (1 - Number.EPSILON))!;
    expect(limited.min).toBe(-50);
    expect(limited.max).toBeLessThan(50);
    expect(pitchToHz({ midi: high, cents: limited.max })).toBeLessThan(16_000);

    const low = pitchFromHz(20)!.midi;
    const nearFloor = detuneBounds(low, 20, 20_000)!;
    expect(nearFloor.min).toBeGreaterThan(0);
    expect(pitchToHz({ midi: low, cents: nearFloor.min })).toBeGreaterThanOrEqual(20);
    expect(detuneBounds(69, 20, 20_000)).toEqual({ min: -50, max: 50 });
    expect(detuneBounds(pitchFromHz(24_500)!.midi, 20, 20_000)).toBeNull();
  });

  it("converts BPM entry to Hz without changing Hz or note entry", () => {
    expect(parseFrequency("120", "bpm")).toBe(2);
    expect(parseFrequency(" 0.6 bpm ", "bpm")).toBe(0.01);
    expect(parseFrequency("0", "bpm")).toBe(0);
    expect(parseFrequency("BPM", "bpm")).toBeNaN();
    expect(parseFrequency("120 BPM")).toBeNaN();
    expect(parseFrequency("120", "note")).toBe(120);
    expect(parseFrequency("A4", "note")).toBe(440);
    expect(hzToBpm(2)).toBe(120);
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
