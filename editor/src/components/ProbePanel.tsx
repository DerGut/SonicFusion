import { useEffect, useMemo, useState } from "react";

import type { EngineClient } from "../api/client";
import type { GraphSpec, ProbeWindow, SessionInfo } from "../api/types";
import type { DocNode } from "../model/document";
import { formatDb, formatHz, spectrumDb, triggerIndex } from "../model/signal";
import { describe } from "../state/useEngineSession";
import { Icon } from "./icons";

const PROBE_FRAMES = 2048;
const LIVE_INTERVAL_MS = 200;
const IDLE_DEBOUNCE_MS = 150;
const SCOPE_SECONDS = 0.02;

/**
 * Scope and spectrum of the selected node. The engine recomputes the node's
 * output from the draft at the session's current frame; nothing is tapped
 * from the device stream.
 */
export function ProbePanel({ client, graph, node, playing, draft, session, onClose }: {
  client: EngineClient;
  graph: GraphSpec;
  node: DocNode;
  playing: boolean;
  /** The draft differs from what is playing, so the probe shows unapplied edits. */
  draft: boolean;
  session: SessionInfo;
  onClose: () => void;
}) {
  const [view, setView] = useState<"scope" | "spectrum">("scope");
  const [probe, setProbe] = useState<ProbeWindow | null>(null);
  const [error, setError] = useState<string | null>(null);
  const graphJson = JSON.stringify(graph);

  useEffect(() => {
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout>;
    // One probe at a time; while playing, the next starts after the last returns.
    const run = () =>
      client
        .probe(JSON.parse(graphJson) as GraphSpec, node.id, PROBE_FRAMES)
        .then((result) => {
          if (cancelled) return;
          setProbe(result);
          setError(null);
        })
        .catch((reason) => !cancelled && setError(describe(reason)))
        .finally(() => {
          if (!cancelled && playing) timer = setTimeout(run, LIVE_INTERVAL_MS);
        });
    timer = setTimeout(run, playing ? 0 : IDLE_DEBOUNCE_MS);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [client, graphJson, node.id, playing]);

  const samples = useMemo(() => probe?.samples ?? [], [probe]);
  const peak = samples.reduce((max, sample) => Math.max(max, Math.abs(sample)), 0);
  const rate = probe?.sample_rate_hz ?? session.sample_rate_hz;
  const spectrum = useMemo(() => (samples.length === PROBE_FRAMES ? spectrumDb(samples) : []), [samples]);
  const peakBin = spectrum.reduce((best, db, bin) => (bin > 0 && db > (spectrum[best] ?? -Infinity) ? bin : best), 1);
  const cutoff = lowPassMarker(graph, node, session);

  return (
    <section className="probe" aria-label="Signal probe">
      <div className="probe-head">
        <Icon name="probe" />
        <span className="probe-title">{node.id} › out</span>
        <div role="tablist" aria-label="Probe view" className="tabs">
          {(["scope", "spectrum"] as const).map((tab) => (
            <button key={tab} type="button" role="tab" aria-selected={view === tab} aria-controls="probe-plot"
              className={view === tab ? "active" : ""} onClick={() => setView(tab)}>
              {tab === "scope" ? "Scope" : "Spectrum"}
            </button>
          ))}
        </div>
        {draft && <span className="draft-tag" title="The probe follows your edits; the meter follows what is playing">draft, not applied</span>}
        <span className="hint">
          {probe ? `recomputed by the ${client.live ? "engine" : "mock engine"} at frame ${probe.start_frame}` : "probing…"}
        </span>
        <span className="spacer" />
        <button type="button" className="icon-button" aria-label="Close probe" onClick={onClose}><Icon name="close" size={12} /></button>
      </div>
      <div className="probe-body">
        <div className="plot" id="probe-plot" role="tabpanel" aria-label={view === "scope" ? "Scope" : "Spectrum"}>
          {error ? (
            <p className="plot-message error">{error}</p>
          ) : view === "scope" ? (
            <Scope samples={samples} rate={rate} />
          ) : (
            <Spectrum spectrum={spectrum} rate={rate} cutoff={cutoff} />
          )}
          <span className="plot-label">
            {view === "scope" ? `${SCOPE_SECONDS * 1000} ms · triggered · auto-scaled` : "dBFS · 20 Hz – 20 kHz · log"}
          </span>
        </div>
        <dl className="readouts">
          <div><dt>Peak</dt><dd>{formatDb(peak > 0 ? 20 * Math.log10(peak) : null)}FS</dd></div>
          <div><dt>Strongest partial</dt><dd>{spectrum.length ? formatHz((peakBin * rate) / PROBE_FRAMES) : "—"}</dd></div>
          {cutoff && <div><dt>Cutoff</dt><dd>{cutoff.label}</dd></div>}
          <div><dt>Window</dt><dd>{PROBE_FRAMES} frames</dd></div>
        </dl>
      </div>
    </section>
  );
}

function Scope({ samples, rate }: { samples: number[]; rate: number }) {
  const length = Math.round(SCOPE_SECONDS * rate);
  const start = triggerIndex(samples, samples.length - length);
  const trace = samples.slice(start, start + length);
  const peak = trace.reduce((max, sample) => Math.max(max, Math.abs(sample)), 0);
  const scale = peak > 1e-6 ? 0.85 / peak : 1;
  const path = trace
    .map((sample, index) => `${index ? "L" : "M"}${((index / Math.max(1, trace.length - 1)) * 1000).toFixed(1)} ${(70 - sample * scale * 60).toFixed(1)}`)
    .join("");
  return (
    <svg viewBox="0 0 1000 140" preserveAspectRatio="none" role="img" aria-label="Waveform">
      <path d="M0 70H1000" className="grid" vectorEffect="non-scaling-stroke" />
      <path d={path} className="trace" vectorEffect="non-scaling-stroke" />
    </svg>
  );
}

function Spectrum({ spectrum, rate, cutoff }: {
  spectrum: number[];
  rate: number;
  cutoff: ReturnType<typeof lowPassMarker>;
}) {
  const x = (hz: number) => (1000 * Math.log10(Math.min(20_000, Math.max(20, hz)) / 20)) / 3;
  const y = (db: number) => Math.min(136, Math.max(4, 4 + (-db / 96) * 132));
  const bins = spectrum.length;
  let path = "";
  for (let bin = 1; bin < bins; bin++) {
    const hz = (bin * rate) / (bins * 2);
    if (hz < 20 || hz > 20_000) continue;
    path += `${path ? "L" : "M"}${x(hz).toFixed(1)} ${y(spectrum[bin]).toFixed(1)}`;
  }
  return (
    <svg viewBox="0 0 1000 140" preserveAspectRatio="none" role="img" aria-label="Spectrum">
      {[100, 1000, 10_000].map((hz) => (
        <path key={hz} d={`M${x(hz)} 0V140`} className="grid" vectorEffect="non-scaling-stroke" />
      ))}
      {cutoff?.band && (
        <rect x={x(cutoff.band[0])} width={x(cutoff.band[1]) - x(cutoff.band[0])} y="0" height="140" className="band" />
      )}
      {cutoff && <path d={`M${x(cutoff.hz)} 0V140`} className="cutoff" vectorEffect="non-scaling-stroke" />}
      <path d={path} className="trace" vectorEffect="non-scaling-stroke" />
    </svg>
  );
}

function lowPassMarker(graph: GraphSpec, node: DocNode, session: SessionInfo) {
  if (node.kind !== "low_pass") return null;
  const modulated = graph.wires.some((wire) => wire.to.node === node.id && wire.to.port === "cutoff");
  if (modulated) {
    const { base_hz, depth_hz } = session.cutoff_modulation;
    return { hz: base_hz, band: [base_hz - depth_hz, base_hz + depth_hz] as const, label: `${formatHz(base_hz)} ± ${Math.round(depth_hz)} Hz` };
  }
  const hz = Number(node.params.cutoff_hz);
  return { hz, band: null, label: formatHz(hz) };
}
