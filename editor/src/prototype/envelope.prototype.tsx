/**
 * PROTOTYPE — throwaway. Envelope node (#34) layouts and a slow scope.
 *
 * `?env=` A (four slider rows) · B (curve editor) · C (2×2 compact sliders)
 * `?scenario=gate` seeds a gated patch: square gate → envelope, saw → low-pass → envelope.
 */
import { useRef } from "react";

import type { GraphSpec, NodeDescriptor, ParamDescriptor } from "../api/types";
import type { DocNode, EditorDocument } from "../model/document";
import { ParamControl, ParameterRange, formatValue, fromSlider, toSlider } from "../components/ParamControl";
import { useDocNodes, useNodeLabel, useVariant } from "./design-33-41.prototype";

export function formatSeconds(seconds: number): string {
  if (seconds < 1) return `${Number((seconds * 1000).toPrecision(3))} ms`;
  return `${Number(seconds.toPrecision(3))} s`;
}

/** The patch the gate designs are judged against. */
export function gateScenario(doc: EditorDocument): EditorDocument {
  const node = (id: string, kind: DocNode["kind"], params: DocNode["params"], x: number, y: number, displayName?: string): DocNode =>
    ({ id, kind, params, position: { x, y }, ...(displayName ? { displayName } : {}) });
  return {
    ...doc,
    name: "gated-bass",
    nodes: [
      node("saw", "sawtooth", { frequency_hz: 110 }, 32, 40, "Bass"),
      node("lfo", "sine", { frequency_hz: 0.25 }, 32, 196, "LFO"),
      node("lpf", "low_pass", { cutoff_hz: 800, wet: 0.7 }, 272, 40),
      node("clock", "square", { frequency_hz: 2, pulse_width: 0.5 }, 272, 300, "Clock"),
      node("env", "envelope", { attack_s: 0.01, decay_s: 0.15, sustain: 0.6, release_s: 0.12 }, 512, 40),
      node("gain", "gain", { factor: 0.3 }, 752, 40),
      node("out", "output", {}, 992, 40),
    ],
    wires: [
      { from: "saw", to: { node: "lpf", port: "in" } },
      { from: "lfo", to: { node: "lpf", port: "cutoff" } },
      { from: "lpf", to: { node: "env", port: "in" } },
      { from: "clock", to: { node: "env", port: "gate" } },
      { from: "env", to: { node: "gain", port: "in" } },
      { from: "gain", to: { node: "out", port: "in" } },
    ],
  };
}

interface BodyProps {
  node: DocNode;
  descriptor: NodeDescriptor;
  gateFrom: string | undefined;
  gateHandle: (top: number) => React.ReactNode;
  rowTop: (index: number) => number;
  paramIssue: (name: string) => string | undefined;
  setParam: (name: string, value: number, continuous: boolean) => void;
  beginContinuous: () => void;
}

/** Rows and handles for an Envelope node; the gate row comes first so its port sits at a fixed height. */
export function EnvelopeBody(props: BodyProps): { rows: React.ReactNode[]; handles: React.ReactNode[] } {
  const { node, descriptor, gateFrom, gateHandle, rowTop } = props;
  return {
    rows: [
      <GateRow key="gate" node={node} gateFrom={gateFrom} />,
      <EnvelopeControls key="controls" {...props} />,
    ],
    handles: [gateHandle(rowTop(0))],
  };
  void descriptor;
}

function GateRow({ node, gateFrom }: { node: DocNode; gateFrom: string | undefined }) {
  const label = useNodeLabel();
  const nodes = useDocNodes();
  return (
    <div className={`param proto-gate${gateFrom ? "" : " invalid"}`}>
      <div className="param-top">
        <span className="proto-gate-label">Gate</span>
        <EnvelopeThumb node={node} />
        <span className="proto-gate-rule" title="The gate opens when the signal rises above 0 and closes at or below 0">opens &gt; 0</span>
      </div>
      <p className="param-note proto-gate-note">
        {gateFrom ? <>from {label(nodes.get(gateFrom), gateFrom)}</> : "not connected · output stays silent"}
      </p>
    </div>
  );
}

function EnvelopeControls({ node, descriptor, paramIssue, setParam, beginContinuous }: BodyProps) {
  const variant = useVariant("env");
  const value = (name: string) => Number(node.params[name] ?? descriptor.params.find((param) => param.name === name)?.default);
  if (variant === "B") return <CurveEditor node={node} value={value} setParam={setParam} beginContinuous={beginContinuous} />;
  if (variant === "C") {
    const pairs = [descriptor.params.slice(0, 2), descriptor.params.slice(2, 4)];
    return (
      <>
        {pairs.map((pair) => (
          <div key={pair[0].name} className="proto-pair">
            {pair.map((param) => (
              <CompactParam key={param.name} id={`${node.id}-${param.name}`} descriptor={param} value={value(param.name)}
                invalid={paramIssue(param.name)} beginContinuous={beginContinuous}
                onChange={(next, continuous) => setParam(param.name, next, continuous)} />
            ))}
          </div>
        ))}
      </>
    );
  }
  return (
    <>
      {descriptor.params.map((param) => (
        <ParamControl key={param.name} id={`${node.id}-${param.name}`} descriptor={param} value={value(param.name)}
          invalid={paramIssue(param.name)} onBeginContinuous={beginContinuous}
          onChange={(next, continuous) => setParam(param.name, next, continuous)} />
      ))}
    </>
  );
}

function CompactParam({ id, descriptor, value, invalid, onChange, beginContinuous }: {
  id: string;
  descriptor: ParamDescriptor;
  value: number;
  invalid?: string;
  onChange: (value: number, continuous: boolean) => void;
  beginContinuous: () => void;
}) {
  return (
    <div className={`proto-compact${invalid ? " invalid" : ""}`} title={invalid}>
      <div className="param-top">
        <label htmlFor={id}>{descriptor.label}</label>
        <span className="value">{formatValue(descriptor, value).replace(" ", "\u202F")}</span>
      </div>
      <ParameterRange id={id} min={0} max={1000} step="any" position={toSlider(descriptor, value)}
        valueText={formatValue(descriptor, value)} onBeginContinuous={beginContinuous}
        onMove={(position) => onChange(fromSlider(descriptor, position), true)} />
    </div>
  );
}

// ---------------------------------------------------------------- curve

/** Segment widths grow with log time so 1 ms and 10 s both stay visible. */
const width = (seconds: number) => Math.log10(1 + seconds / 0.001);
const fromWidth = (units: number) => 0.001 * (10 ** Math.max(0, units) - 1);
const SUSTAIN_UNITS = 1.5;

function shape(a: number, d: number, s: number, r: number) {
  const wa = width(a), wd = width(d), wr = width(r);
  const total = Math.max(8, wa + wd + SUSTAIN_UNITS + wr);
  return { wa, wd, wr, total, s };
}

function curvePath(a: number, d: number, s: number, r: number, w: number, h: number, pad = 0) {
  const g = shape(a, d, s, r);
  const x = (units: number) => pad + (units / g.total) * (w - 2 * pad);
  const y = (level: number) => pad + (1 - level) * (h - 2 * pad);
  const x1 = x(g.wa), x2 = x(g.wa + g.wd), x3 = x(g.wa + g.wd + SUSTAIN_UNITS), x4 = x(g.wa + g.wd + SUSTAIN_UNITS + g.wr);
  return { d: `M${x(0)} ${y(0)}L${x1} ${y(1)}L${x2} ${y(s)}L${x3} ${y(s)}L${x4} ${y(0)}`, x, y, x1, x2, x3, x4, g };
}

function EnvelopeThumb({ node }: { node: DocNode }) {
  const variant = useVariant("env");
  if (variant !== "C") return null;
  const p = (name: string) => Number(node.params[name]);
  const { d } = curvePath(p("attack_s"), p("decay_s"), p("sustain"), p("release_s"), 56, 16, 1);
  return (
    <svg className="proto-thumb" width="56" height="16" viewBox="0 0 56 16" aria-hidden="true">
      <path d={d} />
    </svg>
  );
}

const tight = (text: string) => text.replace(" ", "\u202F");
const W = 168;
const H = 72;

function CurveEditor({ node, value, setParam, beginContinuous }: {
  node: DocNode;
  value: (name: string) => number;
  setParam: (name: string, value: number, continuous: boolean) => void;
  beginContinuous: () => void;
}) {
  const svg = useRef<SVGSVGElement>(null);
  const a = value("attack_s"), d = value("decay_s"), s = value("sustain"), r = value("release_s");
  const c = curvePath(a, d, s, r, W, H, 6);
  const toLocal = (event: { clientX: number; clientY: number }) => {
    const box = svg.current!.getBoundingClientRect();
    return { x: ((event.clientX - box.left) / box.width) * W, y: ((event.clientY - box.top) / box.height) * H };
  };
  const units = (px: number) => ((px - 6) / (W - 12)) * c.g.total;
  const clampTime = (seconds: number) => Math.min(10, Math.max(0.001, Number(seconds.toPrecision(3))));
  const apply = (which: "attack" | "decay" | "release", event: PointerEvent | React.PointerEvent) => {
    const { x, y } = toLocal(event);
    const u = units(x);
    if (which === "attack") setParam("attack_s", clampTime(fromWidth(u)), true);
    if (which === "decay") {
      setParam("decay_s", clampTime(fromWidth(u - c.g.wa)), true);
      setParam("sustain", Math.min(1, Math.max(0, Number((1 - (y - 6) / (H - 12)).toFixed(2)))), true);
    }
    if (which === "release") setParam("release_s", clampTime(fromWidth(u - c.g.wa - c.g.wd - SUSTAIN_UNITS)), true);
  };
  const drag = (which: "attack" | "decay" | "release") => ({
    onPointerDown: (event: React.PointerEvent) => {
      event.stopPropagation();
      event.preventDefault();
      beginContinuous();
      const move = (next: PointerEvent) => apply(which, next);
      const up = () => {
        window.removeEventListener("pointermove", move);
        window.removeEventListener("pointerup", up);
      };
      window.addEventListener("pointermove", move);
      window.addEventListener("pointerup", up);
    },
  });
  return (
    <div className="proto-curve nodrag nowheel">
      <svg ref={svg} width={W} height={H} viewBox={`0 0 ${W} ${H}`} role="img"
        aria-label={`Envelope: attack ${formatSeconds(a)}, decay ${formatSeconds(d)}, sustain ${s.toFixed(2)}, release ${formatSeconds(r)}`}>
        <path d={`M6 ${H - 6}H${W - 6}`} className="proto-curve-axis" />
        <path d={c.d + `L${c.x(0)} ${c.y(0)}Z`} className="proto-curve-fill" />
        <path d={c.d} className="proto-curve-line" />
        <path d={`M${c.x2} ${c.y(s)}H${c.x3}`} className="proto-curve-sustain" />
        <circle cx={c.x1} cy={c.y(1)} r="5" className="proto-handle" {...drag("attack")}><title>Attack: drag sideways</title></circle>
        <circle cx={c.x2} cy={c.y(s)} r="5" className="proto-handle" {...drag("decay")}><title>Decay and sustain: drag sideways and up/down</title></circle>
        <circle cx={c.x4} cy={c.y(0)} r="5" className="proto-handle" {...drag("release")}><title>Release: drag sideways</title></circle>
      </svg>
      <div className="proto-curve-values">
        <span><b>A</b>{tight(formatSeconds(a))}</span>
        <span><b>D</b>{tight(formatSeconds(d))}</span>
        <span><b>S</b>{s.toFixed(2)}</span>
        <span><b>R</b>{tight(formatSeconds(r))}</span>
      </div>
    </div>
  );
  void node;
}

// ---------------------------------------------------------------- slow scope

/** Envelope nodes need a window of about two gate periods, not 43 ms. */
export function probeFrames(graph: GraphSpec, node: DocNode, rate: number, fallback: number): number {
  // A sub-audio oscillator (an LFO or a gate clock) also needs a couple of its own cycles.
  const own = Number(node.params.frequency_hz ?? 0);
  if (own > 0 && own < 20) return Math.round(Math.min(4, Math.max(0.25, 2 / own)) * rate);
  // PROTOTYPE sequencer: one bar, from the internal tempo or 16 clock periods.
  if (node.kind === "sequencer") {
    const clock = graph.wires.find((wire) => wire.to.node === node.id && wire.to.port === "clock")?.from;
    const hz = Number(graph.nodes.find((candidate) => candidate.id === clock)?.params.frequency_hz ?? 0);
    const seconds = hz > 0 ? 16 / hz : (4 * 60) / Number(node.params.tempo_bpm ?? 120);
    return Math.round(Math.min(8, seconds) * rate);
  }
  if (node.kind !== "envelope") return fallback;
  const gate = gateSource(graph, node.id);
  const hz = Number(gate?.params.frequency_hz ?? 0);
  const seconds = hz > 0 ? Math.min(4, Math.max(0.25, 2 / hz)) : 1;
  return Math.round(seconds * rate);
}

export function gateSource(graph: GraphSpec, id: string) {
  const from = graph.wires.find((wire) => wire.to.node === id && wire.to.port === "gate")?.from;
  return graph.nodes.find((candidate) => candidate.id === from);
}

/** Peak level per 10 ms, mirrored, so the envelope's outline reads at any audio pitch; the gate is dashed. */
export function SlowScope({ samples, gate, rate, outline }: { samples: number[]; gate: number[]; rate: number; outline: boolean }) {
  if (samples.length === 0) return <svg viewBox="0 0 1000 140" role="img" aria-label="No signal" />;
  if (!outline) {
    // A slow oscillator: its waveform, one point per pixel column.
    const step = Math.max(1, Math.floor(samples.length / 1000));
    const points = samples.filter((_, index) => index % step === 0);
    const wave = points.map((sample, index) => `${index ? "L" : "M"}${((index / (points.length - 1)) * 1000).toFixed(1)} ${(70 - sample * 0.85 * 60).toFixed(1)}`).join("");
    return (
      <svg viewBox="0 0 1000 140" preserveAspectRatio="none" role="img" aria-label={`Waveform over ${(samples.length / rate).toFixed(2)} s`}>
        <path d="M0 70H1000" className="grid" vectorEffect="non-scaling-stroke" />
        <path d={wave} className="trace" vectorEffect="non-scaling-stroke" />
      </svg>
    );
  }
  const per = Math.max(1, Math.round(rate * 0.01));
  const columns = Math.max(2, Math.ceil(samples.length / per));
  const peaks = Array.from({ length: columns }, (_, column) => {
    let peak = 0;
    for (let n = column * per; n < Math.min(samples.length, (column + 1) * per); n++) peak = Math.max(peak, Math.abs(samples[n]));
    return peak;
  });
  const top = Math.max(...peaks);
  const scale = top > 1e-6 ? 0.85 / top : 1;
  const x = (column: number) => ((column / (columns - 1)) * 1000).toFixed(1);
  const upper = peaks.map((peak, column) => `${column ? "L" : "M"}${x(column)} ${(70 - peak * scale * 60).toFixed(1)}`).join("");
  const lower = peaks.map((peak, column) => `L${x(column)} ${(70 + peak * scale * 60).toFixed(1)}`).reverse().join("");
  const gatePath = gate.length
    ? peaks.map((_, column) => `${column ? "L" : "M"}${x(column)} ${gate[Math.min(gate.length - 1, column * per + (per >> 1))] > 0 ? 8 : 132}`).join("")
    : "";
  return (
    <svg viewBox="0 0 1000 140" preserveAspectRatio="none" role="img" aria-label={`Envelope output over ${(samples.length / rate).toFixed(2)} s`}>
      <path d="M0 70H1000" className="grid" vectorEffect="non-scaling-stroke" />
      <path d={upper + lower + "Z"} className="proto-band-trace" vectorEffect="non-scaling-stroke" />
      {gatePath && <path d={gatePath} className="proto-gate-trace" vectorEffect="non-scaling-stroke" />}
    </svg>
  );
}
