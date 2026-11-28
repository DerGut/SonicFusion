import type { SessionInfo } from "../api/types";
import { nodeLabel, type EditorDocument } from "./document";

export interface ConstantDestination {
  node: string;
  label: string;
  port: string;
  hz: number;
  minHz: number;
  maxHz: number;
  capped: boolean;
}

/** Each direct destination interprets the same normalized DC value independently. */
export function constantDestinations(doc: EditorDocument, id: string, session: SessionInfo): ConstantDestination[] {
  const source = doc.nodes.find((node) => node.id === id);
  const value = source?.params.value;
  if (source?.kind !== "constant" || typeof value !== "number" || !Number.isFinite(value) || value < -1 || value > 1) return [];
  // Frame samples are f32, even though saved parameters are JSON numbers.
  const control = Math.fround(value);
  const nyquist = new Float64Array([session.sample_rate_hz / 2]);
  new BigUint64Array(nyquist.buffer)[0] -= 1n;
  const ceiling = nyquist[0];
  return doc.wires.flatMap((wire) => {
    if (wire.from !== id) return [];
    const target = doc.nodes.find((node) => node.id === wire.to.node);
    if (!target) return [];
    const destination = { node: target.id, label: nodeLabel(target), port: wire.to.port };
    if (wire.to.port === "frequency" && (target.kind === "sine" || target.kind === "sawtooth")) {
      const base = target.params.frequency_hz;
      if (typeof base !== "number" || !Number.isFinite(base) || base < 0 || base >= session.sample_rate_hz / 2) return [];
      const requested = base * 2 ** control;
      return [{ ...destination, hz: Math.min(requested, ceiling), minHz: base / 2, maxHz: Math.min(base * 2, ceiling), capped: requested > ceiling }];
    }
    if (wire.to.port === "cutoff" && target.kind === "low_pass") {
      const { base_hz: base, depth_hz: depth } = session.cutoff_modulation;
      return [{ ...destination, hz: base + depth * control, minHz: base - depth, maxHz: base + depth, capped: false }];
    }
    return [];
  });
}
