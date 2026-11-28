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
  let gatePath = "";
  let mixedPath = "";
  if (gate?.kind === "edges") {
    let open = gate.start_open;
    gatePath = `M0 ${open ? 8 : 132}`;
    for (const frame of gate.transitions) {
      open = !open;
      gatePath += `H${x(frame)}V${open ? 8 : 132}`;
    }
    gatePath += "H1000";
  } else if (gate?.kind === "bands") {
    // Dense gates can change several times in a bin. Shade the entire bin
    // instead of suggesting that an edge occurred at a particular frame.
    gate.states.forEach((state, index) => {
      const start = x(index * bucket_frames);
      const end = x((index + 1) * bucket_frames);
      if (state === "mixed") {
        mixedPath += `M${start} 8H${end}V132H${start}Z`;
      } else {
        gatePath += `M${start} ${state === "open" ? 8 : 132}H${end}`;
      }
    });
  }
  const duration = formatSeconds(frames / scope.sample_rate_hz);
  return (
    <svg viewBox="0 0 1000 140" preserveAspectRatio="none" role="img"
      aria-label={`${gate ? "Envelope and gate" : "Waveform"} over ${duration}`}>
      <path d="M0 70H1000" className="grid" vectorEffect="non-scaling-stroke" />
      {gate ? (
        <>
          <path d={upper ? upper + lower + "Z" : ""} className="scope-peak-band" vectorEffect="non-scaling-stroke" />
          {mixedPath && <path d={mixedPath} className="scope-gate-mixed" />}
          <path d={gatePath} className="scope-gate" vectorEffect="non-scaling-stroke" />
        </>
      ) : <path d={wave} className="trace" vectorEffect="non-scaling-stroke" />}
    </svg>
  );
}
