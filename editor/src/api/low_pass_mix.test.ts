import { expect, it } from "vitest";

import studioPatch from "../../../crates/sonicfusion-graph-api/fixtures/studio-patch.json";
import { formatValue } from "../components/ParamControl";
import { parseDocument, setParam, studioDocument, toGraph } from "../model/document";
import { MockEngine } from "./mock";
import type { GraphSpec } from "./types";

it("restores Dry/Wet and blends the dry and wet probes", async () => {
  const engine = new MockEngine();
  const document = studioDocument(studioPatch as GraphSpec);
  const probe = async (graph: GraphSpec, node: string) => (await engine.probe(graph, node, 512)).samples;
  const dry = await probe(toGraph(document), "mix");
  const wet = await probe(toGraph(document), "lpf");
  expect(dry).not.toEqual(wet);
  expect(await probe(toGraph(setParam(document, "lpf", "wet", 0)), "lpf")).toEqual(dry);
  expect(await probe(toGraph(setParam(document, "lpf", "wet", 1)), "lpf")).toEqual(wet);

  const saved = JSON.stringify(setParam(document, "lpf", "wet", 0.375));
  const reopened = toGraph(parseDocument(saved));
  expect(reopened.nodes.find((node) => node.id === "lpf")?.params.wet).toBe(0.375);
  const blended = await probe(reopened, "lpf");
  expect(blended).toEqual(dry.map((sample, n) => Math.fround(0.625 * sample + 0.375 * wet[n])));
});

it("validates Dry/Wet while cutoff modulation is wired", async () => {
  const engine = new MockEngine();
  const document = studioDocument(studioPatch as GraphSpec);
  const graph = toGraph(setParam(document, "lpf", "wet", 1.2));
  const issues = await engine.validate(graph);
  expect(issues).toContainEqual(expect.objectContaining({
    severity: "error",
    code: "invalid_parameter",
    node: "lpf",
    param: "wet",
  }));
});

it("migrates the old Low-pass mix key without changing the Mix node", async () => {
  const engine = new MockEngine();
  const document = studioDocument(studioPatch as GraphSpec);
  const old = setParam(document, "lpf", "mix", 0);
  const restored = toGraph(parseDocument(JSON.stringify(old)));
  expect(restored.nodes.find((node) => node.id === "lpf")?.params).toEqual({ cutoff_hz: 1200, wet: 0 });
  expect(restored.nodes.find((node) => node.id === "mix")?.params).toEqual({ weights: [0.6, 0.4] });
  const dry = (await engine.probe(toGraph(document), "mix", 512)).samples;
  expect((await engine.probe(restored, "lpf", 512)).samples).toEqual(dry);
  expect((await engine.probe(toGraph(old), "lpf", 512)).samples).toEqual(dry);
});

it("distinguishes near-endpoint wet ratios from bypass and full wet", async () => {
  const catalog = await new MockEngine().catalog();
  const wet = catalog.find((node) => node.kind === "low_pass")!.params.find((param) => param.name === "wet")!;
  expect([0, 0.0001, 0.7, 0.9999, 1].map((ratio) => formatValue(wet, ratio)))
    .toEqual(["0% wet", "<0.1% wet", "70% wet", ">99.9% wet", "100% wet"]);
});
