import { BaseEdge, type ConnectionLineComponentProps, type EdgeProps, getBezierPath } from "@xyflow/react";

import type { SfEdge } from "./flow";

/** Audio wires are solid amber, modulation wires dashed blue. */
export function WireEdge({ sourceX, sourceY, targetX, targetY, sourcePosition, targetPosition, data, selected }: EdgeProps<SfEdge>) {
  const [path] = getBezierPath({ sourceX, sourceY, targetX, targetY, sourcePosition, targetPosition });
  const control = data?.signal === "control";
  return (
    <BaseEdge path={path} interactionWidth={16}
      className={`wire ${control ? "control" : "audio"}${selected ? " selected" : ""}`} />
  );
}

export function ConnectionLine({ fromX, fromY, toX, toY, fromPosition, toPosition, connectionStatus }: ConnectionLineComponentProps) {
  const [path] = getBezierPath({ sourceX: fromX, sourceY: fromY, targetX: toX, targetY: toY, sourcePosition: fromPosition, targetPosition: toPosition });
  return (
    <g>
      <path d={path} className={`wire dragging${connectionStatus === "invalid" ? " invalid" : ""}`} fill="none" />
      {connectionStatus === "valid" && <circle cx={toX} cy={toY} r={14} className="snap-ring" />}
    </g>
  );
}
