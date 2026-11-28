import catalogJson from "../../../crates/sonicfusion-graph-api/fixtures/catalog.json";

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
  SessionInfo,
} from "./types";

const INFO: SessionInfo = {
  sample_rate_hz: 48_000,
  batch_frames: 1024,
  cutoff_modulation: { base_hz: 1000, depth_hz: 900 },
};

/**
 * A stand-in for the desktop host so the UI can be developed and tested in a
 * browser. It plays nothing. Its checks are a small subset of the engine's and
 * its probes are an approximation; the real rules live in Rust.
 */
export class MockEngine implements EngineClient {
  readonly label = "Mock engine";
  readonly live = false;

  private revision = 0;
  private session = 0;
  private playing: GraphSpec | null = null;
  private startedAt = 0;
  private timers: ReturnType<typeof setTimeout>[] = [];
  private revisionListeners = new Set<(event: RevisionEvent) => void>();

  async catalog() {
    return catalogJson as NodeDescriptor[];
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
    const missing = missingUpstream(graph, node);
    if (missing.length > 0) throw error("invalid", `the graph has ${missing.length} error(s)`, missing);
    const start = this.playing ? this.frame() : 0;
    return {
      node,
      start_frame: start,
      sample_rate_hz: INFO.sample_rate_hz,
      samples: render(graph, node, start, frames),
    };
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

function inputNames(node: NodeSpec): string[] {
  switch (node.kind) {
    case "gain":
    case "output":
      return ["in"];
    case "low_pass":
      return ["in"];
    case "mix":
      return (node.params.weights as number[]).map((_, index) => `in.${index}`);
    default:
      return [];
  }
}

/** Required inputs left open in `node` or anything feeding it, as the host reports them. */
function missingUpstream(graph: GraphSpec, node: string): GraphIssue[] {
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
    for (const port of inputNames(spec)) {
      const from = feeds.get(`${id}:${port}`);
      if (from) pending.push(from);
      else issues.push(issue("missing_input", id, `input "${port}" is not connected`, port));
    }
    const cutoff = feeds.get(`${id}:cutoff`);
    if (cutoff) pending.push(cutoff);
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
    const cutoff = feeds.get(`${id}:cutoff`);
    if (cutoff) pending.push(cutoff);
  }
  for (const node of graph.nodes) {
    if (!reachable.has(node.id)) {
      issues.push({ ...issue("unreachable", node.id, "not connected to the output; it won't be played"), severity: "warning" });
    }
    for (const name of ["frequency_hz", "cutoff_hz"]) {
      const value = node.params[name];
      if (typeof value === "number" && value >= INFO.sample_rate_hz / 2) {
        issues.push({ ...issue("invalid_parameter", node.id, `${name === "cutoff_hz" ? "cutoff" : "frequency"} must be below Nyquist (24000 Hz at 48000 Hz)`), param: name });
      }
    }
  }
  return issues;
}

/** A rough stateless model of the engine's nodes; good enough to draw. */
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
    const p = (name: string) => Number(node?.params[name] ?? 0);
    const noiseKey = mix32(seeds.get(id) ?? 0);
    for (let n = 0; n < frames && node; n++) {
      const t = (start + n) / INFO.sample_rate_hz;
      const phase = (p("frequency_hz") * t) % 1;
      switch (node.kind) {
        case "sine":
          out[n] = Math.sin(2 * Math.PI * phase);
          break;
        case "square":
          out[n] = phase < p("pulse_width") ? 1 : -1;
          break;
        case "sawtooth":
          out[n] = 2 * phase - 1;
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
          out[n] = (n > 0 ? out[n - 1] * (1 - a) : 0) + a * input("in")[n];
          break;
        }
      }
    }
    cache.set(id, out);
    return out;
  };
  return Array.from(signal(root));
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
