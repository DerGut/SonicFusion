import { describe, expect, it } from "vitest";

import { formatDb, formatHz, spectrumDb, triggerIndex } from "./signal";

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
