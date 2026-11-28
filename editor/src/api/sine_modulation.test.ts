import { expect, it } from "vitest";

import { MockEngine } from "./mock";
import { addConnectedNode, catalogMap, disconnect, fromGraph, inputPorts, parseDocument, setParam, toGraph } from "../model/document";

it("saves Sine FM and probes steady and varying pitch with a fixed-frequency fallback", async () => {
  const engine = new MockEngine();
  const document = fromGraph({
    version: 1,
    nodes: [
      { id: "control", kind: "square", params: { frequency_hz: 0, pulse_width: 0.5 } },
      { id: "sine", kind: "sine", params: { frequency_hz: 6000 } },
      { id: "out", kind: "output", params: {} },
    ],
    wires: [
      { from: "control", to: { node: "sine", port: "frequency" } },
      { from: "sine", to: { node: "out", port: "in" } },
    ],
  }, "Sine FM", {});
  const catalog = catalogMap(await engine.catalog());
  expect(inputPorts(document.nodes[1], catalog)).toEqual([
    { name: "frequency", label: "Frequency modulation", signal: "control", required: false },
  ]);
  const saved = parseDocument(JSON.stringify(document));
  expect(await engine.validate(toGraph(saved))).toEqual([]);
  const check = async (patch: typeof document, phases: number[]) => {
    const samples = (await engine.probe(toGraph(patch), "out", phases.length)).samples;
    samples.forEach((sample, n) => expect(sample).toBeCloseTo(Math.sin(2 * Math.PI * phases[n]), 6));
  };
  await check(saved, [0, 0.25, 0.5, 0.75, 0]);
  const moving = setParam(saved, "control", "frequency_hz", 12000);
  await check(moving, [0, 0.25, 0.5, 0.5625, 0.625, 0.875, 0.125, 0.1875]);
  await check(disconnect(moving, { node: "sine", port: "frequency" }), [0, 0.125, 0.25, 0.375, 0.5]);
});

it("auto-connects a dropped wire to Sine FM", async () => {
  const catalog = catalogMap(await new MockEngine().catalog());
  const document = fromGraph({
    version: 1,
    nodes: [{ id: "control", kind: "sine", params: { frequency_hz: 1 } }],
    wires: [],
  }, "Autowire", {});
  const [patch, sine] = addConnectedNode(document, catalog, "sine", { x: 300, y: 50 }, "control");
  expect(patch.wires).toEqual([{ from: "control", to: { node: sine, port: "frequency" } }]);
});
