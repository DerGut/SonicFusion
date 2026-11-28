import { describe, expect, it } from "vitest";

import catalogJson from "../../../crates/sonicfusion-graph-api/fixtures/catalog.json";
import studioPatch from "../../../crates/sonicfusion-graph-api/fixtures/studio-patch.json";
import { MockEngine } from "../api/mock";
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
  nodeLabel,
  parseDocument,
  removeMixInput,
  removeNodes,
  renameNode,
  studioDocument,
  toGraph,
  setFrequencyMode,
  setParam,
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

  it("round-trips duplicate display names without changing the graph", () => {
    const doc = studio();
    const named = renameNode(renameNode(doc, "lfo", "  LFO  "), "osc_b", "LFO");
    const reopened = parseDocument(JSON.stringify(named));
    expect(reopened.nodes.filter((node) => node.displayName === "LFO").map((node) => node.id))
      .toEqual(["osc_b", "lfo"]);
    expect(reopened.nodes.filter((node) => node.displayName === "LFO").map(nodeLabel))
      .toEqual(["LFO (osc_b)", "LFO (lfo)"]);
    expect(toGraph(reopened)).toEqual(toGraph(doc));
    expect(renameNode(named, "lfo", "LFO")).toBe(named);
    expect(renameNode(named, "missing", "LFO")).toBe(named);
  });

  it("clears a display name without changing the node's default title", () => {
    const doc = studio();
    const named = renameNode(doc, "lfo", "Modulator");
    const cleared = renameNode(named, "lfo", "  ");
    expect(parseDocument(JSON.stringify(cleared))).toEqual(doc);
    expect(nodeLabel(cleared.nodes.find((node) => node.id === "lfo")!)).toBe("lfo");
    expect(renameNode(cleared, "lfo", "")).toBe(cleared);
  });

  it("rejects a saved project with a malformed display name", () => {
    const doc = studio();
    const damaged = { ...doc, nodes: [{ ...doc.nodes[0], displayName: 42 }, ...doc.nodes.slice(1)] };
    expect(() => parseDocument(JSON.stringify(damaged))).toThrow("malformed node");
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

  it("saves, reopens and probes a patch with a Noise source", async () => {
    const [withNoise, id] = addNode(studio(), catalog, "noise", { x: 0, y: 0 });
    const source = withNoise.nodes.find((node) => node.id === id)!;
    expect(source.params).toEqual({});
    expect(inputPorts(source, catalog)).toEqual([]);
    const doc = connect(withNoise, id, { node: "out", port: "in" });
    const reopened = parseDocument(JSON.stringify(doc));
    const graph = toGraph(reopened);
    expect(reopened).toEqual(doc);
    const engine = new MockEngine();
    expect((await engine.validate(graph)).every((issue) => issue.severity !== "error")).toBe(true);
    const sourceProbe = await engine.probe(graph, id, 256);
    const outputProbe = await engine.probe(graph, "out", 256);
    expect(outputProbe.samples).toEqual(sourceProbe.samples);
    expect(outputProbe.samples.some((sample) => sample < -0.5)).toBe(true);
    expect(outputProbe.samples.some((sample) => sample > 0.5)).toBe(true);
  });

  it("keeps mock Noise sources distinct when their node IDs hash to the same value", async () => {
    const graph = toGraph(studio());
    graph.nodes.push(
      { id: "liquid", kind: "noise", params: {} },
      { id: "costarring", kind: "noise", params: {} },
    );
    const engine = new MockEngine();
    const first = await engine.probe(graph, "liquid", 64);
    const second = await engine.probe(graph, "costarring", 64);
    expect(first.samples).not.toEqual(second.samples);
    const reordered = { ...graph, nodes: [...graph.nodes].reverse() };
    expect((await engine.probe(reordered, "liquid", 64)).samples).toEqual(first.samples);
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

  it.each(["hz", "bpm"] as const)("round-trips a %s display override only in the editor document", (mode) => {
    const doc = studio();
    const changed = setFrequencyMode(doc, "osc_a", mode);
    expect(toGraph(changed)).toEqual(toGraph(doc));
    const restored = parseDocument(JSON.stringify(changed));
    expect(restored.nodes.find((node) => node.id === "osc_a")?.frequencyMode).toBe(mode);
    expect(setFrequencyMode(restored, "osc_a")).toEqual(doc);
  });

  it("does not write a parameter when its value is already set", () => {
    const doc = studio();
    expect(setParam(doc, "osc_a", "frequency_hz", 220)).toBe(doc);
  });
});
