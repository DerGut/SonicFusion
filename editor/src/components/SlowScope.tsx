import type { ScopeWindow } from "../api/types";
import { formatSeconds } from "../model/signal";

export function SlowScope({ scope }: { scope: ScopeWindow }) {
  const { frames, bucket_frames, bands, gate } = scope;
  const x = (frame: number) => (1_000 * Math.min(frame, frames)) / Math.max(1, frames);
  const center = (index: number) => index * bucket_frames + Math.min(bucket_frames, frames - index * bucket_frames) / 2;
  const peaks = bands.map(([min, max]) => Math.max(-min, max));
  const largest = Math.max(1e-6, ...peaks);
  const scale = 51 / largest;
  const wave = bands.map(([min, max], index) =>
    `${index === 0 ? "M" : "L"}${x(center(index))} ${70 - (min + max) * 25.5}`,
  ).join("");
  const upper = peaks.map((peak, index) =>
    `${index === 0 ? "M" : "L"}${x(center(index))} ${70 - peak * scale}`,
  ).join("");
  const lower = peaks.map((peak, index) => `L${x(center(index))} ${70 + peak * scale}`).reverse().join("");
  const gatePath = gate?.map((open, index) => {
    const y = open ? 8 : 132;
    return `${index === 0 ? "M0 " : "V"}${y}H${x((index + 1) * bucket_frames)}`;
  }).join("");
  const duration = formatSeconds(frames / scope.sample_rate_hz);
  return (
    <svg viewBox="0 0 1000 140" preserveAspectRatio="none" role="img"
      aria-label={`${gate ? "Envelope and gate" : "Waveform"} over ${duration}`}>
      <path d="M0 70H1000" className="grid" vectorEffect="non-scaling-stroke" />
      {gate ? (
        <>
          <path d={upper ? upper + lower + "Z" : ""} className="scope-peak-band" vectorEffect="non-scaling-stroke" />
          <path d={gatePath} className="scope-gate" vectorEffect="non-scaling-stroke" />
        </>
      ) : <path d={wave} className="trace" vectorEffect="non-scaling-stroke" />}
    </svg>
  );
}
