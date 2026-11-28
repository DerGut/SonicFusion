import { describe, expect, it } from "vitest";

import catalogJson from "../../../crates/sonicfusion-graph-api/fixtures/catalog.json";
import studioPatch from "../../../crates/sonicfusion-graph-api/fixtures/studio-patch.json";
import { MockEngine } from "../api/mock";
import type { GraphSpec, NodeDescriptor } from "../api/types";
import { addNode, catalogMap, connect, inputPorts, parseDocument, setParam, studioDocument, toGraph } from "./document";
import { activeSteps, invertSteps, readSteps, setStep, shiftSteps, stepName, writeSteps } from "./pattern";
import { slowScopeSeconds } from "./scope";

const catalog = catalogMap(catalogJson as NodeDescriptor[]);
const TRESILLO = [1, 0, 0, 1, 0, 0, 1, 0, 0, 0, 1, 0, 1, 0, 0, 0];

function rising(samples: number[]): number[] {
  return samples.flatMap((sample, index) => (index > 0 && samples[index - 1] <= 0 && sample > 0 ? [index] : []));
}

describe("pattern model", () => {
  const steps = readSteps(TRESILLO, 16);

  it("reads only well-formed patterns and writes zeros and ones", () => {
    expect(activeSteps(steps)).toBe(5);
    expect(writeSteps(steps)).toEqual(TRESILLO);
    expect(activeSteps(readSteps([1, 0], 16))).toBe(0);
    expect(activeSteps(readSteps(undefined, 16))).toBe(0);
    expect(readSteps([...TRESILLO.slice(0, 15), 0.5], 16)[15]).toBe(false);
  });

  it("shifts with wrap-around, inverts and sets single steps", () => {
    expect(writeSteps(shiftSteps(steps, 1))).toEqual([0, ...TRESILLO.slice(0, 15)]);
    expect(writeSteps(shiftSteps(steps, -1))).toEqual([...TRESILLO.slice(1), 1]);
    expect(writeSteps(shiftSteps(shiftSteps(steps, 3), -3))).toEqual(TRESILLO);
    expect(activeSteps(invertSteps(steps))).toBe(11);
    expect(writeSteps(setStep(steps, 1, true)).slice(0, 4)).toEqual([1, 1, 0, 1]);
  });

  it("names steps by beat and 16th", () => {
    expect([0, 1, 2, 3, 4, 15].map(stepName)).toEqual([
      "Beat 1, 1", "Beat 1, e", "Beat 1, +", "Beat 1, a", "Beat 2, 1", "Beat 4, a",
    ]);
  });
});

describe("sequencer node", () => {
  it("is a source with an optional clock and a quarter-note default pattern", () => {
    const descriptor = catalog.get("sequencer")!;
    expect(descriptor.category).toBe("source");
    expect(descriptor.pattern).toEqual({ name: "steps", label: "Pattern", length: 16, default: [1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0] });
    const [doc, id] = addNode(studioDocument(studioPatch as GraphSpec), catalog, "sequencer", { x: 0, y: 0 });
    const node = doc.nodes.find((candidate) => candidate.id === id)!;
    expect(node.params).toEqual({ tempo_bpm: 120, gate_length: 0.5, steps: descriptor.pattern!.default });
    expect(node.params.steps).not.toBe(descriptor.pattern!.default);
    expect(inputPorts(node, catalog).map(({ name, signal, required }) => [name, signal, required])).toEqual([
      ["clock", "control", false],
    ]);
  });

  it("saves, reopens and plays 16ths at 120 BPM: eight rising edges per second", async () => {
    const [doc, seq] = addNode(studioDocument(studioPatch as GraphSpec), catalog, "sequencer", { x: 0, y: 0 });
    let patch = setParam(doc, seq, "steps", Array(16).fill(1));
    patch = connect(patch, seq, { node: "out", port: "in" });
    const reopened = parseDocument(JSON.stringify(patch));
    expect(reopened).toEqual(patch);

    const engine = new MockEngine();
    const graph = toGraph(reopened);
    expect(await engine.validate(graph)).not.toContainEqual(expect.objectContaining({ node: seq, severity: "error" }));
    const { samples } = await engine.probe(graph, seq, 48_000);
    expect(samples[0]).toBe(1);
    expect(rising(samples)).toHaveLength(7);
    // Half of each 6,000-frame step is open.
    expect(samples.slice(0, 6_000).filter((sample) => sample === 1)).toHaveLength(3_000);
  });

  it("follows a clock's rising edges and ignores the internal tempo", async () => {
    const [doc, seq] = addNode(studioDocument(studioPatch as GraphSpec), catalog, "sequencer", { x: 0, y: 0 });
    const [withClock, clock] = addNode(doc, catalog, "square", { x: 0, y: 0 });
    let patch = setParam(withClock, clock, "frequency_hz", 8);
    patch = setParam(patch, seq, "steps", [1, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    patch = setParam(patch, seq, "tempo_bpm", 20);
    patch = connect(patch, clock, { node: seq, port: "clock" });
    patch = connect(patch, seq, { node: "out", port: "in" });
    const graph = toGraph(patch);
    const engine = new MockEngine();
    // An 8 Hz square at 48 kHz rises every 6,000 frames: steps 0 and 2 are at 0 and 12,000.
    const { samples } = await engine.probe(graph, seq, 48_000);
    expect(samples[0]).toBe(1);
    expect(rising(samples)).toEqual([12_000]);
    expect(samples.slice(12_000, 18_000).filter((sample) => sample === 1)).toHaveLength(3_000);
    expect(slowScopeSeconds(graph, graph.nodes.find((node) => node.id === seq)!)).toBe(2);
  });

  it("shows one bar in the scope", () => {
    const [doc, seq] = addNode(studioDocument(studioPatch as GraphSpec), catalog, "sequencer", { x: 0, y: 0 });
    const node = (bpm: number) => toGraph(setParam(doc, seq, "tempo_bpm", bpm)).nodes.find((candidate) => candidate.id === seq)!;
    expect(slowScopeSeconds(toGraph(doc), node(120))).toBe(2);
    expect(slowScopeSeconds(toGraph(doc), node(240))).toBe(1);
    // Very slow tempos are capped at the scope's four-second limit.
    expect(slowScopeSeconds(toGraph(doc), node(20))).toBe(4);
  });

  it("reports malformed patterns, tempos and gate lengths", async () => {
    const [doc, seq] = addNode(studioDocument(studioPatch as GraphSpec), catalog, "sequencer", { x: 0, y: 0 });
    let patch = setParam(doc, seq, "steps", [1, 0.5]);
    patch = setParam(patch, seq, "tempo_bpm", 0);
    patch = setParam(patch, seq, "gate_length", 1.5);
    patch = connect(patch, seq, { node: "out", port: "in" });
    const issues = await new MockEngine().validate(toGraph(patch));
    for (const param of ["steps", "tempo_bpm", "gate_length"]) {
      expect(issues).toContainEqual(expect.objectContaining({ node: seq, param, severity: "error" }));
    }
  });
});
