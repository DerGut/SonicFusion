import type {
  GraphSpec,
  NodeDescriptor,
  NodeKind,
  ParamValue,
  PortDescriptor,
  PortRef,
  Wire,
} from "../api/types";

export interface Position {
  x: number;
  y: number;
}

export interface DocNode {
  id: string;
  kind: NodeKind;
  params: Record<string, ParamValue>;
  position: Position;
}

/**
 * The editor's project file: the graph plus layout. Layout never reaches the
 * engine; `toGraph` strips it.
 */
export interface EditorDocument {
  format: "sonicfusion-editor";
  version: 1;
  name: string;
  nodes: DocNode[];
  wires: Wire[];
}

export type Catalog = Map<NodeKind, NodeDescriptor>;

export function catalogMap(descriptors: NodeDescriptor[]): Catalog {
  return new Map(descriptors.map((descriptor) => [descriptor.kind, descriptor]));
}

export function toGraph(doc: EditorDocument): GraphSpec {
  return {
    version: 1,
    nodes: doc.nodes.map(({ id, kind, params }) => ({ id, kind, params })),
    wires: doc.wires,
  };
}

export function fromGraph(graph: GraphSpec, name: string, layout: Record<string, Position>): EditorDocument {
  return {
    format: "sonicfusion-editor",
    version: 1,
    name,
    nodes: graph.nodes.map((node, index) => ({
      ...node,
      position: layout[node.id] ?? { x: 40 + 240 * index, y: 80 },
    })),
    wires: graph.wires,
  };
}

/** Input ports in display order, including user-added mix inputs. */
export function inputPorts(node: DocNode, catalog: Catalog): PortDescriptor[] {
  const descriptor = catalog.get(node.kind);
  if (!descriptor) return [];
  const variadic = descriptor.variadic_input;
  if (!variadic) return descriptor.inputs;
  const weights = node.params[variadic.weight.name];
  const count = Array.isArray(weights) ? weights.length : 0;
  return Array.from({ length: count }, (_, index) => ({
    name: `${variadic.prefix}.${index}`,
    label: `Input ${index + 1}`,
    signal: "audio" as const,
    required: true,
  }));
}

export function addNode(
  doc: EditorDocument,
  catalog: Catalog,
  kind: NodeKind,
  position: Position,
): [EditorDocument, string] {
  const descriptor = catalog.get(kind);
  if (!descriptor) throw new Error(`unknown node kind ${kind}`);
  const params: Record<string, ParamValue> = {};
  for (const param of descriptor.params) params[param.name] = param.default;
  if (descriptor.variadic_input) {
    const { weight, default: count } = descriptor.variadic_input;
    params[weight.name] = Array.from({ length: count }, () => weight.default);
  }
  const id = uniqueId(doc, kind);
  return [{ ...doc, nodes: [...doc.nodes, { id, kind, params, position }] }, id];
}

function uniqueId(doc: EditorDocument, kind: NodeKind): string {
  const taken = new Set(doc.nodes.map((node) => node.id));
  for (let index = 1; ; index++) {
    const id = `${kind}_${index}`;
    if (!taken.has(id)) return id;
  }
}

/** True if a wire from `from` into `toNode` would close a loop. */
export function wouldCycle(doc: EditorDocument, from: string, toNode: string): boolean {
  if (from === toNode) return true;
  // A loop exists if `toNode` already feeds `from`.
  const upstream = new Map<string, string[]>();
  for (const wire of doc.wires) {
    upstream.set(wire.to.node, [...(upstream.get(wire.to.node) ?? []), wire.from]);
  }
  const pending = [from];
  const seen = new Set<string>();
  while (pending.length > 0) {
    const id = pending.pop()!;
    if (id === toNode) return true;
    if (seen.has(id)) continue;
    seen.add(id);
    pending.push(...(upstream.get(id) ?? []));
  }
  return false;
}

export function portIsFree(doc: EditorDocument, to: PortRef): boolean {
  return !doc.wires.some((wire) => samePort(wire.to, to));
}

/**
 * Connect `from` into `to`, replacing whatever fed that input. Loops are
 * refused and return the document unchanged.
 */
export function connect(doc: EditorDocument, from: string, to: PortRef): EditorDocument {
  if (wouldCycle(doc, from, to.node)) return doc;
  const wires = doc.wires.filter((wire) => !samePort(wire.to, to));
  return { ...doc, wires: [...wires, { from, to }] };
}

export function disconnect(doc: EditorDocument, to: PortRef): EditorDocument {
  return { ...doc, wires: doc.wires.filter((wire) => !samePort(wire.to, to)) };
}

export function removeNodes(doc: EditorDocument, ids: Iterable<string>): EditorDocument {
  const gone = new Set(ids);
  return {
    ...doc,
    nodes: doc.nodes.filter((node) => !gone.has(node.id)),
    wires: doc.wires.filter((wire) => !gone.has(wire.from) && !gone.has(wire.to.node)),
  };
}

export function moveNodes(doc: EditorDocument, positions: Map<string, Position>): EditorDocument {
  if (positions.size === 0) return doc;
  return {
    ...doc,
    nodes: doc.nodes.map((node) => {
      const position = positions.get(node.id);
      return position ? { ...node, position } : node;
    }),
  };
}

export function setParam(doc: EditorDocument, id: string, name: string, value: ParamValue): EditorDocument {
  return {
    ...doc,
    nodes: doc.nodes.map((node) =>
      node.id === id ? { ...node, params: { ...node.params, [name]: value } } : node,
    ),
  };
}

/** Append a mix input with `weight`. */
export function addMixInput(doc: EditorDocument, id: string, weight: number): EditorDocument {
  const node = doc.nodes.find((candidate) => candidate.id === id);
  const weights = node?.params.weights;
  if (!node || !Array.isArray(weights)) return doc;
  return setParam(doc, id, "weights", [...weights, weight]);
}

/** Drop the last mix input and its wire; a mix keeps at least `min` inputs. */
export function removeMixInput(doc: EditorDocument, id: string, min: number): EditorDocument {
  const node = doc.nodes.find((candidate) => candidate.id === id);
  const weights = node?.params.weights;
  if (!node || !Array.isArray(weights) || weights.length <= min) return doc;
  const port = `in.${weights.length - 1}`;
  return disconnect(setParam(doc, id, "weights", weights.slice(0, -1)), { node: id, port });
}

export function samePort(a: PortRef, b: PortRef): boolean {
  return a.node === b.node && a.port === b.port;
}

export function wireId(wire: Wire): string {
  return `${wire.from}->${wire.to.node}:${wire.to.port}`;
}

export function parseDocument(text: string): EditorDocument {
  const value = JSON.parse(text) as Partial<EditorDocument>;
  if (value.format !== "sonicfusion-editor" || value.version !== 1) {
    throw new Error("not a SonicFusion editor project (version 1)");
  }
  if (!Array.isArray(value.nodes) || !Array.isArray(value.wires)) {
    throw new Error("project is missing nodes or wires");
  }
  return value as EditorDocument;
}

/** The patch from the approved Studio design, laid out the same way. */
export function studioDocument(graph: GraphSpec): EditorDocument {
  return fromGraph(graph, "sketch-01", {
    osc_a: { x: 32, y: 40 },
    osc_b: { x: 32, y: 196 },
    mix: { x: 272, y: 96 },
    lfo: { x: 272, y: 336 },
    lpf: { x: 512, y: 96 },
    gain: { x: 752, y: 96 },
    out: { x: 992, y: 96 },
  });
}
