// Mirrors the JSON of `crates/sonicfusion-graph-api`. The node vocabulary
// itself is not duplicated here: it comes from the engine's catalog.

export type NodeKind =
  | "sine"
  | "square"
  | "sawtooth"
  | "gain"
  | "low_pass"
  | "mix"
  | "output";

export type ParamValue = number | number[];

export interface NodeSpec {
  id: string;
  kind: NodeKind;
  params: Record<string, ParamValue>;
}

export interface PortRef {
  node: string;
  port: string;
}

export interface Wire {
  from: string;
  to: PortRef;
}

export interface GraphSpec {
  version: 1;
  nodes: NodeSpec[];
  wires: Wire[];
}

export type Signal = "audio" | "control";

export interface PortDescriptor {
  name: string;
  label: string;
  signal: Signal;
  required: boolean;
}

export interface ParamDescriptor {
  name: string;
  label: string;
  unit: string | null;
  min: number;
  max: number;
  default: number;
  scale: "linear" | "log";
}

export interface VariadicInput {
  prefix: string;
  weight: ParamDescriptor;
  min: number;
  default: number;
}

export interface NodeDescriptor {
  kind: NodeKind;
  label: string;
  category: "source" | "filter" | "utility" | "output";
  signature: string;
  inputs: PortDescriptor[];
  variadic_input: VariadicInput | null;
  has_output: boolean;
  params: ParamDescriptor[];
}

export interface GraphIssue {
  severity: "error" | "warning";
  code: string;
  node: string | null;
  port: string | null;
  param: string | null;
  message: string;
}

export type RevisionStatus =
  | { state: "submitted" }
  | { state: "accepted" }
  | { state: "superseded"; by_revision: number }
  | { state: "rejected"; message: string; issues: GraphIssue[] }
  | { state: "scheduled"; frame: number }
  | { state: "activation_rejected"; frame: number; message: string }
  | { state: "applied"; frame: number }
  | { state: "run_closed" };

export interface RevisionUpdate {
  revision: number;
  status: RevisionStatus;
}

/** Revision numbers restart per session, so events name their session. */
export interface RevisionEvent extends RevisionUpdate {
  session: number;
}

export interface SessionEnded {
  session: number;
  message: string;
}

export interface SessionInfo {
  sample_rate_hz: number;
  batch_frames: number;
  cutoff_modulation: { base_hz: number; depth_hz: number };
}

export interface ProbeWindow {
  node: string;
  start_frame: number;
  sample_rate_hz: number;
  samples: number[];
}

export interface LevelReading {
  frame: number;
  frames: number;
  peak_dbfs: number | null;
}

/** What a failed engine call rejects with. */
export interface CommandError {
  kind: "invalid" | "engine" | "no_session";
  message: string;
  issues: GraphIssue[];
}

export function isCommandError(value: unknown): value is CommandError {
  return (
    typeof value === "object" &&
    value !== null &&
    "kind" in value &&
    "message" in value &&
    "issues" in value
  );
}

export function isTerminal(status: RevisionStatus): boolean {
  return !["submitted", "accepted", "scheduled"].includes(status.state);
}
