import { expect, it } from "vitest";

import { MockEngine } from "./mock";
import {
  addConnectedNode,
  addNode,
  catalogMap,
  connect,
  disconnect,
  fromGraph,
  inputPorts,
  parseDocument,
  setParam,
  toGraph,
} from "../model/document";

it("saves and probes a sawtooth with a steady or changing frequency input", async () => {
  const engine = new MockEngine();
  const catalog = catalogMap(await engine.catalog());
  const empty = fromGraph({ version: 1, nodes: [], wires: [] }, "Saw", {});
  const [withControl, control] = addNode(empty, catalog, "square", { x: 30, y: 30 });
  const [withSaw, saw] = addNode(withControl, catalog, "sawtooth", { x: 280, y: 30 });
  const [withOutput, output] = addNode(withSaw, catalog, "output", { x: 530, y: 30 });
  const port = { node: saw, port: "frequency" };
  expect(inputPorts(withOutput.nodes[1], catalog)).toEqual([
    { name: "frequency", label: "Frequency modulation", signal: "control", required: false },
  ]);

  let patch = connect(withOutput, control, port);
  patch = connect(patch, saw, { node: output, port: "in" });
  patch = setParam(setParam(patch, control, "frequency_hz", 0), saw, "frequency_hz", 6000);
  const saved = parseDocument(JSON.stringify(patch));
  expect(await engine.validate(toGraph(saved))).toEqual([]);
  expect((await engine.probe(toGraph(saved), output, 5)).samples).toEqual([-1, -0.5, 0, 0.5, -1]);

  const moving = setParam(saved, control, "frequency_hz", 12000);
  expect((await engine.probe(toGraph(moving), output, 8)).samples)
    .toEqual([-1, -0.5, 0, 0.125, 0.25, 0.75, -0.75, -0.625]);
  expect((await engine.probe(toGraph(disconnect(moving, port)), output, 5)).samples)
    .toEqual([-1, -0.75, -0.5, -0.25, 0]);
});

it("ignores an invalid saved frequency wire to a node without that port", async () => {
  const engine = new MockEngine();
  const doc = fromGraph({
    version: 1,
    nodes: [
      { id: "square", kind: "square", params: { frequency_hz: 220, pulse_width: 0.5 } },
      { id: "gain", kind: "gain", params: { factor: 0.5 } },
      { id: "out", kind: "output", params: {} },
    ],
    wires: [
      { from: "square", to: { node: "out", port: "in" } },
      { from: "gain", to: { node: "square", port: "frequency" } },
    ],
  }, "Unused input", {});
  const saved = toGraph(parseDocument(JSON.stringify(doc)));
  expect(await engine.validate(saved)).toEqual([
    expect.objectContaining({ severity: "warning", code: "unreachable", node: "gain" }),
  ]);
  const probe = await engine.probe(saved, "out", 16);
  expect(probe.samples.some(sample => sample !== 0)).toBe(true);
});


it("auto-connects a dropped wire to Sawtooth FM and still prefers audio ports", async () => {
  const catalog = catalogMap(await new MockEngine().catalog());
  const doc = fromGraph({
    version: 1,
    nodes: [{ id: "control", kind: "sine", params: { frequency_hz: 1 } }],
    wires: [],
  }, "Autowire", {});
  const [saw, oscillator] = addConnectedNode(doc, catalog, "sawtooth", { x: 300, y: 50 }, "control");
  expect(saw.wires).toEqual([{ from: "control", to: { node: oscillator, port: "frequency" } }]);

  const [patch, filter] = addConnectedNode(saw, catalog, "low_pass", { x: 550, y: 50 }, oscillator!);
  expect(patch.wires[1]).toEqual({ from: oscillator, to: { node: filter, port: "in" } });
});
