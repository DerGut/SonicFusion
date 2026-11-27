import { describe, expect, it } from "vitest";

import { noteName, pitchFromHz } from "./pitch";
import { formatDb, formatHz, spectrumDb, strongestPartial, triggerIndex } from "./signal";

const RATE = 48_000;
const sine = (hz: number, size: number, phase = 0) =>
  Array.from({ length: size }, (_, n) => Math.sin((2 * Math.PI * hz * n) / RATE + phase));

describe("signal helpers", () => {
  it("finds the peak bin of a full-scale sine near 0 dB", () => {
    const size = 2048;
    const hz = (100 * RATE) / size; // exactly bin 100
    const spectrum = spectrumDb(sine(hz, size));
    const peak = spectrum.indexOf(Math.max(...spectrum));
    expect(peak).toBe(100);
    expect(spectrum[peak]).toBeGreaterThan(-1);
    expect(spectrum[peak]).toBeLessThan(0.5);
    expect(spectrum[400]).toBeLessThan(-60);
  });

  it.each([[110, "A2"], [220, "A3"], [440, "A4"]])("estimates %i Hz between FFT bins as %s", (hz, name) => {
    const found = strongestPartial(spectrumDb(sine(hz, 2048)), RATE);
    expect(found).not.toBeNull();
    expect(found!.canNamePitch).toBe(true);
    expect(Math.abs(found!.hz - hz)).toBeLessThan(0.5);
    expect(noteName(pitchFromHz(found!.hz)!.midi)).toBe(name);
  });

  it.each([27.5, 32.703])("does not name a bass pitch unresolved by the FFT at %f Hz", (hz) => {
    expect(strongestPartial(spectrumDb(sine(hz, 2048)), RATE)).toEqual({
      hz: RATE / 2048,
      canNamePitch: false,
    });
  });

  it("has no strongest partial for silence or a steady DC value", () => {
    for (const value of [0, 0.25]) {
      expect(strongestPartial(spectrumDb(Array<number>(2048).fill(value)), RATE)).toBeNull();
    }
  });

  it("triggers on the first rising zero crossing", () => {
    const samples = sine(1000, 200, Math.PI / 2);
    const start = triggerIndex(samples, 100);
    expect(samples[start - 1]).toBeLessThan(0);
    expect(samples[start]).toBeGreaterThanOrEqual(0);
  });

  it("formats readouts", () => {
    expect(formatHz(0.5)).toBe("0.50 Hz");
    expect(formatHz(1200)).toBe("1.20 kHz");
    expect(formatDb(-21.94)).toBe("−21.9 dB");
    expect(formatDb(null)).toBe("−∞ dB");
  });
});
