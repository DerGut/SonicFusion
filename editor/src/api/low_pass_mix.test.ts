import { expect, it } from "vitest";

import studioPatch from "../../../crates/sonicfusion-graph-api/fixtures/studio-patch.json";
import { parseDocument, setParam, studioDocument, toGraph } from "../model/document";
import { MockEngine } from "./mock";
import type { GraphSpec } from "./types";

it("restores a low-pass mix and blends the dry and wet probes", async () => {
  const engine = new MockEngine();
  const document = studioDocument(studioPatch as GraphSpec);
  const probe = async (graph: GraphSpec, node: string) => (await engine.probe(graph, node, 512)).samples;
  const dry = await probe(toGraph(document), "mix");
  const wet = await probe(toGraph(document), "lpf");
  expect(dry).not.toEqual(wet);
  expect(await probe(toGraph(setParam(document, "lpf", "mix", 0)), "lpf")).toEqual(dry);
  expect(await probe(toGraph(setParam(document, "lpf", "mix", 1)), "lpf")).toEqual(wet);

  const saved = JSON.stringify(setParam(document, "lpf", "mix", 0.375));
  const reopened = toGraph(parseDocument(saved));
  expect(reopened.nodes.find((node) => node.id === "lpf")?.params.mix).toBe(0.375);
  const blended = await probe(reopened, "lpf");
  expect(blended).toEqual(dry.map((sample, n) => Math.fround(0.625 * sample + 0.375 * wet[n])));
});

it("validates low-pass mix while cutoff modulation is wired", async () => {
  const engine = new MockEngine();
  const document = studioDocument(studioPatch as GraphSpec);
  const graph = toGraph(setParam(document, "lpf", "mix", 1.2));
  const issues = await engine.validate(graph);
  expect(issues).toContainEqual(expect.objectContaining({
    severity: "error",
    code: "invalid_parameter",
    node: "lpf",
    param: "mix",
  }));
});
