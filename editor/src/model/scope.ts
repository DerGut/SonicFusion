import type { GraphSpec, NodeSpec } from "../api/types";

const MAX_SECONDS = 4;

/** Gate waveform and envelope use a common window. Unknown gate rates get one second. */
export function slowScopeSeconds(graph: GraphSpec, node: NodeSpec): number | null {
  const twoCycles = (hz: number) => Math.min(MAX_SECONDS, Math.max(0.25, 2 / hz));
  if (node.kind === "envelope") {
    const gate = graph.nodes.find((candidate) => candidate.id === envelopeGate(graph, node.id));
    const hz = gate && oscillatorHz(gate);
    const constantSquare = gate?.kind === "square" && (gate.params.pulse_width === 0 || gate.params.pulse_width === 1);
    const dynamic = gate && graph.wires.some((wire) => wire.to.node === gate.id);
    return hz && !constantSquare && !dynamic ? twoCycles(hz) : 1;
  }
  const hz = oscillatorHz(node);
  return hz && hz < 20 ? twoCycles(hz) : null;
}

export function envelopeGate(graph: GraphSpec, node: string): string | undefined {
  return graph.wires.find((wire) => wire.to.node === node && wire.to.port === "gate")?.from;
}

function oscillatorHz(node: NodeSpec): number | undefined {
  if (node.kind !== "sine" && node.kind !== "square" && node.kind !== "sawtooth") return;
  const hz = node.params.frequency_hz;
  return typeof hz === "number" && Number.isFinite(hz) && hz > 0 ? hz : undefined;
}
