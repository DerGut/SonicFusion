import { expect, it } from "vitest";

import { fromSlider, SLIDER_STEPS, toSlider } from "../components/ParamControl";
import { fromGraph, parseDocument, setParam, toGraph } from "../model/document";
import { MockEngine } from "./mock";

it("offers mute, attenuation, unity and amplification on the Gain slider", async () => {
  const descriptor = (await new MockEngine().catalog()).find((node) => node.kind === "gain")!.params[0];
  expect(fromSlider(descriptor, 0)).toBe(0);
  expect(fromSlider(descriptor, SLIDER_STEPS)).toBe(4);
  for (const factor of [0, 0.1, 0.5, 1, 2, 4]) {
    const position = toSlider(descriptor, factor);
    expect(position).toBeGreaterThanOrEqual(0);
    expect(position).toBeLessThanOrEqual(SLIDER_STEPS);
    expect(fromSlider(descriptor, position)).toBeCloseTo(factor, 5);
  }
});

it("saves and probes gain below, at and above unity with bounded output", async () => {
  const engine = new MockEngine();
  const document = fromGraph({
    version: 1,
    nodes: [
      { id: "osc", kind: "sawtooth", params: { frequency_hz: 6000 } },
      { id: "quiet", kind: "gain", params: { factor: 0.25 } },
      { id: "gain", kind: "gain", params: { factor: 1 } },
      { id: "out", kind: "output", params: {} },
    ],
    wires: [
      { from: "osc", to: { node: "quiet", port: "in" } },
      { from: "quiet", to: { node: "gain", port: "in" } },
      { from: "gain", to: { node: "out", port: "in" } },
    ],
  }, "Gain", {});
  const input = (await engine.probe(toGraph(document), "quiet", 8)).samples;
  for (const factor of [0, 0.5, 1, 2, 4, 8]) {
    const restored = toGraph(parseDocument(JSON.stringify(setParam(document, "gain", "factor", factor))));
    expect(restored.nodes.find((node) => node.id === "gain")!.params.factor).toBe(factor);
    expect(await engine.validate(restored)).toEqual([]);
    expect((await engine.probe(restored, "out", 8)).samples)
      .toEqual(input.map((sample) => Math.fround(Math.max(-1, Math.min(1, sample * factor)))));
  }
});
