import type {
  GraphIssue,
  GraphSpec,
  LevelReading,
  NodeDescriptor,
  ProbeWindow,
  RevisionEvent,
  ScopeWindow,
  SessionEnded,
  SessionInfo,
  Started,
} from "./types";

/**
 * Everything the UI may ask of the engine. Graphs always travel whole; there
 * is no call that edits a running node. Implementations: the Tauri host, and
 * a browser mock for UI development and tests.
 */
export interface EngineClient {
  readonly label: string;
  /** False when no real audio engine is behind this client. */
  readonly live: boolean;
  catalog(): Promise<NodeDescriptor[]>;
  sessionInfo(): Promise<SessionInfo>;
  validate(graph: GraphSpec): Promise<GraphIssue[]>;
  exportDsl(graph: GraphSpec): Promise<string>;
  /** Play `graph` on the default output at its native rate. */
  startSession(graph: GraphSpec): Promise<Started>;
  stopSession(): Promise<void>;
  /** Resolves with the revision once the graph passed validation. */
  applyGraph(graph: GraphSpec): Promise<number>;
  /** Recomputed output of `node` at the session's current frame. */
  probe(graph: GraphSpec, node: string, frames: number): Promise<ProbeWindow>;
  /** Decimated long-window Scope display with an optional envelope gate overlay. */
  scope(graph: GraphSpec, node: string, seconds: number): Promise<ScopeWindow>;
  /** Peak of what is playing, or null without a session. */
  level(frames: number): Promise<LevelReading | null>;
  onRevision(listener: (event: RevisionEvent) => void): () => void;
  /** Playback failed on its own (stops you requested aren't reported). */
  onSessionEnded(listener: (event: SessionEnded) => void): () => void;
}

export async function connectEngine(): Promise<EngineClient> {
  if ("__TAURI_INTERNALS__" in window) {
    const { TauriEngine } = await import("./tauri");
    return new TauriEngine();
  }
  const { MockEngine } = await import("./mock");
  return new MockEngine();
}
