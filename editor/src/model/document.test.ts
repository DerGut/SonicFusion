import { describe, expect, it } from "vitest";

import catalogJson from "../../../crates/sonicfusion-graph-api/fixtures/catalog.json";
import studioPatch from "../../../crates/sonicfusion-graph-api/fixtures/studio-patch.json";
import type { GraphSpec, NodeDescriptor } from "../api/types";
import {
  addMixInput,
  addNode,
  catalogMap,
  connect,
  disconnect,
  inputPorts,
  parseDocument,
  removeMixInput,
  removeNodes,
  studioDocument,
  toGraph,
  wouldCycle,
} from "./document";

const catalog = catalogMap(catalogJson as NodeDescriptor[]);
const studio = () => studioDocument(studioPatch as GraphSpec);

describe("document", () => {
  it("round-trips the shared fixture without layout", () => {
    const doc = studio();
    expect(toGraph(doc)).toEqual(studioPatch);
    expect(parseDocument(JSON.stringify(doc))).toEqual(doc);
  });

  it("adds nodes with catalog defaults and unique ids", () => {
    let [doc, first] = addNode(studio(), catalog, "sine", { x: 0, y: 0 });
    let second: string;
    [doc, second] = addNode(doc, catalog, "sine", { x: 0, y: 0 });
    expect([first, second]).toEqual(["sine_1", "sine_2"]);
    expect(doc.nodes.at(-1)?.params).toEqual({ frequency_hz: 220 });
    const [withMix, mix] = addNode(doc, catalog, "mix", { x: 0, y: 0 });
    const node = withMix.nodes.find((candidate) => candidate.id === mix)!;
    expect(node.params.weights).toEqual([0.5, 0.5]);
    expect(inputPorts(node, catalog).map((port) => port.name)).toEqual(["in.0", "in.1"]);
  });

  it("replaces the wire into a used input", () => {
    const doc = connect(studio(), "osc_b", { node: "mix", port: "in.0" });
    const into = doc.wires.filter((wire) => wire.to.node === "mix" && wire.to.port === "in.0");
    expect(into).toEqual([{ from: "osc_b", to: { node: "mix", port: "in.0" } }]);
  });

  it("refuses wires that would close a loop", () => {
    const doc = studio();
    expect(wouldCycle(doc, "gain", "mix")).toBe(true);
    expect(wouldCycle(doc, "lpf", "lpf")).toBe(true);
    expect(wouldCycle(doc, "lfo", "gain")).toBe(false);
    expect(connect(doc, "gain", { node: "mix", port: "in.1" })).toBe(doc);
  });

  it("removes wires with their nodes and mix inputs", () => {
    const doc = removeNodes(studio(), ["lfo"]);
    expect(doc.wires.some((wire) => wire.from === "lfo")).toBe(false);
    const fewer = removeMixInput(doc, "mix", 1);
    expect(fewer.nodes.find((node) => node.id === "mix")?.params.weights).toEqual([0.6]);
    expect(fewer.wires.some((wire) => wire.to.port === "in.1")).toBe(false);
    expect(removeMixInput(fewer, "mix", 1)).toBe(fewer);
    const more = addMixInput(fewer, "mix", 0.5);
    expect(more.nodes.find((node) => node.id === "mix")?.params.weights).toEqual([0.6, 0.5]);
    expect(disconnect(more, { node: "out", port: "in" }).wires.length).toBe(more.wires.length - 1);
  });

  it("rejects foreign files", () => {
    expect(() => parseDocument(JSON.stringify({ version: 1 }))).toThrow();
  });
});
