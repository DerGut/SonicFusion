import { ReactFlowProvider, useReactFlow } from "@xyflow/react";
import { Component, type ReactNode, useCallback, useEffect, useMemo, useReducer, useState } from "react";

import studioPatch from "../../crates/sonicfusion-graph-api/fixtures/studio-patch.json";
import { type EngineClient, connectEngine } from "./api/client";
import type { GraphIssue, GraphSpec, NodeKind, ParamValue, SessionInfo } from "./api/types";
import { Canvas } from "./components/Canvas";
import { ExportDialog } from "./components/ExportDialog";
import { LevelContext, type NodeActions, type SfEdge, type SfNode } from "./components/flow";
import { Palette } from "./components/Palette";
import { ProbePanel } from "./components/ProbePanel";
import { StatusBar } from "./components/StatusBar";
import { TopBar } from "./components/TopBar";
import { OscSwitcher, oscillatorScenario, prototypeActive } from "./prototype/oscillator-pitch.prototype";
import { DocNodesProvider } from "./prototype/doc-nodes.prototype";
import {
  type Catalog,
  type EditorDocument,
  type Position,
  addConnectedNode,
  addMixInput,
  catalogMap,
  connect,
  moveNodes,
  moveWire,
  nextNodeId,
  nodeLabel,
  parseDocument,
  removeMixInput,
  removeNodes,
  renameNode,
  setParam,
  setFrequencyMode,
  studioDocument,
  toGraph,
  wireId,
} from "./model/document";
import { historyReducer } from "./state/history";
import { describe, useEngineSession } from "./state/useEngineSession";

const DOCUMENT_KEY = "sonicfusion.document";
const VALIDATE_DELAY_MS = 120;

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
    <CrashBoundary>
      <ReactFlowProvider>
        <Editor {...ready} />
      </ReactFlowProvider>
    </CrashBoundary>
  );
}

/** Keeps a broken project from leaving a blank window on every launch. */
class CrashBoundary extends Component<{ children: ReactNode }, { error: Error | null }> {
  state = { error: null as Error | null };

  static getDerivedStateFromError(error: Error) {
    return { error };
  }

  render() {
    if (!this.state.error) return this.props.children;
    return (
      <div className="crash" role="alert">
        <p>The editor hit an error: {this.state.error.message}</p>
        <button type="button" className="button primary" onClick={() => {
          try {
            localStorage.removeItem(DOCUMENT_KEY);
          } catch {
            // Nothing saved to clear.
          }
          window.location.reload();
        }}>
          Reset the project and reload
        </button>
      </div>
    );
  }
}

function Editor({ client, catalog, session: initialSession }: { client: EngineClient; catalog: Catalog; session: SessionInfo }) {
  const flow = useReactFlow<SfNode, SfEdge>();
  // Sample rate and limits follow the output device each time playback starts.
  const [session, setSession] = useState(initialSession);
  const [history, dispatch] = useReducer(historyReducer, undefined, () => ({
    // PROTOTYPE oscillator pitch
    doc: prototypeActive() ? oscillatorScenario(studioDocument(studioPatch as GraphSpec)) : loadDocument() ?? studioDocument(studioPatch as GraphSpec),
    past: [],
    future: [],
  }));
  const doc = history.doc;
  // Positions change constantly while dragging; the graph only when it really changes.
  const graphJson = JSON.stringify(toGraph(doc));
  const graph = useMemo(() => JSON.parse(graphJson) as GraphSpec, [graphJson]);

  // Issues belong to the graph they were computed for, so a stale "no errors"
  // can never let an unchecked draft through.
  const [checked, setChecked] = useState<{ json: string; session: SessionInfo; issues: GraphIssue[] } | null>(null);
  useEffect(() => {
    let cancelled = false;
    const timer = setTimeout(() => {
      client
        .validate(graph)
        .then((issues) => !cancelled && setChecked({ json: graphJson, session, issues }))
        .catch((reason) => {
          if (cancelled) return;
          const issue: GraphIssue = { severity: "error", code: "engine_rejected", node: null, port: null, param: null, message: `couldn't check the graph: ${describe(reason)}` };
          setChecked({ json: graphJson, session, issues: [issue] });
        });
    }, VALIDATE_DELAY_MS);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [client, graph, graphJson, session]);
  const current = checked?.json === graphJson && checked.session === session;
  const issues = checked?.issues ?? [];

  const [autoApply, setAutoApply] = useStored("sonicfusion.autoApply", true);
  const [ceilingDb, setCeilingDb] = useStored("sonicfusion.ceilingDb", -6);
  const allIssues = issues;
  const hasErrors = issues.some((issue) => issue.severity === "error");
  const updateSession = useCallback((info: SessionInfo) => {
    setSession((previous) => (JSON.stringify(previous) === JSON.stringify(info) ? previous : info));
  }, []);
  const engine = useEngineSession(client, graph, current && !hasErrors, autoApply, updateSession);
  const shownIssues = useMemo(() => [...allIssues, ...engine.engineIssues], [allIssues, engine.engineIssues]);
  const errorCount = shownIssues.filter((issue) => issue.severity === "error").length;
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
      if (isTextEntry(event.target)) return;
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
      renameNode: (id, name) => edit((current) => renameNode(current, id, name)),
      setParam: (id: string, name: string, value: ParamValue, continuous: boolean) =>
        edit((current) => setParam(current, id, name, value), !continuous),
      setFrequencyMode: (id, mode) => edit((current) => setFrequencyMode(current, id, mode)),
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
      // Decided from the current document, outside the reducer, so the new
      // node can be selected right away.
      const [, target] = addConnectedNode(doc, catalog, kind, position, connectFrom);
      edit((current) => addConnectedNode(current, catalog, kind, position, connectFrom)[0]);
      const id = target ?? (kind === "output" ? null : nextNodeId(doc, kind));
      if (id) setSelected(new Set([id]));
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
  const nodesById = useMemo(() => new Map(doc.nodes.map((node) => [node.id, node])), [doc.nodes]);
  const playing = engine.playback.state === "playing";

  return (
    <DocNodesProvider value={nodesById}>
    <div className="app">
      <TopBar
        name={doc.name}
        engineLabel={client.label}
        live={client.live}
        sampleRateHz={session.sample_rate_hz}
        batchFrames={session.batch_frames}
        playback={engine.playback}
        latest={engine.latest}
        refusal={engine.refusal}
        dirty={engine.dirty}
        errorCount={errorCount}
        checking={!current}
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
          <LevelContext.Provider value={engine.level}>
            <Canvas
              doc={doc}
              catalog={catalog}
              issues={shownIssues}
              session={session}
              ceilingDb={ceilingDb}
              selected={selected}
              actions={actions}
              onSelect={setSelected}
              onMove={(positions, start) => {
                if (start) dispatch({ type: "checkpoint" });
                edit((current) => moveNodes(current, positions), false);
              }}
              onRemove={(nodes, wires) => {
                const gone = new Set(wires);
                edit((current) => {
                  const withoutWires = { ...current, wires: current.wires.filter((wire) => !gone.has(wireId(wire))) };
                  return removeNodes(withoutWires, nodes);
                });
                if (nodes.length > 0) setSelected(new Set());
              }}
              onConnect={(from, to) => edit((current) => connect(current, from, to))}
              onBeginWireMove={() => dispatch({ type: "checkpoint" })}
              onMoveWire={(wire, from, to) =>
                edit(
                  (current) =>
                    to
                      ? moveWire(current, wire, from, to)
                      : { ...current, wires: current.wires.filter((candidate) => wireId(candidate) !== wireId(wire)) },
                  false,
                )
              }
              onAdd={add}
            />
          </LevelContext.Provider>
          {selectedNode && (
            <ProbePanel
              key={selectedNode.id}
              client={client}
              graph={graph}
              node={selectedNode}
              playing={playing}
              draft={engine.dirty}
              session={session}
              onRename={actions.renameNode}
              onClose={() => setSelected(new Set())}
            />
          )}
        </main>
      </div>
      <StatusBar
        nodes={doc.nodes.length}
        wires={doc.wires.length}
        issues={shownIssues}
        selected={selectedNode ? nodeLabel(selectedNode) : null}
        pending={engine.dirty}
        onFocus={focus}
      />
      {exportSource !== null && (
        <ExportDialog name={doc.name} source={exportSource} onClose={() => setExportSource(null)} />
      )}
      <OscSwitcher />
    </div>
    </DocNodesProvider>
  );
}

/** Fields where Cmd+Z should edit text rather than undo the patch. */
function isTextEntry(target: EventTarget | null): boolean {
  if (target instanceof HTMLTextAreaElement) return true;
  if (!(target instanceof HTMLInputElement)) return false;
  return !["range", "checkbox", "radio", "button"].includes(target.type);
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
