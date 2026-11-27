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

import type { GraphIssue, LevelReading, NodeKind, SessionInfo } from "../api/types";
import {
  type Catalog,
  type EditorDocument,
  type Position,
  inputPorts,
  wireId,
  wouldCycle,
} from "../model/document";
import type { NodeActions, SfEdge, SfNode } from "./flow";
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
  level: LevelReading | null;
  ceilingDb: number;
  selected: Set<string>;
  actions: NodeActions;
  onSelect(ids: Set<string>): void;
  onMove(positions: Map<string, Position>, done: boolean): void;
  onRemove(nodes: string[], wires: string[]): void;
  onConnect(from: string, to: { node: string; port: string }): void;
  onAdd(kind: NodeKind, position: Position, connectFrom?: string): void;
}

export function Canvas(props: Props) {
  const { doc, catalog, issues, session, level, ceilingDb, selected, actions } = props;
  const flow = useReactFlow<SfNode, SfEdge>();
  const [flowNodes, setFlowNodes] = useState<SfNode[]>([]);
  const [selectedWires, setSelectedWires] = useState<Set<string>>(new Set());
  const [menu, setMenu] = useState<{ x: number; y: number; from: string } | null>(null);
  const nodesRef = useRef(flowNodes);
  nodesRef.current = flowNodes;

  // Rebuild node data from the document, keeping React Flow's own fields
  // (measured size, dragging) from the previous objects.
  useEffect(() => {
    const sourcesByNode = new Map<string, Record<string, string>>();
    for (const wire of doc.wires) {
      const sources = sourcesByNode.get(wire.to.node) ?? {};
      sources[wire.to.port] = wire.from;
      sourcesByNode.set(wire.to.node, sources);
    }
    setFlowNodes((previous) => {
      const old = new Map(previous.map((node) => [node.id, node]));
      return doc.nodes.flatMap((node) => {
        const descriptor = catalog.get(node.kind);
        if (!descriptor) return [];
        const kept = old.get(node.id);
        return [{
          ...kept,
          id: node.id,
          type: "sf" as const,
          position: kept?.dragging ? kept.position : node.position,
          selected: selected.has(node.id),
          data: {
            node,
            descriptor,
            ports: inputPorts(node, catalog),
            sources: sourcesByNode.get(node.id) ?? {},
            issues: issues.filter((issue) => issue.node === node.id),
            session,
            level: node.kind === "output" ? level : null,
            ceilingDb,
            actions,
          },
        }];
      });
    });
  }, [doc, catalog, issues, session, level, ceilingDb, selected, actions]);

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

  return (
    <div className="canvas"
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
          const next = changes.applyTo(nodesRef.current);
          setFlowNodes(next);
          const moved = new Map<string, Position>();
          let dragging = false;
          for (const node of next) {
            const current = doc.nodes.find((candidate) => candidate.id === node.id);
            if (current && (current.position.x !== node.position.x || current.position.y !== node.position.y)) {
              moved.set(node.id, node.position);
            }
            dragging ||= Boolean(node.dragging);
          }
          if (moved.size > 0) props.onMove(moved, !dragging);
          const nextSelected = new Set(next.filter((node) => node.selected).map((node) => node.id));
          if (nextSelected.size !== selected.size || [...nextSelected].some((id) => !selected.has(id))) {
            props.onSelect(nextSelected);
          }
          const remaining = new Set(next.map((node) => node.id));
          const removed = doc.nodes.filter((node) => !remaining.has(node.id)).map((node) => node.id);
          if (removed.length > 0) props.onRemove(removed, []);
        }}
        onEdgesChange={(changes) => {
          const next = changes.applyTo(edges);
          const remaining = new Set(next.map((edge) => edge.id));
          const removed = edges.filter((edge) => !remaining.has(edge.id)).map((edge) => edge.id);
          if (removed.length > 0) props.onRemove([], removed);
          setSelectedWires(new Set(next.filter((edge) => edge.selected).map((edge) => edge.id)));
        }}
        onConnect={(connection) => {
          if (connection.targetHandle) props.onConnect(connection.source, { node: connection.target, port: connection.targetHandle });
        }}
        onConnectEnd={onConnectEnd}
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
          onClose={() => setMenu(null)}
        />
      )}
    </div>
  );
}
