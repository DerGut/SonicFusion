import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import type { EngineClient } from "./client";
import type {
  GraphIssue,
  GraphSpec,
  LevelReading,
  NodeDescriptor,
  ProbeWindow,
  RevisionEvent,
  SessionEnded,
  SessionInfo,
  Started,
} from "./types";

export class TauriEngine implements EngineClient {
  readonly label = "Engine";
  readonly live = true;

  catalog() {
    return invoke<NodeDescriptor[]>("catalog");
  }

  sessionInfo() {
    return invoke<SessionInfo>("session_info");
  }

  validate(graph: GraphSpec) {
    return invoke<GraphIssue[]>("validate", { graph });
  }

  exportDsl(graph: GraphSpec) {
    return invoke<string>("export_dsl", { graph });
  }

  startSession(graph: GraphSpec) {
    return invoke<Started>("start_session", { graph });
  }

  stopSession() {
    return invoke<void>("stop_session");
  }

  applyGraph(graph: GraphSpec) {
    return invoke<number>("apply_graph", { graph });
  }

  probe(graph: GraphSpec, node: string, frames: number) {
    return invoke<ProbeWindow>("probe", { graph, node, frames });
  }

  level(frames: number) {
    return invoke<LevelReading | null>("level", { frames });
  }

  onRevision(listener: (event: RevisionEvent) => void) {
    return subscribe<RevisionEvent>("revision-status", listener);
  }

  onSessionEnded(listener: (event: SessionEnded) => void) {
    return subscribe<SessionEnded>("session-ended", listener);
  }
}

function subscribe<T>(event: string, listener: (payload: T) => void): () => void {
  let unlisten: (() => void) | undefined;
  let cancelled = false;
  void listen<T>(event, (message) => listener(message.payload)).then((stop) => {
    if (cancelled) stop();
    else unlisten = stop;
  });
  return () => {
    cancelled = true;
    unlisten?.();
  };
}
