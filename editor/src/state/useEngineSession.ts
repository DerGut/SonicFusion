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
 * nothing is submitted unless `canSubmit` says the current draft was checked
 * and has no errors.
 */
export function useEngineSession(client: EngineClient, draft: GraphSpec, canSubmit: boolean, autoApply: boolean) {
  const [playback, setPlayback] = useState<Playback>({ state: "idle" });
  const [latest, setLatest] = useState<RevisionInfo | null>(null);
  /** A submission the host refused before it got a revision. */
  const [refused, setRefused] = useState<{ json: string; message: string; issues: GraphIssue[] } | null>(null);
  const [appliedJson, setAppliedJson] = useState<string | null>(null);
  const [level, setLevel] = useState<LevelReading | null>(null);
  const session = useRef<number | null>(null);
  const submitted = useRef(new Map<number, string>());
  const appliedEarly = useRef(new Set<number>());
  const lastSubmittedJson = useRef<string | null>(null);
  const draftJson = JSON.stringify(draft);

  useEffect(
    () =>
      client.onRevision(({ session: id, revision, status }) => {
        if (id !== session.current) return; // A stopped session's late news.
        setLatest((current) => (current && current.revision > revision ? current : { revision, status }));
        if (status.state === "applied") {
          const json = submitted.current.get(revision);
          if (json) setAppliedJson(json);
          // The event can beat the invoke's reply; settle it there.
          else appliedEarly.current.add(revision);
        }
      }),
    [client],
  );

  useEffect(
    () =>
      client.onSessionEnded(({ session: id, message }) => {
        if (id !== session.current) return;
        session.current = null;
        setPlayback({ state: "failed", message });
        setLevel(null);
      }),
    [client],
  );

  const start = useCallback(async () => {
    setPlayback({ state: "starting" });
    setRefused(null);
    try {
      const id = await client.startSession(draft);
      session.current = id;
      submitted.current.clear();
      appliedEarly.current.clear();
      setPlayback({ state: "playing" });
      setAppliedJson(draftJson);
      lastSubmittedJson.current = draftJson;
      setLatest(null);
    } catch (error) {
      setPlayback({ state: "failed", message: describe(error) });
      if (isCommandError(error)) setRefused({ json: draftJson, message: describe(error), issues: error.issues });
    }
  }, [client, draft, draftJson]);

  const stop = useCallback(async () => {
    session.current = null;
    await client.stopSession();
    setPlayback({ state: "idle" });
    setLevel(null);
  }, [client]);

  const apply = useCallback(async () => {
    lastSubmittedJson.current = draftJson;
    setRefused(null);
    try {
      const revision = await client.applyGraph(draft);
      submitted.current.set(revision, draftJson);
      if (appliedEarly.current.delete(revision)) setAppliedJson(draftJson);
    } catch (error) {
      setRefused({
        json: draftJson,
        message: describe(error),
        issues: isCommandError(error) ? error.issues : [],
      });
    }
  }, [client, draft, draftJson]);

  const playing = playback.state === "playing";

  // Auto-apply valid drafts after edits settle, so slider drags don't become
  // a revision per tick.
  useEffect(() => {
    if (!playing || !autoApply || !canSubmit || draftJson === lastSubmittedJson.current) return;
    const timer = setTimeout(() => void apply(), AUTO_APPLY_DELAY_MS);
    return () => clearTimeout(timer);
  }, [playing, autoApply, canSubmit, draftJson, apply]);

  // Poll one reading at a time, so a slow render can't pile up requests.
  useEffect(() => {
    if (!playing) return;
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout>;
    const poll = () => {
      client
        .level(LEVEL_FRAMES)
        .then((reading) => !cancelled && setLevel(reading))
        .catch(() => {})
        .finally(() => {
          if (!cancelled) timer = setTimeout(poll, LEVEL_INTERVAL_MS);
        });
    };
    poll();
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [client, playing]);

  // A refusal describes the draft it was about; edits make it stale.
  const currentRefusal = refused && refused.json === draftJson ? refused : null;

  return {
    playback,
    latest,
    level,
    refusal: currentRefusal,
    /** Engine-side issues for the current draft, to show on nodes. */
    engineIssues: currentRefusal?.issues.filter((issue) => issue.code === "engine_rejected") ?? [],
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
