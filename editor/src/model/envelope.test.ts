import { expect, it } from "vitest";

import catalogJson from "../../../crates/sonicfusion-graph-api/fixtures/catalog.json";
import studioPatch from "../../../crates/sonicfusion-graph-api/fixtures/studio-patch.json";
import { MockEngine } from "../api/mock";
import { slowScopeSeconds } from "./scope";
import type { GraphSpec, NodeDescriptor } from "../api/types";
import { addNode, catalogMap, connect, inputPorts, parseDocument, setParam, studioDocument, toGraph } from "./document";

const catalog = catalogMap(catalogJson as NodeDescriptor[]);

it("persists envelope settings and wires, and shapes a square-gated audio signal", async () => {
  const doc = studioDocument(studioPatch as GraphSpec);
  const [withEnvelope, envelope] = addNode(doc, catalog, "envelope", { x: 0, y: 0 });
  const [withAudio, audio] = addNode(withEnvelope, catalog, "square", { x: 0, y: 0 });
  const [withGate, gate] = addNode(withAudio, catalog, "square", { x: 0, y: 0 });
  const processor = withGate.nodes.find((node) => node.id === envelope)!;
  expect(inputPorts(processor, catalog).map(({ name, signal, required }) => [name, signal, required])).toEqual([
    ["in", "audio", true],
    ["gate", "control", true],
  ]);

  let configured = setParam(withGate, audio, "frequency_hz", 0);
  configured = setParam(configured, audio, "pulse_width", 1);
  configured = setParam(configured, gate, "frequency_hz", 12_000);
  configured = setParam(configured, envelope, "attack_seconds", 0);
  configured = setParam(configured, envelope, "decay_seconds", 0);
  configured = setParam(configured, envelope, "sustain_level", 0.25);
  configured = setParam(configured, envelope, "release_seconds", 0);
  configured = connect(configured, audio, { node: envelope, port: "in" });
  configured = connect(configured, envelope, { node: "out", port: "in" });

  const engine = new MockEngine();
  const openGate = await engine.validate(toGraph(configured));
  expect(openGate).toContainEqual(expect.objectContaining({ code: "missing_input", node: envelope, port: "gate" }));
  configured = connect(configured, gate, { node: envelope, port: "gate" });
  const reopened = parseDocument(JSON.stringify(configured));
  expect(reopened).toEqual(configured);

  const graph = toGraph(reopened);
  const { samples } = await engine.probe(graph, "out", 8);
  expect(samples).toEqual([0.25, 0.25, 0, 0, 0.25, 0.25, 0, 0]);
  const scope = await engine.scope(graph, envelope, 1);
  expect([scope.frames, scope.bucket_frames, scope.bands.length]).toEqual([48_000, 480, 100]);
  const clocked = toGraph(setParam(reopened, gate, "frequency_hz", 2));
  const envNode = clocked.nodes.find((node) => node.id === envelope)!;
  const gateNode = clocked.nodes.find((node) => node.id === gate)!;
  expect(slowScopeSeconds(clocked, envNode)).toBe(1);
  expect(slowScopeSeconds(clocked, gateNode)).toBe(1);
  const gated = await engine.scope(clocked, envelope, 1);
  expect(gated.gate?.slice(0, 25)).toEqual(Array(25).fill(true));
  expect(gated.gate?.slice(25, 50)).toEqual(Array(25).fill(false));
  expect(gated.bands[49]).toEqual([0, 0]);
  const invalid = await engine.validate(toGraph(setParam(reopened, envelope, "sustain_level", 1.1)));
  expect(invalid).toContainEqual(expect.objectContaining({ code: "invalid_parameter", node: envelope, param: "sustain_level" }));

  // Saved documents can be incomplete. A powered gate must not start rendering
  // with a missing duration, even when only probing a branch of the draft.
  const saved = structuredClone(configured);
  delete saved.nodes.find((node) => node.id === envelope)!.params.attack_seconds;
  const malformed = toGraph(parseDocument(JSON.stringify(saved)));
  await expect(engine.probe(malformed, "out", 8)).rejects.toMatchObject({
    kind: "invalid",
    issues: [expect.objectContaining({ code: "invalid_parameter", node: envelope, param: "attack_seconds" })],
  });
});

it("bounds scope duration for slow oscillators and falls back for complex gates", () => {
  const clock = { id: "clock", kind: "square", params: { frequency_hz: 0.1, pulse_width: 0.5 } } as const;
  const env = { id: "env", kind: "envelope", params: {} } as const;
  const graph: GraphSpec = {
    version: 1,
    nodes: [clock, env],
    wires: [{ from: clock.id, to: { node: env.id, port: "gate" } }],
  };
  expect(slowScopeSeconds(graph, clock)).toBe(4);
  expect(slowScopeSeconds(graph, env)).toBe(4);
  const fast = { ...clock, params: { ...clock.params, frequency_hz: 12 } };
  expect(slowScopeSeconds(graph, fast)).toBe(0.25);
  expect(slowScopeSeconds(graph, { ...fast, params: { ...fast.params, frequency_hz: 20 } })).toBeNull();
  const dc = { ...clock, params: { ...clock.params, pulse_width: 1 } };
  expect(slowScopeSeconds({ ...graph, nodes: [dc, env] }, env)).toBe(1);
  expect(slowScopeSeconds({ ...graph, nodes: [{ id: clock.id, kind: "mix", params: { weights: [1] } }, env] }, env)).toBe(1);
});
