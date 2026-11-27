import type { RevisionStatus } from "../api/types";
import type { Playback, RevisionInfo } from "../state/useEngineSession";
import { Icon } from "./icons";

export function TopBar(props: {
  name: string;
  engineLabel: string;
  live: boolean;
  sampleRateHz: number;
  batchFrames: number;
  playback: Playback;
  latest: RevisionInfo | null;
  dirty: boolean;
  errorCount: number;
  autoApply: boolean;
  canUndo: boolean;
  canRedo: boolean;
  onRename(name: string): void;
  onUndo(): void;
  onRedo(): void;
  onPlay(): void;
  onStop(): void;
  onApply(): void;
  onAutoApply(enabled: boolean): void;
  onExport(): void;
}) {
  const { playback, latest } = props;
  const playing = playback.state === "playing";
  const status = statusLine(playback, latest);
  return (
    <header className="topbar">
      <div className="brand">
        <svg width="22" height="22" viewBox="0 0 22 22" fill="none" aria-hidden="true">
          <rect x="0.5" y="0.5" width="21" height="21" rx="6" fill="#1C2129" stroke="#2B323D" />
          <path d="M4 11C6 5 8 5 10 11S14 17 18 11" stroke="#F2A33A" strokeWidth="2" strokeLinecap="round" />
        </svg>
        <span>SonicFusion</span>
      </div>
      <div className="divider" />
      <input className="patch-name" aria-label="Patch name" value={props.name}
        onChange={(event) => props.onRename(event.target.value)} />
      <div className="tool-group">
        <button type="button" className="icon-button" aria-label="Undo" disabled={!props.canUndo} onClick={props.onUndo}>
          <Icon name="undo" size={16} />
        </button>
        <button type="button" className="icon-button" aria-label="Redo" disabled={!props.canRedo} onClick={props.onRedo}>
          <Icon name="redo" size={16} />
        </button>
      </div>
      <div className="spacer" />
      <div className={`status-pill ${status.tone}`} role="status" aria-live="polite" title={status.detail}>
        <span className="dot" />
        <span className="status-main">{status.text}</span>
        <span className="status-meta">{props.engineLabel} · {props.sampleRateHz / 1000} kHz · {props.batchFrames}-frame blocks</span>
      </div>
      <label className="toggle" title="Apply valid edits automatically while playing">
        <input type="checkbox" checked={props.autoApply} onChange={(event) => props.onAutoApply(event.target.checked)} />
        Auto-apply
      </label>
      <button type="button" className="button secondary" disabled={!playing || !props.dirty || props.errorCount > 0}
        onClick={props.onApply}
        title="Replace the playing circuit at the next block. This is a hard cut: filter state restarts.">
        <Icon name="check" /> Apply
      </button>
      {playing ? (
        <button type="button" className="button secondary" onClick={props.onStop}><Icon name="stop" size={12} /> Stop</button>
      ) : (
        <button type="button" className="button primary" disabled={props.errorCount > 0 || playback.state === "starting"}
          onClick={props.onPlay} title={props.live ? "Play on the default output device" : "The mock engine doesn't make sound"}>
          <Icon name="play" size={12} /> Play
        </button>
      )}
      <button type="button" className="button secondary" disabled={props.errorCount > 0} onClick={props.onExport}>
        <Icon name="export" /> Export .sf
      </button>
    </header>
  );
}

type Tone = "idle" | "pending" | "ok" | "error" | "neutral";

function statusLine(playback: Playback, latest: RevisionInfo | null): { text: string; tone: Tone; detail?: string } {
  if (playback.state === "failed") return { text: `Playback failed: ${playback.message}`, tone: "error" };
  if (playback.state === "starting") return { text: "Starting…", tone: "pending" };
  if (playback.state === "idle") return { text: "Not playing", tone: "idle" };
  if (!latest) return { text: "Live", tone: "ok" };
  return revisionLine(latest.revision, latest.status);
}

export function revisionLine(revision: number, status: RevisionStatus): { text: string; tone: Tone; detail?: string } {
  const rev = revision > 0 ? `rev ${revision}` : "edit";
  switch (status.state) {
    case "submitted":
      return { text: `Live · ${rev} preparing`, tone: "pending" };
    case "accepted":
      return { text: `Live · ${rev} waiting for the next block`, tone: "pending" };
    case "scheduled":
      return { text: `Live · ${rev} scheduled at frame ${status.frame}`, tone: "pending" };
    case "applied":
      return {
        text: `Live · ${rev} applied at frame ${status.frame}`,
        tone: "ok",
        detail: "Hard cut: the new circuit started from fresh state at this frame.",
      };
    case "superseded":
      return { text: `Live · ${rev} replaced by rev ${status.by_revision}`, tone: "neutral" };
    case "rejected":
      return { text: `${rev} rejected · previous circuit still playing`, tone: "error", detail: status.message };
    case "activation_rejected":
      return { text: `${rev} failed at frame ${status.frame} · previous circuit still playing`, tone: "error", detail: status.message };
    case "run_closed":
      return { text: `${rev} not applied: playback stopped`, tone: "neutral" };
  }
}
