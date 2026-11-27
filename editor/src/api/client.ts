import type {
  GraphIssue,
  GraphSpec,
  LevelReading,
  NodeDescriptor,
  ProbeWindow,
  RevisionUpdate,
  SessionInfo,
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
  startSession(graph: GraphSpec): Promise<void>;
  stopSession(): Promise<void>;
  /** Resolves with the revision once the graph passed validation. */
  applyGraph(graph: GraphSpec): Promise<number>;
  /** Recomputed output of `node` at the session's current frame. */
  probe(graph: GraphSpec, node: string, frames: number): Promise<ProbeWindow>;
  /** Peak of what is playing, or null without a session. */
  level(frames: number): Promise<LevelReading | null>;
  onRevision(listener: (update: RevisionUpdate) => void): () => void;
  /** Playback ended on its own; `message` is set when it failed. */
  onSessionEnded(listener: (message: string | null) => void): () => void;
}

export async function connectEngine(): Promise<EngineClient> {
  if ("__TAURI_INTERNALS__" in window) {
    const { TauriEngine } = await import("./tauri");
    return new TauriEngine();
  }
  const { MockEngine } = await import("./mock");
  return new MockEngine();
}
