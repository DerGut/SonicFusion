/** Display helpers for probe windows: a triggered scope trace and a spectrum. */

/** Start index of the first rising zero crossing, so periodic traces hold still. */
export function triggerIndex(samples: number[], searchUpTo: number): number {
  const limit = Math.min(searchUpTo, samples.length - 1);
  for (let index = 1; index < limit; index++) {
    if (samples[index - 1] < 0 && samples[index] >= 0) return index;
  }
  return 0;
}

const SPECTRUM_FLOOR_DB = -180;

/**
 * Magnitude spectrum in dBFS for a power-of-two window, Hann-weighted and
 * normalized so a full-scale sine reads about 0 dB. Bin `k` is
 * `k * sampleRate / samples.length` Hz.
 */
export function spectrumDb(samples: number[]): number[] {
  const size = samples.length;
  if (size < 2 || (size & (size - 1)) !== 0) {
    throw new Error("spectrum needs a power-of-two window");
  }
  const real = new Float64Array(size);
  const imag = new Float64Array(size);
  // Remove the DC component before windowing so a steady value has no AC peak.
  const mean = samples.reduce((sum, sample) => sum + sample, 0) / size;
  let windowSum = 0;
  for (let index = 0; index < size; index++) {
    const hann = 0.5 - 0.5 * Math.cos((2 * Math.PI * index) / (size - 1));
    real[index] = (samples[index] - mean) * hann;
    windowSum += hann;
  }
  fft(real, imag);
  const bins = size / 2;
  const result = new Array<number>(bins);
  for (let bin = 0; bin < bins; bin++) {
    const magnitude = (2 * Math.hypot(real[bin], imag[bin])) / windowSum;
    result[bin] = magnitude > 1e-9 ? 20 * Math.log10(magnitude) : SPECTRUM_FLOOR_DB;
  }
  return result;
}

/** In-place iterative radix-2 FFT. */
function fft(real: Float64Array, imag: Float64Array) {
  const size = real.length;
  for (let index = 1, swap = 0; index < size; index++) {
    let bit = size >> 1;
    for (; swap & bit; bit >>= 1) swap ^= bit;
    swap ^= bit;
    if (index < swap) {
      [real[index], real[swap]] = [real[swap], real[index]];
      [imag[index], imag[swap]] = [imag[swap], imag[index]];
    }
  }
  for (let length = 2; length <= size; length <<= 1) {
    const angle = (-2 * Math.PI) / length;
    for (let start = 0; start < size; start += length) {
      for (let offset = 0; offset < length / 2; offset++) {
        const cos = Math.cos(angle * offset);
        const sin = Math.sin(angle * offset);
        const a = start + offset;
        const b = a + length / 2;
        const tr = real[b] * cos - imag[b] * sin;
        const ti = real[b] * sin + imag[b] * cos;
        real[b] = real[a] - tr;
        imag[b] = imag[a] - ti;
        real[a] += tr;
        imag[a] += ti;
      }
    }
  }
}

/** Estimate the strongest AC component between FFT bins in a Hann window. */
export function strongestPartialHz(spectrum: number[], rate: number): number | null {
  let peakBin = 0;
  let peakDb = SPECTRUM_FLOOR_DB;
  for (let bin = 1; bin < spectrum.length; bin++) {
    if (spectrum[bin] > peakDb) {
      peakBin = bin;
      peakDb = spectrum[bin];
    }
  }
  if (peakBin === 0) return null;
  let correction = 0;
  // A DC bin is not a usable neighbor, even if the strongest AC bin is bin 1.
  if (peakBin > 1 && peakBin + 1 < spectrum.length) {
    const left = spectrum[peakBin - 1];
    const right = spectrum[peakBin + 1];
    const curvature = left - 2 * peakDb + right;
    if (curvature < 0) correction = 0.5 * (left - right) / curvature;
  }
  return ((peakBin + correction) * rate) / (2 * spectrum.length);
}

export function formatHz(hz: number): string {
  if (hz < 10) return `${hz.toFixed(2)} Hz`;
  if (hz < 1000) return `${hz.toFixed(1)} Hz`;
  return `${(hz / 1000).toFixed(2)} kHz`;
}

export function formatDb(db: number | null): string {
  if (db === null || !Number.isFinite(db)) return "−∞ dB";
  return `${db < -0.05 ? "−" : ""}${Math.abs(db).toFixed(1)} dB`;
}
