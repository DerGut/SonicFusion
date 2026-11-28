import { renderToStaticMarkup } from "react-dom/server";
import { expect, it } from "vitest";

import { MockEngine } from "../api/mock";
import { ConstantReadout } from "../components/ConstantReadout";
import { fromSlider, toSlider } from "../components/ParamControl";
import { constantDestinations } from "./constant";
import { addNode, catalogMap, connect, disconnect, fromGraph, parseDocument, setParam, toGraph } from "./document";

const info = { sample_rate_hz: 48000, batch_frames: 3, cutoff_modulation: { base_hz: 1000, depth_hz: 900 } };
const patch = () => fromGraph({
  version: 1,
  nodes: [
    { id: "dc", kind: "constant", params: { value: 0 } },
    { id: "sine", kind: "sine", params: { frequency_hz: 440 } },
    { id: "saw", kind: "sawtooth", params: { frequency_hz: 16000 } },
    { id: "lpf", kind: "low_pass", params: { cutoff_hz: 1200, wet: 1 } },
    { id: "out", kind: "output", params: {} },
  ],
  wires: [
    { from: "dc", to: { node: "sine", port: "frequency" } },
    { from: "dc", to: { node: "saw", port: "frequency" } },
    { from: "dc", to: { node: "lpf", port: "cutoff" } },
    { from: "sine", to: { node: "lpf", port: "in" } },
    { from: "lpf", to: { node: "out", port: "in" } },
  ],
}, "Constant", {});

it("maps each destination independently and refreshes for base, wiring and rate changes", () => {
  const doc = patch();
  for (const value of [-1, 0, 1]) {
    const destinations = constantDestinations(setParam(doc, "dc", "value", value), "dc", info);
    expect(destinations[0]).toMatchObject({ hz: 440 * 2 ** value, minHz: 220, maxHz: 880, capped: false });
    expect(destinations[2]).toMatchObject({ hz: 1000 + 900 * value, minHz: 100, maxHz: 1900 });
  }
  const high = constantDestinations(setParam(doc, "dc", "value", 1), "dc", info)[1];
  expect(high.hz).toBeLessThan(24000);
  expect(high.hz).toBeCloseTo(24000, 6);
  expect(high.capped).toBe(true);
  const fractional = constantDestinations(setParam(doc, "dc", "value", 0.1), "dc", info);
  expect(fractional[0].hz).toBe(440 * 2 ** Math.fround(0.1));
  const smallMapping = { ...info, cutoff_modulation: { base_hz: 1, depth_hz: 0.9 } };
  const small = constantDestinations(doc, "dc", smallMapping)[2];
  expect(small.hz).toBe(1);
  expect(small.minHz).toBeCloseTo(0.1);
  expect(small.maxHz).toBe(1.9);
  const faster = setParam(doc, "sine", "frequency_hz", 880);
  expect(constantDestinations(faster, "dc", info)[0].hz).toBe(880);
  const lowerRate = { ...info, sample_rate_hz: 44100 };
  expect(constantDestinations(setParam(doc, "dc", "value", 1), "dc", lowerRate)[1].hz).toBeLessThan(22050);
  const unplugged = disconnect(doc, { node: "sine", port: "frequency" });
  expect(constantDestinations(unplugged, "dc", info).map((target) => target.node)).toEqual(["saw", "lpf"]);
  expect(constantDestinations(setParam(doc, "sine", "frequency_hz", 0), "dc", info)[0]).toMatchObject({ hz: 0, minHz: 0, maxHz: 0 });
  for (const value of [NaN, Infinity, -1.01, 1.01]) expect(constantDestinations(setParam(doc, "dc", "value", value), "dc", info)).toEqual([]);
});

it("shows effective Hz, ranges, fan-out and a Nyquist cap in the readout", () => {
  const destinations = constantDestinations(setParam(patch(), "dc", "value", 1), "dc", info);
  const html = renderToStaticMarkup(<ConstantReadout destinations={destinations} />);
  expect(html).toContain("880.0 Hz");
  expect(html).toContain("1.90 kHz");
  expect(html).toContain("Range 220.0 Hz–880.0 Hz");
  expect(html).toContain("Capped below Nyquist");
  expect(renderToStaticMarkup(<ConstantReadout destinations={[]} />)).toContain("Connect to FM or cutoff");
});

it("adds a Constant from the catalog and saves, reopens, probes and applies its value", async () => {
  const engine = new MockEngine();
  const catalog = catalogMap(await engine.catalog());
  const descriptor = catalog.get("constant")!;
  expect(descriptor).toMatchObject({ category: "source", inputs: [], has_output: true });
  for (const value of [-1, -0.25, 0, 0.75, 1]) expect(fromSlider(descriptor.params[0], toSlider(descriptor.params[0], value))).toBe(value);
  const empty = fromGraph({ version: 1, nodes: [], wires: [] }, "DC", {});
  const [added, dc] = addNode(empty, catalog, "constant", { x: 0, y: 0 });
  const [withOutput, out] = addNode(added, catalog, "output", { x: 300, y: 0 });
  const doc = connect(withOutput, dc, { node: out, port: "in" });
  expect(doc.nodes[0].params.value).toBe(0);
  await engine.startSession(toGraph(doc));
  for (const value of [-1, -0.375, 0, 0.625, 1]) {
    const reopened = parseDocument(JSON.stringify(setParam(doc, dc, "value", value)));
    const graph = toGraph(reopened);
    expect(await engine.validate(graph)).toEqual([]);
    expect((await engine.probe(graph, dc, 17)).samples).toEqual(Array(17).fill(value));
    await expect(engine.applyGraph(graph)).resolves.toBeTypeOf("number");
  }
  await engine.stopSession();
  for (const value of [NaN, Infinity, -1.000001, 1.000001]) {
    expect(await engine.validate(toGraph(setParam(doc, dc, "value", value)))).toContainEqual(expect.objectContaining({ severity: "error", code: "invalid_parameter", param: "value" }));
  }
});
