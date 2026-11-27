import { describe, expect, it } from "vitest";

import catalogJson from "../../../crates/sonicfusion-graph-api/fixtures/catalog.json";
import studioPatch from "../../../crates/sonicfusion-graph-api/fixtures/studio-patch.json";
import type { GraphSpec, NodeDescriptor } from "../api/types";
import {
  addConnectedNode,
  addMixInput,
  addNode,
  catalogMap,
  connect,
  disconnect,
  inputPorts,
  moveWire,
  parseDocument,
  removeMixInput,
  removeNodes,
  studioDocument,
  toGraph,
  setFrequencyMode,
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

  it("round-trips a frequency display override only in the editor document", () => {
    const doc = studio();
    const changed = setFrequencyMode(doc, "osc_a", "hz");
    expect(toGraph(changed)).toEqual(toGraph(doc));
    const restored = parseDocument(JSON.stringify(changed));
    expect(restored.nodes.find((node) => node.id === "osc_a")?.frequencyMode).toBe("hz");
    expect(setFrequencyMode(restored, "osc_a")).toEqual(doc);
  });

  it("replaces the wire into a used input", () => {
    const doc = connect(studio(), "osc_b", { node: "mix", port: "in.0" });
    const into = doc.wires.filter((wire) => wire.to.node === "mix" && wire.to.port === "in.0");
    expect(into).toEqual([{ from: "osc_b", to: { node: "mix", port: "in.0" } }]);
  });

  it("moves a picked-up wire to another input or source", () => {
    const doc = studio();
    const lfoWire = doc.wires.find((wire) => wire.from === "lfo")!;
    const retargeted = moveWire(doc, lfoWire, "lfo", { node: "mix", port: "in.1" });
    expect(retargeted.wires).toContainEqual({ from: "lfo", to: { node: "mix", port: "in.1" } });
    expect(retargeted.wires.some((wire) => wire.to.port === "cutoff")).toBe(false);
    expect(retargeted.wires.filter((wire) => wire.to.node === "mix" && wire.to.port === "in.1")).toHaveLength(1);

    const resourced = moveWire(doc, lfoWire, "osc_b", { node: "lpf", port: "cutoff" });
    expect(resourced.wires).toContainEqual({ from: "osc_b", to: { node: "lpf", port: "cutoff" } });
    expect(resourced.wires.some((wire) => wire.from === "lfo")).toBe(false);

    const gainWire = doc.wires.find((wire) => wire.from === "gain")!;
    expect(moveWire(doc, gainWire, "gain", { node: "mix", port: "in.0" })).toBe(doc);
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

  it("adds a node from a dropped wire, already connected", () => {
    const [doc, id] = addConnectedNode(studio(), catalog, "low_pass", { x: 0, y: 0 }, "osc_a");
    expect(id).toBe("low_pass_1");
    expect(doc.wires).toContainEqual({ from: "osc_a", to: { node: "low_pass_1", port: "in" } });

    const [rewired, target] = addConnectedNode(studio(), catalog, "output", { x: 0, y: 0 }, "mix");
    expect(target).toBe("out");
    expect(rewired.nodes.filter((node) => node.kind === "output")).toHaveLength(1);
    expect(rewired.wires).toContainEqual({ from: "mix", to: { node: "out", port: "in" } });
  });

  it("rejects foreign and damaged files", () => {
    expect(() => parseDocument(JSON.stringify({ version: 1 }))).toThrow();
    const damaged = studio();
    delete (damaged.nodes[0] as Partial<(typeof damaged.nodes)[0]>).position;
    expect(() => parseDocument(JSON.stringify(damaged))).toThrow("malformed node");
    const unknown = { ...studio(), wires: [{ from: "a" }] };
    expect(() => parseDocument(JSON.stringify(unknown))).toThrow("malformed wire");
  });
});
