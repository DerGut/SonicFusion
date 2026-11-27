import { useCallback, useEffect, useRef, useState } from "react";

import type { EngineClient } from "../api/client";
import {
  type GraphIssue,
  type GraphSpec,
  type LevelReading,
  type RevisionStatus,
  isCommandError,
} from "../api/types";

export type Playback =
  | { state: "idle" }
  | { state: "starting" }
  | { state: "playing" }
  | { state: "failed"; message: string };

export interface RevisionInfo {
  revision: number;
  status: RevisionStatus;
}

const AUTO_APPLY_DELAY_MS = 350;
const LEVEL_INTERVAL_MS = 100;
const LEVEL_FRAMES = 1024;

/**
 * Session lifecycle and whole-graph submission. The draft may be invalid;
 * nothing is submitted until it has no errors.
 */
export function useEngineSession(client: EngineClient, draft: GraphSpec, hasErrors: boolean, autoApply: boolean) {
  const [playback, setPlayback] = useState<Playback>({ state: "idle" });
  const [latest, setLatest] = useState<RevisionInfo | null>(null);
  const [appliedJson, setAppliedJson] = useState<string | null>(null);
  const [commandIssues, setCommandIssues] = useState<GraphIssue[]>([]);
  const [level, setLevel] = useState<LevelReading | null>(null);
  const submitted = useRef(new Map<number, string>());
  const lastSubmittedJson = useRef<string | null>(null);
  const draftJson = JSON.stringify(draft);

  useEffect(
    () =>
      client.onRevision(({ revision, status }) => {
        setLatest((current) => (current && current.revision > revision ? current : { revision, status }));
        if (status.state === "applied") {
          const json = submitted.current.get(revision);
          if (json) setAppliedJson(json);
        }
      }),
    [client],
  );

  useEffect(
    () =>
      client.onSessionEnded((message) => {
        setPlayback(message ? { state: "failed", message } : { state: "idle" });
        setLevel(null);
      }),
    [client],
  );

  const start = useCallback(async () => {
    setPlayback({ state: "starting" });
    setCommandIssues([]);
    try {
      await client.startSession(draft);
      setPlayback({ state: "playing" });
      setAppliedJson(draftJson);
      lastSubmittedJson.current = draftJson;
      setLatest(null);
    } catch (error) {
      setPlayback({ state: "failed", message: describe(error) });
      if (isCommandError(error)) setCommandIssues(error.issues);
    }
  }, [client, draft, draftJson]);

  const stop = useCallback(async () => {
    await client.stopSession();
    setPlayback({ state: "idle" });
    setLevel(null);
  }, [client]);

  const apply = useCallback(async () => {
    lastSubmittedJson.current = draftJson;
    setCommandIssues([]);
    try {
      const revision = await client.applyGraph(draft);
      submitted.current.set(revision, draftJson);
    } catch (error) {
      setLatest((current) => ({
        revision: current?.revision ?? 0,
        status: { state: "rejected", message: describe(error), issues: isCommandError(error) ? error.issues : [] },
      }));
      if (isCommandError(error)) setCommandIssues(error.issues);
    }
  }, [client, draft, draftJson]);

  const playing = playback.state === "playing";

  // Auto-apply valid drafts after edits settle, so slider drags don't become
  // a revision per tick.
  useEffect(() => {
    if (!playing || !autoApply || hasErrors || draftJson === lastSubmittedJson.current) return;
    const timer = setTimeout(() => void apply(), AUTO_APPLY_DELAY_MS);
    return () => clearTimeout(timer);
  }, [playing, autoApply, hasErrors, draftJson, apply]);

  useEffect(() => {
    if (!playing) return;
    let cancelled = false;
    const timer = setInterval(() => {
      client
        .level(LEVEL_FRAMES)
        .then((reading) => !cancelled && setLevel(reading))
        .catch(() => {});
    }, LEVEL_INTERVAL_MS);
    return () => {
      cancelled = true;
      clearInterval(timer);
    };
  }, [client, playing]);

  return {
    playback,
    latest,
    level,
    commandIssues,
    dirty: playing && appliedJson !== draftJson,
    start,
    stop,
    apply,
  };
}

export function describe(error: unknown): string {
  if (isCommandError(error)) {
    const first = error.issues.find((issue) => issue.severity === "error");
    return first ? `${first.node ? `${first.node}: ` : ""}${first.message}` : error.message;
  }
  return error instanceof Error ? error.message : String(error);
}
