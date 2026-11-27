import type { Edge, Node } from "@xyflow/react";
import { createContext } from "react";

import type {
  GraphIssue,
  LevelReading,
  NodeDescriptor,
  ParamValue,
  PortDescriptor,
  SessionInfo,
  Signal,
} from "../api/types";
import type { DocNode } from "../model/document";
import type { FrequencyMode } from "../model/pitch";

/** Fixed row metrics, so port handles line up with their rows. */
export const HEADER_HEIGHT = 34;
export const IO_HEIGHT = 28;
export const ROW_HEIGHT = 52;
export const IO_PORT_TOP = HEADER_HEIGHT + IO_HEIGHT / 2;

/** Top of the port that belongs to parameter row `index`. */
export function rowPortTop(index: number): number {
  return HEADER_HEIGHT + IO_HEIGHT + ROW_HEIGHT * index + 16;
}

/**
 * Start dragging an existing wire's end, as if its reconnect anchor had been
 * grabbed. Ports are drawn above those anchors, so a press on a port is
 * forwarded to the wire that should move.
 */
export function pickUpWire(event: React.PointerEvent | React.MouseEvent, wireId: string, end: "source" | "target"): boolean {
  const edge = [...document.querySelectorAll<SVGElement>(".react-flow__edge")].find(
    (element) => element.getAttribute("data-id") === wireId,
  );
  const anchor = edge?.querySelector<SVGElement>(`.react-flow__edgeupdater-${end}`);
  if (!anchor) return false;
  event.preventDefault();
  event.stopPropagation();
  anchor.dispatchEvent(
    new MouseEvent("mousedown", { bubbles: true, cancelable: true, clientX: event.clientX, clientY: event.clientY, button: 0, buttons: 1 }),
  );
  return true;
}

export interface NodeActions {
  setParam(id: string, name: string, value: ParamValue, continuous: boolean): void;
  setFrequencyMode(id: string, mode?: FrequencyMode): void;
  beginContinuous(): void;
  addMixInput(id: string): void;
  removeMixInput(id: string): void;
  setCeiling(db: number): void;
}

export interface SfNodeData extends Record<string, unknown> {
  node: DocNode;
  descriptor: NodeDescriptor;
  ports: PortDescriptor[];
  /** Connected input port name → the node feeding it. */
  sources: Record<string, string>;
  /** Ids of the wires leaving this node, oldest first. */
  outgoing: string[];
  feedsControl: boolean;
  issues: GraphIssue[];
  session: SessionInfo;
  ceilingDb: number;
  actions: NodeActions;
}

export type SfNode = Node<SfNodeData, "sf">;

/**
 * The output level, outside node data so a new reading every 100 ms
 * re-renders only the meter instead of every node.
 */
export const LevelContext = createContext<LevelReading | null>(null);

export interface WireData extends Record<string, unknown> {
  signal: Signal;
}

export type SfEdge = Edge<WireData, "wire">;
