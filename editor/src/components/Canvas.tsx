import {
  Background,
  BackgroundVariant,
  type Connection,
  Controls,
  type FinalConnectionState,
  MiniMap,
  ReactFlow,
  useReactFlow,
} from "@xyflow/react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import type { GraphIssue, NodeKind, SessionInfo } from "../api/types";
import {
  type Catalog,
  type EditorDocument,
  type Position,
  inputPorts,
  wireId,
  wouldCycle,
} from "../model/document";
import type { Wire } from "../api/types";
import type { NodeActions, SfEdge, SfNode, SfNodeData } from "./flow";
import { NodeView } from "./NodeView";
import { ConnectionLine, WireEdge } from "./WireEdge";
import { WireDropMenu } from "./WireDropMenu";

const nodeTypes = { sf: NodeView };
const edgeTypes = { wire: WireEdge };
export const DRAG_KIND = "application/x-sonicfusion-kind";

interface Props {
  doc: EditorDocument;
  catalog: Catalog;
  issues: GraphIssue[];
  session: SessionInfo;
  ceilingDb: number;
  selected: Set<string>;
  actions: NodeActions;
  onSelect(ids: Set<string>): void;
  /** Called with `start` on the first step of a drag, so it undoes as one step. */
  onMove(positions: Map<string, Position>, start: boolean): void;
  onRemove(nodes: string[], wires: string[]): void;
  onConnect(from: string, to: { node: string; port: string }): void;
  /** A picked-up wire end was dropped: on a port (`to` set) or on empty canvas. */
  onBeginWireMove(): void;
  onMoveWire(wire: Wire, from: string, to: { node: string; port: string } | null): void;
  onAdd(kind: NodeKind, position: Position, connectFrom?: string): void;
}

interface Menu {
  x: number;
  y: number;
  /** Set when opened by dropping a wire from this node. */
  from?: string;
}

export function Canvas(props: Props) {
  const { doc, catalog, issues, session, ceilingDb, selected, actions } = props;
  const flow = useReactFlow<SfNode, SfEdge>();
  const [flowNodes, setFlowNodes] = useState<SfNode[]>([]);
  const [selectedWires, setSelectedWires] = useState<Set<string>>(new Set());
  const [menu, setMenu] = useState<Menu | null>(null);
  const nodesRef = useRef<SfNode[]>([]);
  const pointer = useRef({ x: 0, y: 0 });
  const wireMoved = useRef(false);
  const dragging = useRef(false);
  const canvas = useRef<HTMLDivElement>(null);

  // Rebuild node objects only when their own inputs change, keeping React
  // Flow's fields (measured size, dragging) from the previous objects.
  useEffect(() => {
    const sourcesByNode = new Map<string, Record<string, string>>();
    const outgoingByNode = new Map<string, string[]>();
    for (const wire of doc.wires) {
      const sources = sourcesByNode.get(wire.to.node) ?? {};
      sources[wire.to.port] = wire.from;
      sourcesByNode.set(wire.to.node, sources);
      outgoingByNode.set(wire.from, [...(outgoingByNode.get(wire.from) ?? []), wireId(wire)]);
    }
    {
      const old = new Map(nodesRef.current.map((node) => [node.id, node]));
      nodesRef.current = doc.nodes.flatMap((node) => {
        const descriptor = catalog.get(node.kind);
        if (!descriptor) return [];
        const kept = old.get(node.id);
        const data: SfNodeData = {
          node,
          descriptor,
          ports: inputPorts(node, catalog),
          sources: sourcesByNode.get(node.id) ?? {},
          outgoing: outgoingByNode.get(node.id) ?? [],
          issues: issues.filter((issue) => issue.node === node.id),
          session,
          ceilingDb,
          actions,
        };
        const sameData = kept && sameNodeData(kept.data, data);
        const position = kept?.dragging ? kept.position : node.position;
        const isSelected = selected.has(node.id);
        if (kept && sameData && kept.position === position && Boolean(kept.selected) === isSelected) return [kept];
        return [{ ...kept, id: node.id, type: "sf" as const, position, selected: isSelected, data: sameData ? kept.data : data }];
      });
      setFlowNodes(nodesRef.current);
    }
  }, [doc, catalog, issues, session, ceilingDb, selected, actions]);

  const edges = useMemo<SfEdge[]>(
    () =>
      doc.wires.map((wire) => {
        const target = doc.nodes.find((node) => node.id === wire.to.node);
        const port = target && inputPorts(target, catalog).find((candidate) => candidate.name === wire.to.port);
        const id = wireId(wire);
        return {
          id,
          type: "wire",
          source: wire.from,
          sourceHandle: "out",
          target: wire.to.node,
          targetHandle: wire.to.port,
          selected: selectedWires.has(id),
          data: { signal: port?.signal ?? "audio" },
        };
      }),
    [doc, catalog, selectedWires],
  );

  const isValidConnection = useCallback(
    (connection: Connection | SfEdge) => {
      const { source, target, targetHandle } = connection;
      if (!source || !target || !targetHandle) return false;
      const node = doc.nodes.find((candidate) => candidate.id === target);
      if (!node || !inputPorts(node, catalog).some((port) => port.name === targetHandle)) return false;
      return !wouldCycle(doc, source, target);
    },
    [doc, catalog],
  );

  const onConnectEnd = useCallback((event: MouseEvent | TouchEvent, state: FinalConnectionState) => {
    // A wire dropped on empty canvas offers nodes that can take it.
    if (state.isValid || state.toHandle || state.fromHandle?.type !== "source" || !state.fromNode) return;
    const point = "changedTouches" in event ? event.changedTouches[0] : event;
    setMenu({ x: point.clientX, y: point.clientY, from: state.fromNode.id });
  }, []);

  // Tab opens the same menu, unfiltered, where the pointer is.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Tab" || event.metaKey || event.ctrlKey || event.altKey) return;
      const target = event.target;
      if (target instanceof Element && target !== document.body && !canvas.current?.contains(target)) return;
      if (target instanceof Element && target.closest("input, textarea, button, [role='menu']")) return;
      event.preventDefault();
      const box = canvas.current?.getBoundingClientRect();
      const inside = box && pointer.current.x >= box.left && pointer.current.x <= box.right
        && pointer.current.y >= box.top && pointer.current.y <= box.bottom;
      setMenu(inside ? { ...pointer.current } : { x: (box?.left ?? 0) + 80, y: (box?.top ?? 0) + 80 });
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  return (
    <div className="canvas" ref={canvas}
      onPointerMove={(event) => {
        pointer.current = { x: event.clientX, y: event.clientY };
      }}
      onDragOver={(event) => {
        if (event.dataTransfer.types.includes(DRAG_KIND)) {
          event.preventDefault();
          event.dataTransfer.dropEffect = "copy";
        }
      }}
      onDrop={(event) => {
        const kind = event.dataTransfer.getData(DRAG_KIND) as NodeKind;
        if (!kind) return;
        event.preventDefault();
        props.onAdd(kind, flow.screenToFlowPosition({ x: event.clientX, y: event.clientY }));
      }}>
      <ReactFlow<SfNode, SfEdge>
        nodes={flowNodes}
        edges={edges}
        nodeTypes={nodeTypes}
        edgeTypes={edgeTypes}
        forceColorMode="dark"
        fitView
        fitViewOptions={{ padding: 0.15 }}
        minZoom={0.3}
        maxZoom={2}
        connectionRadius={24}
        connectionLineComponent={ConnectionLine}
        isValidConnection={isValidConnection}
        deleteKeyCode={["Backspace", "Delete"]}
        onNodesChange={(changes) => {
          // Removals go through onDelete as one edit; here only layout and selection.
          nodesRef.current = changes.applyTo(nodesRef.current);
          setFlowNodes(nodesRef.current);
          reconcile(nodesRef.current);
        }}
        onEdgesChange={(changes) => {
          const next = changes.applyTo(edges);
          setSelectedWires(new Set(next.filter((edge) => edge.selected).map((edge) => edge.id)));
        }}
        onDelete={({ nodes, edges: wires }) => {
          props.onRemove(nodes.map((node) => node.id), wires.map((edge) => edge.id));
        }}
        onConnect={(connection) => {
          if (connection.targetHandle) props.onConnect(connection.source, { node: connection.target, port: connection.targetHandle });
        }}
        onConnectEnd={onConnectEnd}
        edgesReconnectable
        reconnectRadius={14}
        onReconnectStart={() => {
          wireMoved.current = false;
          props.onBeginWireMove();
        }}
        onReconnect={(edge, connection) => {
          const wire = wireOf(edge);
          if (!wire || !connection.targetHandle) return;
          wireMoved.current = true;
          props.onMoveWire(wire, connection.source, { node: connection.target, port: connection.targetHandle });
        }}
        onReconnectEnd={(_, edge) => {
          // Dropped on empty canvas: the wire is detached.
          const wire = wireOf(edge);
          if (!wireMoved.current && wire) props.onMoveWire(wire, wire.from, null);
        }}
        onPaneClick={() => setMenu(null)}
      >
        <Background variant={BackgroundVariant.Dots} gap={24} size={1.4} color="#262C35" />
        <Controls showInteractive={false} position="bottom-left" />
        <MiniMap position="bottom-right" pannable zoomable nodeColor={(node) => (node.selected ? "#C4CAD3" : "#3A424E")}
          maskColor="rgba(14,16,20,0.6)" />
      </ReactFlow>
      {menu && (
        <WireDropMenu
          x={menu.x}
          y={menu.y}
          from={menu.from}
          doc={doc}
          catalog={catalog}
          onPick={(kind) => {
            props.onAdd(kind, flow.screenToFlowPosition({ x: menu.x, y: menu.y }), menu.from);
            setMenu(null);
          }}
          onClose={() => {
            setMenu(null);
            canvas.current?.querySelector<HTMLElement>(".react-flow__pane")?.focus();
          }}
        />
      )}
    </div>
  );

  function reconcile(next: SfNode[]) {
    const moved = new Map<string, Position>();
    let anyDragging = false;
    for (const node of next) {
      const current = doc.nodes.find((candidate) => candidate.id === node.id);
      if (current && (current.position.x !== node.position.x || current.position.y !== node.position.y)) {
        moved.set(node.id, node.position);
      }
      anyDragging ||= Boolean(node.dragging);
    }
    if (moved.size > 0) {
      props.onMove(moved, !dragging.current);
      dragging.current = anyDragging;
    } else if (!anyDragging) {
      dragging.current = false;
    }
    const nextSelected = new Set(next.filter((node) => node.selected).map((node) => node.id));
    if (nextSelected.size !== selected.size || [...nextSelected].some((id) => !selected.has(id))) {
      props.onSelect(nextSelected);
    }
  }
}

function wireOf(edge: SfEdge): Wire | null {
  if (!edge.targetHandle) return null;
  return { from: edge.source, to: { node: edge.target, port: edge.targetHandle } };
}

function sameNodeData(a: SfNodeData, b: SfNodeData): boolean {
  return (
    a.node === b.node &&
    a.descriptor === b.descriptor &&
    a.session === b.session &&
    a.ceilingDb === b.ceilingDb &&
    a.actions === b.actions &&
    JSON.stringify(a.ports) === JSON.stringify(b.ports) &&
    JSON.stringify(a.sources) === JSON.stringify(b.sources) &&
    JSON.stringify(a.outgoing) === JSON.stringify(b.outgoing) &&
    JSON.stringify(a.issues) === JSON.stringify(b.issues)
  );
}
