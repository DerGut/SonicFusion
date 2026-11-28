import { useEffect, useMemo, useRef, useState } from "react";

import type { EngineClient } from "../api/client";
import { type GraphSpec, type ProbeWindow, type ScopeWindow, type SessionInfo, isCommandError } from "../api/types";
import type { DocNode } from "../model/document";
import { noteName, pitchFromHz } from "../model/pitch";
import { slowScopeSeconds } from "../model/scope";
import { formatDb, formatHz, formatSeconds, spectrumDb, strongestPartial, triggerIndex } from "../model/signal";
import { describe } from "../state/useEngineSession";
import { Icon } from "./icons";
import { NodeTitle } from "./NodeTitle";
import { SlowScope } from "./SlowScope";

const PROBE_FRAMES = 2048;
const LIVE_INTERVAL_MS = 200;
const IDLE_DEBOUNCE_MS = 150;
const SCOPE_SECONDS = 0.02;

type Preview = { kind: "short"; window: ProbeWindow } | { kind: "slow"; seconds: number; window: ScopeWindow };

/**
 * Scope and spectrum of the selected node. The engine recomputes the node's
 * output from the draft at the session's current frame; nothing is tapped
 * from the device stream.
 */
export function ProbePanel({ client, graph, node, playing, draft, session, onRename, onClose }: {
  client: EngineClient;
  graph: GraphSpec;
  node: DocNode;
  playing: boolean;
  /** The draft differs from what is playing, so the probe shows unapplied edits. */
  draft: boolean;
  session: SessionInfo;
  onRename: (id: string, name: string) => void;
  onClose: () => void;
}) {
  const [view, setView] = useState<"scope" | "spectrum">("scope");
  const [preview, setPreview] = useState<Preview | null>(null);
  const inFlight = useRef<Promise<Preview> | null>(null);
  const slowSeconds = view === "scope" ? slowScopeSeconds(graph, node) : null;
  /** Why there is no signal: an unfinished branch is normal while editing. */
  const [unavailable, setUnavailable] = useState<{ text: string; problem: boolean } | null>(null);
  const graphJson = JSON.stringify(graph);

  useEffect(() => {
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout>;
    // One probe at a time; while playing, the next starts after the last returns.
    setUnavailable(null);
    const run = async () => {
      // A draft or tab change can rerun this effect while a previous command is
      // still rendering. Let it finish before sending another window to the host.
      if (inFlight.current) await inFlight.current.catch(() => {});
      if (cancelled) return;
      const draft = JSON.parse(graphJson) as GraphSpec;
      const request: Promise<Preview> = slowSeconds === null
        ? client.probe(draft, node.id, PROBE_FRAMES).then((window) => ({ kind: "short", window }))
        : client.scope(draft, node.id, slowSeconds).then((window) => ({ kind: "slow", seconds: slowSeconds, window }));
      inFlight.current = request;
      try {
        const result = await request;
        if (cancelled) return;
        setPreview(result);
        setUnavailable(null);
      } catch (reason) {
        if (cancelled) return;
        setPreview(null);
        setUnavailable(whyUnavailable(reason));
      } finally {
        if (inFlight.current === request) inFlight.current = null;
        if (!cancelled && playing) timer = setTimeout(run, LIVE_INTERVAL_MS);
      }
    };
    timer = setTimeout(run, playing ? 0 : IDLE_DEBOUNCE_MS);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [client, graphJson, node.id, playing, slowSeconds]);

  const probe = slowSeconds === null && preview?.kind === "short" ? preview.window : null;
  const slow = preview?.kind === "slow" && preview.seconds === slowSeconds ? preview.window : null;
  const displayed = probe ?? slow;
  const samples = useMemo(() => probe?.samples ?? [], [probe]);
  const peak = slow
    ? slow.bands.reduce((peak, [min, max]) => Math.max(peak, -min, max), 0)
    : samples.reduce((max, sample) => Math.max(max, Math.abs(sample)), 0);
  const rate = displayed?.sample_rate_hz ?? session.sample_rate_hz;
  const slowFrames = slow?.frames ?? (slowSeconds === null ? 0 : Math.round(slowSeconds * rate));
  const spectrum = useMemo(() => (samples.length === PROBE_FRAMES ? spectrumDb(samples) : []), [samples]);
  const partial = strongestPartial(spectrum, rate);
  const partialPitch = view === "spectrum" && partial?.canNamePitch ? pitchFromHz(partial.hz) : null;
  const partialLabel = partial ? (partial.canNamePitch ? "" : "≈") + formatHz(partial.hz) : "—";
  const cutoff = lowPassMarker(graph, node, session);
  const wet = node.params.wet ?? 1;
  const dryFloor = view === "spectrum" && cutoff && typeof wet === "number" && wet >= 0 && wet < 1
    ? formatDb(20 * Math.log10(1 - wet))
    : null;

  return (
    <section className="probe" aria-label="Signal probe">
      <div className="probe-head">
        <Icon name="probe" />
        <NodeTitle node={node} onRename={onRename} inProbe />
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
          {displayed
            ? `rendered by the ${client.live ? "engine" : "mock engine"} at frame ${displayed.start_frame} · drawn here`
            : unavailable
              ? "no signal"
              : "probing…"}
        </span>
        <span className="spacer" />
        <button type="button" className="icon-button" aria-label="Close probe" onClick={onClose}><Icon name="close" size={12} /></button>
      </div>
      <div className="probe-body">
        <div className="plot" id="probe-plot" role="tabpanel" aria-label={view === "scope" ? "Scope" : "Spectrum"}>
          {unavailable ? (
            <p className={`plot-message${unavailable.problem ? " error" : ""}`}>{unavailable.text}</p>
          ) : view === "scope" ? (
            slowSeconds === null ? <Scope samples={samples} rate={rate} /> : slow && <SlowScope scope={slow} />
          ) : (
            <Spectrum spectrum={spectrum} rate={rate} cutoff={cutoff} />
          )}
          <span className="plot-label">
            {view === "scope"
              ? slowSeconds === null
                ? `${SCOPE_SECONDS * 1000} ms · triggered · auto-scaled`
                : node.kind === "envelope"
                  ? `${formatSeconds(slowFrames / rate)} · ${slow?.gate?.kind === "bands" ? "mixed gate shaded" : "gate dashed"} · peak per 10 ms`
                  : `${formatSeconds(slowFrames / rate)} · waveform`
              : "dBFS · 20 Hz – 20 kHz · log"}
          </span>
        </div>
        <dl className="readouts">
          <div><dt>Peak</dt><dd>{displayed ? `${formatDb(peak > 0 ? 20 * Math.log10(peak) : null)}FS` : "—"}</dd></div>
          <div hidden={slowSeconds !== null}>
            <dt>Strongest partial</dt>
            <dd>{partialLabel}{partialPitch && " · " + noteName(partialPitch.midi)}</dd>
          </div>
          {dryFloor && (
            <div title="Asymptotic floor set by the dry signal well above the cutoff">
              <dt>Above cutoff</dt>
              <dd>{dryFloor} floor<br /><small>relative to input</small></dd>
            </div>
          )}
          {cutoff && <div><dt>Cutoff</dt><dd>{cutoff.label}</dd></div>}
          <div><dt>Window</dt><dd>{slowSeconds === null ? PROBE_FRAMES : slowFrames} frames</dd></div>
        </dl>
      </div>
    </section>
  );
}

/** Turn a probe failure into a message; missing inputs are just unfinished work. */
function whyUnavailable(reason: unknown): { text: string; problem: boolean } {
  if (isCommandError(reason)) {
    const missing = reason.issues.find((issue) => issue.code === "missing_input");
    if (missing?.node) {
      return { text: `Connect ${missing.node}'s "${missing.port ?? "in"}" input to see this signal.`, problem: false };
    }
  }
  return { text: describe(reason), problem: true };
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
