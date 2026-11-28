import { expect, it } from "vitest";

import catalogJson from "../../../crates/sonicfusion-graph-api/fixtures/catalog.json";
import studioPatch from "../../../crates/sonicfusion-graph-api/fixtures/studio-patch.json";
import { MockEngine } from "../api/mock";
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
