import type { GraphIssue } from "../api/types";
import { Icon } from "./icons";

export function StatusBar({ nodes, wires, issues, selected, pending, onFocus }: {
  nodes: number;
  wires: number;
  issues: GraphIssue[];
  selected: string | null;
  /** Playing, with edits not yet applied. */
  pending: boolean;
  onFocus: (node: string) => void;
}) {
  const errors = issues.filter((issue) => issue.severity === "error");
  const warnings = issues.length - errors.length;
  const first = errors[0] ?? issues[0];
  return (
    <footer className="statusbar">
      <span>{nodes} nodes · {wires} wires</span>
      {selected && <span>{selected} selected</span>}
      {issues.length === 0 ? (
        <span className="ok"><Icon name="check" size={12} /> {pending ? "Ready to apply" : "No problems"}</span>
      ) : (
        <button type="button" className={`issues ${errors.length ? "error" : "warning"}`}
          disabled={!first?.node} onClick={() => first?.node && onFocus(first.node)}>
          <Icon name="alert" size={12} />
          {errors.length > 0 && `${errors.length} error${errors.length > 1 ? "s" : ""}`}
          {errors.length > 0 && warnings > 0 && ", "}
          {warnings > 0 && `${warnings} warning${warnings > 1 ? "s" : ""}`}
          {first && <span className="issue-first">{first.node ? `${first.node}: ` : ""}{first.message}</span>}
        </button>
      )}
      <span className="spacer" />
      <span className="hint">Drag from a port to connect · Delete removes the selection · Applying restarts filter state</span>
    </footer>
  );
}
