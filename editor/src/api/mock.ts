import catalogJson from "../../../crates/sonicfusion-graph-api/fixtures/catalog.json";

import { envelopeGate } from "../model/scope";
import type { EngineClient } from "./client";
import type {
  CommandError,
  GraphIssue,
  GraphSpec,
  LevelReading,
  NodeDescriptor,
  NodeSpec,
  ProbeWindow,
  RevisionEvent,
  RevisionUpdate,
  ScopeGate,
  ScopeWindow,
  SessionInfo,
} from "./types";

const MAX_SCOPE_GATE_TRANSITIONS = 1_000;

const INFO: SessionInfo = {
  sample_rate_hz: 48_000,
  batch_frames: 1024,
  cutoff_modulation: { base_hz: 1000, depth_hz: 900 },
};

export interface MockScopeRequest {
  graph: GraphSpec;
  node: string;
  start: number;
  seconds: number;
}

/**
 * A stand-in for the desktop host so the UI can be developed and tested in a
 * browser. It plays nothing. Its checks are a small subset of the engine's and
 * its probes are an approximation; the real rules live in Rust.
 */
export class MockEngine implements EngineClient {
  readonly label = "Mock engine";
  readonly live = false;

  constructor(
    private readonly renderScope: (request: MockScopeRequest) => ScopeWindow | Promise<ScopeWindow> = renderMockScope,
  ) {}

  private revision = 0;
  private session = 0;
  private playing: GraphSpec | null = null;
  private startedAt = 0;
  private timers: ReturnType<typeof setTimeout>[] = [];
  private revisionListeners = new Set<(event: RevisionEvent) => void>();

  async catalog() {
    return CATALOG;
  }

  async sessionInfo() {
    return INFO;
  }

  async validate(graph: GraphSpec) {
    return mockValidate(graph);
  }

  async exportDsl(graph: GraphSpec) {
    throwIfInvalid(graph);
    return `# exported by the mock engine; use the desktop app for real DSL\n# ${graph.nodes.length} nodes, ${graph.wires.length} wires\n`;
  }

  async startSession(graph: GraphSpec) {
    throwIfInvalid(graph);
    this.cancelTimers();
    this.playing = graph;
    this.revision = 0;
    this.startedAt = performance.now();
    return { session: ++this.session, info: INFO };
  }

  async stopSession() {
    this.cancelTimers();
    this.playing = null;
  }

  private cancelTimers() {
    this.timers.forEach(clearTimeout);
    this.timers = [];
  }

  async applyGraph(graph: GraphSpec) {
    if (!this.playing) throw error("no_session", "no session is running");
    throwIfInvalid(graph);
    const revision = ++this.revision;
    const frame = Math.ceil(this.frame() / INFO.batch_frames + 1) * INFO.batch_frames;
    const steps: RevisionUpdate["status"][] = [
      { state: "submitted" },
      { state: "accepted" },
      { state: "scheduled", frame },
      { state: "applied", frame },
    ];
    const session = this.session;
    steps.forEach((status, index) =>
      this.timers.push(
        setTimeout(() => {
          if (status.state === "applied") this.playing = graph;
          this.emit({ session, revision, status });
        }, index * 60),
      ),
    );
    return revision;
  }

  async probe(graph: GraphSpec, node: string, frames: number): Promise<ProbeWindow> {
    const issues = upstreamIssues(graph, node);
    if (issues.length > 0) throw error("invalid", `the graph has ${issues.length} error(s)`, issues);
    const start = this.playing ? this.frame() : 0;
    return {
      node,
      start_frame: start,
      sample_rate_hz: INFO.sample_rate_hz,
      samples: render(graph, node, start, frames),
    };
  }

  async scope(graph: GraphSpec, node: string, seconds: number): Promise<ScopeWindow> {
    const start = this.playing ? this.frame() : 0;
    return this.renderScope({ graph, node, start, seconds });
  }

  async level(frames: number): Promise<LevelReading | null> {
    if (!this.playing) return null;
    const out = this.playing.nodes.find((candidate) => candidate.kind === "output");
    if (!out) return null;
    const frame = this.frame();
    const peak = render(this.playing, out.id, frame, frames).reduce(
      (max, sample) => Math.max(max, Math.abs(sample)),
      0,
    );
    return { frame, frames, peak_dbfs: peak > 0 ? 20 * Math.log10(peak) : null };
  }

  onRevision(listener: (event: RevisionEvent) => void) {
    this.revisionListeners.add(listener);
    return () => void this.revisionListeners.delete(listener);
  }

  onSessionEnded() {
    return () => {};
  }

  private emit(event: RevisionEvent) {
    for (const listener of this.revisionListeners) listener(event);
  }

  private frame() {
    return Math.floor(((performance.now() - this.startedAt) / 1000) * INFO.sample_rate_hz);
  }
}

/** The browser runs long windows in a worker. Unit tests use the same renderer directly. */
export function renderMockScope({ graph, node, start, seconds }: MockScopeRequest): ScopeWindow {
  if (!Number.isFinite(seconds) || seconds <= 0 || seconds > 4) {
    throw error("engine", "scope duration must be greater than 0 and at most 4 seconds");
  }
  const issues = upstreamIssues(graph, node);
  if (issues.length > 0) throw error("invalid", `the graph has ${issues.length} error(s)`, issues);
  const rate = INFO.sample_rate_hz;
  const frames = Math.max(1, Math.round(seconds * rate));
  const envelope = graph.nodes.find((candidate) => candidate.id === node)?.kind === "envelope";
  const gateId = envelope ? envelopeGate(graph, node) : undefined;
  const warmup = envelope ? Math.min(start, 2 * rate) : 0;
  const samples = render(graph, node, start - warmup, warmup + frames).slice(warmup);
  const gateSamples = gateId ? render(graph, gateId, start - warmup, warmup + frames).slice(warmup) : null;
  const bucket = envelope ? Math.ceil(rate / 100) : Math.ceil(frames / 1000);
  const bands: ScopeWindow["bands"] = [];
  for (let begin = 0; begin < frames; begin += bucket) {
    const end = Math.min(begin + bucket, frames);
    let min = 1;
    let max = -1;
    for (let n = begin; n < end; n++) {
      min = Math.min(min, samples[n]);
      max = Math.max(max, samples[n]);
    }
    bands.push([min, max]);
  }
  const gate = gateSamples && scopeGate(gateSamples, bucket);
  return { start_frame: start, sample_rate_hz: rate, frames, bucket_frames: bucket, bands, gate };
}

function error(kind: CommandError["kind"], message: string, issues: GraphIssue[] = []): CommandError {
  return { kind, message, issues };
}

function throwIfInvalid(graph: GraphSpec) {
  const issues = mockValidate(graph).filter((issue) => issue.severity === "error");
  if (issues.length > 0) throw error("invalid", `the graph has ${issues.length} problem(s)`, issues);
}

function issue(code: string, node: string | null, message: string, port: string | null = null): GraphIssue {
  return { severity: "error", code, node, port, param: null, message };
}

function optionalInputs(node: NodeSpec) {
  const descriptor = CATALOG.find(candidate => candidate.kind === node.kind);
  return descriptor?.inputs.filter(port => !port.required) ?? [];
}

function inputNames(node: NodeSpec): string[] {
  switch (node.kind) {
    case "gain":
    case "output":
      return ["in"];
    case "low_pass":
      return ["in"];
    case "envelope":
      return ["in", "gate"];
    case "melody":
      return ["advance"];
    case "mix":
      return (node.params.weights as number[]).map((_, index) => `in.${index}`);
    default:
      return [];
  }
}

function envelopeParamIssues(node: NodeSpec): GraphIssue[] {
  if (node.kind !== "envelope") return [];
  const issues: GraphIssue[] = [];
  for (const name of ["attack_seconds", "decay_seconds", "sustain_level", "release_seconds"]) {
    const value = node.params[name];
    const sustain = name === "sustain_level";
    if (typeof value !== "number" || !Number.isFinite(value) || value < 0 || (sustain && value > 1)) {
      const message = sustain ? "sustain must be between 0 and 1" : `${name.split("_")[0]} must be a finite nonnegative time`;
      issues.push({ ...issue("invalid_parameter", node.id, message), param: name });
    }
  }
  return issues;
}

/** Unwired inputs and invalid envelope settings in the selected branch. */
function upstreamIssues(graph: GraphSpec, node: string): GraphIssue[] {
  const feeds = new Map(graph.wires.map((wire) => [`${wire.to.node}:${wire.to.port}`, wire.from]));
  const issues: GraphIssue[] = [];
  const seen = new Set<string>();
  const pending = [node];
  while (pending.length > 0) {
    const id = pending.pop()!;
    if (seen.has(id)) continue;
    seen.add(id);
    const spec = graph.nodes.find((candidate) => candidate.id === id);
    if (!spec) continue;
    issues.push(...envelopeParamIssues(spec));
    for (const port of inputNames(spec)) {
      const from = feeds.get(`${id}:${port}`);
      if (from) pending.push(from);
      else issues.push(issue("missing_input", id, `input "${port}" is not connected`, port));
    }
    for (const { name } of optionalInputs(spec)) {
      const from = feeds.get(`${id}:${name}`);
      if (from) pending.push(from);
    }
  }
  return issues;
}

function mockValidate(graph: GraphSpec): GraphIssue[] {
  const issues: GraphIssue[] = [];
  const outputs = graph.nodes.filter((node) => node.kind === "output");
  if (outputs.length !== 1) {
    issues.push(issue("no_output", null, "the graph needs exactly one output node"));
    return issues;
  }
  const feeds = new Map(graph.wires.map((wire) => [`${wire.to.node}:${wire.to.port}`, wire.from]));
  const reachable = new Set<string>();
  const pending = [outputs[0].id];
  while (pending.length > 0) {
    const id = pending.pop()!;
    if (reachable.has(id)) continue;
    reachable.add(id);
    const node = graph.nodes.find((candidate) => candidate.id === id);
    if (!node) continue;
    for (const port of inputNames(node)) {
      const from = feeds.get(`${id}:${port}`);
      if (from) pending.push(from);
      else issues.push(issue("missing_input", id, `input "${port}" is not connected`, port));
    }
    for (const { name } of optionalInputs(node)) {
      const from = feeds.get(`${id}:${name}`);
      if (from) pending.push(from);
    }
  }
  for (const node of graph.nodes) {
    if (!reachable.has(node.id)) {
      issues.push({ ...issue("unreachable", node.id, "not connected to the output; it won't be played"), severity: "warning" });
    }
    const wet = node.params.wet ?? node.params.mix;
    if (node.kind === "low_pass" && wet !== undefined && (typeof wet !== "number" || !(wet >= 0 && wet <= 1))) {
      issues.push({
        ...issue("invalid_parameter", node.id, "Dry/Wet must be between 0% and 100%"),
        param: "wet",
        severity: reachable.has(node.id) ? "error" : "warning",
      });
    }
    for (const name of ["frequency_hz", "cutoff_hz"]) {
      const value = node.params[name];
      if (typeof value === "number" && value >= INFO.sample_rate_hz / 2) {
        issues.push({ ...issue("invalid_parameter", node.id, `${name === "cutoff_hz" ? "cutoff" : "frequency"} must be below Nyquist (24000 Hz at 48000 Hz)`), param: name });
      }
    }
    const severity: GraphIssue["severity"] = reachable.has(node.id) ? "error" : "warning";
    if (node.kind === "constant") {
      const value = node.params.value;
      if (typeof value !== "number" || !Number.isFinite(value) || value < -1 || value > 1) {
        issues.push({ ...issue("invalid_parameter", node.id, "constant value must be finite and between -1 and 1"), param: "value", severity });
      }
    }
    issues.push(...envelopeParamIssues(node).map((problem) => ({ ...problem, severity })));
  }
  return issues;
}

/** A rough model of the engine's nodes for drawing a probe. */
function render(graph: GraphSpec, root: string, start: number, frames: number): number[] {
  const byId = new Map(graph.nodes.map((node) => [node.id, node]));
  const feeds = new Map(graph.wires.map((wire) => [`${wire.to.node}:${wire.to.port}`, wire.from]));
  const cache = new Map<string, Float32Array>();
  const seeds = noiseSeeds(byId);
  const signal = (id: string): Float32Array => {
    const cached = cache.get(id);
    if (cached) return cached;
    const node = byId.get(id);
    const out = new Float32Array(frames);
    const input = (port: string) => {
      const from = feeds.get(`${id}:${port}`);
      return from ? signal(from) : new Float32Array(frames);
    };
    const frequencyControl = (node?.kind === "sawtooth" || node?.kind === "sine") && feeds.has(`${id}:frequency`) ? input("frequency") : null;
    const baseHz = Number(node?.params.frequency_hz ?? 0);
    // PROTOTYPE: a connected Note input replaces the base frequency (v → MIDI 60 + 60·v); FM still scales it.
    const noteControl = (node?.kind === "sawtooth" || node?.kind === "sine") && feeds.has(`${id}:note`) ? input("note") : null;
    const melodyNotes = node?.kind === "melody" ? melodyPitch(input("advance"), node.params, frames) : null;
    let modulatedPhase = (start * baseHz / INFO.sample_rate_hz) % 1;
    const p = (name: string) => Number(node?.params[name] ?? 0);
    if (node?.kind === "envelope") {
      const samples = envelopeSamples(input("in"), input("gate"), node.params);
      cache.set(id, samples);
      return samples;
    }
    const noiseKey = mix32(seeds.get(id) ?? 0);
    const wetRatio = Number(node?.params.wet ?? node?.params.mix ?? 1);
    let wet = 0;
    for (let n = 0; n < frames && node; n++) {
      const t = (start + n) / INFO.sample_rate_hz;
      const phase = (p("frequency_hz") * t) % 1;
      switch (node.kind) {
        case "square":
          out[n] = phase < p("pulse_width") ? 1 : -1;
          break;
        case "sine":
        case "sawtooth": {
          const currentPhase = frequencyControl || noteControl ? modulatedPhase : phase;
          out[n] = node.kind === "sine" ? Math.sin(2 * Math.PI * currentPhase) : 2 * currentPhase - 1;
          if (frequencyControl || noteControl) {
            const nyquist = INFO.sample_rate_hz / 2 * (1 - Number.EPSILON / 2);
            const base = noteControl ? 440 * 2 ** ((60 + 60 * Math.max(-1, Math.min(1, noteControl[n])) - 69) / 12) : baseHz;
            const hz = Math.min(base * 2 ** (frequencyControl?.[n] ?? 0), nyquist);
            modulatedPhase = (modulatedPhase + hz / INFO.sample_rate_hz) % 1;
          }
          break;
        }
        case "constant":
          out[n] = p("value");
          break;
        case "melody":
          out[n] = melodyNotes![n];
          break;
        case "noise":
          out[n] = whiteNoise(start + n, noiseKey);
          break;
        case "gain":
          out[n] = clamp(input("in")[n] * p("factor"));
          break;
        case "mix":
          out[n] = clamp(
            (node.params.weights as number[]).reduce((sum, weight, index) => sum + weight * input(`in.${index}`)[n], 0),
          );
          break;
        case "output":
          out[n] = input("in")[n];
          break;
        case "low_pass": {
          const control = feeds.has(`${id}:cutoff`) ? input("cutoff")[n] : null;
          const cutoff = control === null ? p("cutoff_hz") : INFO.cutoff_modulation.base_hz + INFO.cutoff_modulation.depth_hz * control;
          const a = 1 - Math.exp((-2 * Math.PI * cutoff) / INFO.sample_rate_hz);
          const dry = input("in")[n];
          wet = Math.fround(wet * (1 - a) + a * dry);
          if (wetRatio === 0) out[n] = dry;
          else if (wetRatio === 1) out[n] = wet;
          else out[n] = (1 - wetRatio) * dry + wetRatio * wet;
          break;
        }
      }
    }
    cache.set(id, out);
    return out;
  };
  return Array.from(signal(root));
}

function scopeGate(samples: readonly number[], bucket: number): ScopeGate {
  const start_open = samples[0] > 0;
  const transitions: number[] = [];
  let open = start_open;
  for (let i = 1; i < samples.length; i++) {
    const nextOpen = samples[i] > 0;
    if (open === nextOpen) continue;
    open = nextOpen;
    transitions.push(i);
    if (transitions.length > MAX_SCOPE_GATE_TRANSITIONS) break;
  }
  if (transitions.length <= MAX_SCOPE_GATE_TRANSITIONS) return { kind: "edges", start_open, transitions };

  const states: ("closed" | "open" | "mixed")[] = [];
  for (let begin = 0; begin < samples.length; begin += bucket) {
    const end = Math.min(begin + bucket, samples.length);
    const firstOpen = samples[begin] > 0;
    let state: (typeof states)[number] = firstOpen ? "open" : "closed";
    for (let i = begin + 1; i < end; i++) {
      if ((samples[i] > 0) !== firstOpen) {
        state = "mixed";
        break;
      }
    }
    states.push(state);
  }
  return { kind: "bands", states };
}

function envelopeSamples(audio: Float32Array, gate: Float32Array, params: NodeSpec["params"]): Float32Array {
  type Stage = "attack" | "decay" | "sustain" | "release" | "idle";
  const frames = (name: string) => Math.ceil(Number(params[name]) * INFO.sample_rate_hz);
  const attack = Number(params.attack_seconds) * INFO.sample_rate_hz;
  const decay = frames("decay_seconds");
  const sustain = Number(params.sustain_level);
  const release = frames("release_seconds");
  let open = false;
  let level = 0;
  let initial = 0;
  let elapsed = 0;
  let stage: Stage = "idle";

  const enter = (next: Stage) => {
    stage = next;
    initial = level;
    elapsed = 0;
  };
  return audio.map((sample, index) => {
    const nextOpen = gate[index] > 0;
    if (nextOpen !== open) {
      open = nextOpen;
      enter(open ? "attack" : "release");
    }
    while (stage !== "idle" && stage !== "sustain") {
      const attackDuration = initial === 1 ? 0 : attack * (1 - initial);
      const duration = stage === "attack" ? attackDuration : stage === "decay" ? decay : release;
      const target = stage === "attack" ? 1 : stage === "decay" ? sustain : 0;
      const next = stage === "attack" ? "decay" : stage === "decay" ? "sustain" : "idle";
      if (duration === 0) {
        level = target;
        enter(next);
        continue;
      }
      elapsed += 1;
      if (elapsed >= duration) {
        level = target;
        enter(next);
      } else {
        level = initial + (target - initial) * (elapsed / duration);
      }
      break;
    }
    return sample * level;
  });
}

function noiseSeeds(nodes: ReadonlyMap<string, NodeSpec>): Map<string, number> {
  const ids = Array.from(nodes.values())
    .filter((node) => node.kind === "noise")
    .map((node) => node.id)
    .sort();
  const used = new Set<number>();
  const seeds = new Map<string, number>();
  const encoder = new TextEncoder();
  for (const id of ids) {
    let hash = 0x811c9dc5;
    for (const byte of encoder.encode(id)) {
      hash = Math.imul(hash ^ byte, 0x01000193);
    }
    let seed = hash >>> 0;
    while (used.has(seed)) seed = (seed + 1) >>> 0;
    used.add(seed);
    seeds.set(id, seed);
  }
  return seeds;
}

// Hash the absolute position so overlapping mock probes draw the same signal.
function whiteNoise(frame: number, key: number): number {
  const counter = frame ^ Math.imul(Math.floor(frame / 2 ** 32), 0x9e3779b9);
  return (mix32(counter ^ key) >>> 8) / 2 ** 23 - 1;
}

function mix32(value: number): number {
  value = Math.imul(value ^ (value >>> 16), 0x21f0aaad);
  value = Math.imul(value ^ (value >>> 15), 0x735a2d97);
  return (value ^ (value >>> 15)) >>> 0;
}

function clamp(value: number) {
  return Math.max(-1, Math.min(1, value));
}

/** PROTOTYPE: the Note input and a minimal Melody source; neither exists in the engine. */
const CATALOG: NodeDescriptor[] = prototypeCatalog(catalogJson as NodeDescriptor[]);

function prototypeCatalog(catalog: NodeDescriptor[]): NodeDescriptor[] {
  // After FM, so dropped control wires keep auto-connecting to FM.
  const note = { name: "note", label: "Note", signal: "control" as const, required: false };
  const withNote = catalog.map((node) => (node.kind === "sine" || node.kind === "sawtooth" ? { ...node, inputs: [...node.inputs, note] } : node));
  const output = withNote.findIndex((node) => node.kind === "output");
  return [...withNote.slice(0, output), {
    kind: "melody",
    label: "Melody",
    category: "source",
    signature: "melody(notes, advance)",
    inputs: [{ name: "advance", label: "Advance", signal: "control", required: true }],
    variadic_input: null,
    has_output: true,
    params: [],
  } as NodeDescriptor, ...withNote.slice(output)];
}

/** Next note on each rising edge of `advance`; holds between edges and loops. */
function melodyPitch(advance: Float32Array, params: Record<string, unknown>, frames: number): Float32Array {
  const notes = (params.notes as number[] | undefined) ?? [60];
  const out = new Float32Array(frames);
  let index = -1, high = false;
  for (let n = 0; n < frames; n++) {
    const nowHigh = advance[n] > 0;
    if (nowHigh && !high) index = (index + 1) % notes.length;
    high = nowHigh;
    out[n] = (notes[Math.max(0, index)] - 60) / 60;
  }
  return out;
}
