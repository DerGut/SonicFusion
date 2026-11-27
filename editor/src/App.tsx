import { ReactFlowProvider, useReactFlow } from "@xyflow/react";
import { useCallback, useEffect, useMemo, useReducer, useState } from "react";

import studioPatch from "../../crates/sonicfusion-graph-api/fixtures/studio-patch.json";
import { type EngineClient, connectEngine } from "./api/client";
import type { GraphIssue, GraphSpec, NodeKind, ParamValue, SessionInfo } from "./api/types";
import { Canvas } from "./components/Canvas";
import { ExportDialog } from "./components/ExportDialog";
import type { NodeActions, SfEdge, SfNode } from "./components/flow";
import { Palette } from "./components/Palette";
import { ProbePanel } from "./components/ProbePanel";
import { StatusBar } from "./components/StatusBar";
import { TopBar } from "./components/TopBar";
import {
  type Catalog,
  type EditorDocument,
  type Position,
  addMixInput,
  addNode,
  catalogMap,
  connect,
  inputPorts,
  moveNodes,
  parseDocument,
  removeMixInput,
  removeNodes,
  setParam,
  studioDocument,
  toGraph,
  wireId,
} from "./model/document";
import { historyReducer } from "./state/history";
import { describe, useEngineSession } from "./state/useEngineSession";

const DOCUMENT_KEY = "sonicfusion.document";

export function App() {
  const [ready, setReady] = useState<{ client: EngineClient; catalog: Catalog; session: SessionInfo } | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    connectEngine()
      .then(async (client) => {
        const [catalog, session] = await Promise.all([client.catalog(), client.sessionInfo()]);
        setReady({ client, catalog: catalogMap(catalog), session });
      })
      .catch((reason) => setError(describe(reason)));
  }, []);
  if (error) return <div className="loading error">Couldn't reach the engine: {error}</div>;
  if (!ready) return <div className="loading">Connecting to the engine…</div>;
  return (
    <ReactFlowProvider>
      <Editor {...ready} />
    </ReactFlowProvider>
  );
}

function Editor({ client, catalog, session }: { client: EngineClient; catalog: Catalog; session: SessionInfo }) {
  const flow = useReactFlow<SfNode, SfEdge>();
  const [history, dispatch] = useReducer(historyReducer, undefined, () => ({
    doc: loadDocument() ?? studioDocument(studioPatch as GraphSpec),
    past: [],
    future: [],
  }));
  const doc = history.doc;
  // Positions change constantly while dragging; the graph only when it really changes.
  const graphJson = JSON.stringify(toGraph(doc));
  const graph = useMemo(() => JSON.parse(graphJson) as GraphSpec, [graphJson]);

  const [issues, setIssues] = useState<GraphIssue[]>([]);
  useEffect(() => {
    let cancelled = false;
    const timer = setTimeout(() => {
      client.validate(graph).then((result) => !cancelled && setIssues(result)).catch(() => {});
    }, 120);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [client, graph]);
  const hasErrors = issues.some((issue) => issue.severity === "error");

  const [autoApply, setAutoApply] = useStored("sonicfusion.autoApply", true);
  const [ceilingDb, setCeilingDb] = useStored("sonicfusion.ceilingDb", -6);
  const engine = useEngineSession(client, graph, hasErrors, autoApply);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [exportSource, setExportSource] = useState<string | null>(null);

  useEffect(() => {
    const timer = setTimeout(() => {
      try {
        localStorage.setItem(DOCUMENT_KEY, JSON.stringify(doc));
      } catch {
        // Storage may be unavailable; autosave is a convenience.
      }
    }, 300);
    return () => clearTimeout(timer);
  }, [doc]);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.target instanceof Element && event.target.closest("input, textarea")) return;
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "z") {
        event.preventDefault();
        dispatch({ type: event.shiftKey ? "redo" : "undo" });
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const edit = useCallback(
    (change: (doc: EditorDocument) => EditorDocument, record = true) => dispatch({ type: "edit", change, record }),
    [],
  );

  const actions = useMemo<NodeActions>(
    () => ({
      setParam: (id: string, name: string, value: ParamValue, continuous: boolean) =>
        edit((current) => setParam(current, id, name, value), !continuous),
      beginContinuous: () => dispatch({ type: "checkpoint" }),
      addMixInput: (id: string) =>
        edit((current) => addMixInput(current, id, catalog.get("mix")?.variadic_input?.weight.default ?? 0.5)),
      removeMixInput: (id: string) =>
        edit((current) => removeMixInput(current, id, catalog.get("mix")?.variadic_input?.min ?? 1)),
      setCeiling: setCeilingDb,
    }),
    [edit, catalog, setCeilingDb],
  );

  const add = useCallback(
    (kind: NodeKind, position: Position, connectFrom?: string) => {
      const existingOutput = doc.nodes.find((node) => node.kind === "output");
      if (kind === "output" && existingOutput) {
        if (connectFrom) edit((current) => connect(current, connectFrom, { node: existingOutput.id, port: "in" }));
        return;
      }
      let added = "";
      edit((current) => {
        const [next, id] = addNode(current, catalog, kind, position);
        added = id;
        const node = next.nodes.find((candidate) => candidate.id === id)!;
        const first = inputPorts(node, catalog).find((port) => port.signal === "audio");
        return connectFrom && first ? connect(next, connectFrom, { node: id, port: first.name }) : next;
      });
      if (added) setSelected(new Set([added]));
    },
    [doc, catalog, edit],
  );

  const addAtCenter = (kind: NodeKind) => {
    const canvas = document.querySelector(".canvas")?.getBoundingClientRect();
    const center = canvas ? { x: canvas.left + canvas.width / 2, y: canvas.top + canvas.height / 3 } : { x: 400, y: 300 };
    const position = flow.screenToFlowPosition(center);
    add(kind, { x: position.x - 92, y: position.y });
  };

  const focus = (id: string) => {
    setSelected(new Set([id]));
    const node = doc.nodes.find((candidate) => candidate.id === id);
    if (node) void flow.setCenter(node.position.x + 92, node.position.y + 80, { duration: 300, zoom: flow.getZoom() });
  };

  const selectedNode = selected.size === 1 ? doc.nodes.find((node) => selected.has(node.id)) ?? null : null;
  const allIssues = useMemo(() => [...issues, ...engine.commandIssues.filter((issue) => issue.code === "engine_rejected")], [issues, engine.commandIssues]);
  const playing = engine.playback.state === "playing";

  return (
    <div className="app">
      <TopBar
        name={doc.name}
        engineLabel={client.label}
        live={client.live}
        sampleRateHz={session.sample_rate_hz}
        batchFrames={session.batch_frames}
        playback={engine.playback}
        latest={engine.latest}
        dirty={engine.dirty}
        errorCount={allIssues.filter((issue) => issue.severity === "error").length}
        autoApply={autoApply}
        canUndo={history.past.length > 0}
        canRedo={history.future.length > 0}
        onRename={(name) => edit((current) => ({ ...current, name }), false)}
        onUndo={() => dispatch({ type: "undo" })}
        onRedo={() => dispatch({ type: "redo" })}
        onPlay={() => void engine.start()}
        onStop={() => void engine.stop()}
        onApply={() => void engine.apply()}
        onAutoApply={setAutoApply}
        onExport={() => {
          client.exportDsl(graph).then(setExportSource).catch((reason) => setExportSource(`# ${describe(reason)}\n`));
        }}
      />
      {!client.live && (
        <p className="mock-banner" role="note">
          Browser preview with a mock engine: no sound, and checks and probes are approximate. Run the desktop app for the real engine.
        </p>
      )}
      <div className="workspace">
        <Palette catalog={catalog} hasOutput={doc.nodes.some((node) => node.kind === "output")} onAdd={addAtCenter} />
        <main className="main">
          <Canvas
            doc={doc}
            catalog={catalog}
            issues={allIssues}
            session={session}
            level={engine.level}
            ceilingDb={ceilingDb}
            selected={selected}
            actions={actions}
            onSelect={setSelected}
            onMove={(positions) => edit((current) => moveNodes(current, positions), false)}
            onRemove={(nodes, wires) => {
              const gone = new Set(wires);
              edit((current) => {
                const withoutWires = { ...current, wires: current.wires.filter((wire) => !gone.has(wireId(wire))) };
                return removeNodes(withoutWires, nodes);
              });
            }}
            onConnect={(from, to) => edit((current) => connect(current, from, to))}
            onAdd={add}
          />
          {selectedNode && (
            <ProbePanel
              key={selectedNode.id}
              client={client}
              graph={graph}
              node={selectedNode}
              playing={playing}
              session={session}
              onClose={() => setSelected(new Set())}
            />
          )}
        </main>
      </div>
      <StatusBar
        nodes={doc.nodes.length}
        wires={doc.wires.length}
        issues={allIssues}
        selected={selectedNode?.id ?? null}
        onFocus={focus}
      />
      {exportSource !== null && (
        <ExportDialog name={doc.name} source={exportSource} onClose={() => setExportSource(null)} />
      )}
    </div>
  );
}

function loadDocument(): EditorDocument | null {
  try {
    const saved = localStorage.getItem(DOCUMENT_KEY);
    return saved ? parseDocument(saved) : null;
  } catch {
    return null;
  }
}

function useStored<T>(key: string, initial: T): [T, (value: T) => void] {
  const [value, setValue] = useState<T>(() => {
    try {
      const saved = localStorage.getItem(key);
      return saved === null ? initial : (JSON.parse(saved) as T);
    } catch {
      return initial;
    }
  });
  const update = useCallback(
    (next: T) => {
      setValue(next);
      try {
        localStorage.setItem(key, JSON.stringify(next));
      } catch {
        // Not persisted; the setting still applies for this run.
      }
    },
    [key],
  );
  return [value, update];
}
