import { Handle, type NodeProps, Position } from "@xyflow/react";
import { memo, useState } from "react";

import type { PortDescriptor } from "../api/types";
import { wireId } from "../model/document";
import { noteName, pitchFromHz } from "../model/pitch";
import { formatHz } from "../model/signal";
import { IO_PORT_TOP, type SfNode, pickUpWire, rowPortTop } from "./flow";
import { KindIcon } from "./icons";
import { OutputMeter } from "./OutputMeter";
import { FrequencyModeSelect, ParamControl, type FrequencyMode } from "./ParamControl";
// PROTOTYPE #33/#41
import { EnvelopeBody } from "../prototype/envelope.prototype";
import { SequencerBody, useSequencerWide } from "../prototype/sequencer.prototype";
import { PrototypeFrequency, PrototypeTitle, useDocNodes, useModulatedReadout, useNodeLabel, useVariant } from "../prototype/design-33-41.prototype";

function NodeViewImpl({ data, selected }: NodeProps<SfNode>) {
  const { node, descriptor, ports, sources, issues, actions, session } = data;
  const [frequencyMode, setFrequencyMode] = useState<FrequencyMode>("hz");
  const pitchVariant = useVariant("pitch");
  const label = useNodeLabel();
  const modulatedReadout = useModulatedReadout();
  const nodes = useDocNodes();
  const wide = useSequencerWide(node);
  const errors = issues.filter((issue) => issue.severity === "error");
  const warning = issues.find((issue) => issue.severity === "warning");
  const portIssue = (port: string) => errors.find((issue) => issue.port === port)?.message;
  const paramIssue = (param: string) => errors.find((issue) => issue.param === param)?.message;
  const audioIn = ports.find((port) => port.name === "in");
  const className = [
    "sf-node",
    wide && "proto-wide",
    selected && "selected",
    errors.length > 0 && "has-error",
    warning && errors.length === 0 && "has-warning",
  ]
    .filter(Boolean)
    .join(" ");

  const rows: React.ReactNode[] = [];
  const handles: React.ReactNode[] = [];
  // A connected input picks up its wire instead of starting a new one.
  const inputHandle = (port: PortDescriptor, top: number) => {
    const from = sources[port.name];
    const wire = from ? wireId({ from, to: { node: node.id, port: port.name } }) : null;
    const pickUp = (event: React.PointerEvent | React.MouseEvent) => {
      if (wire && event.button === 0) pickUpWire(event, wire, "target");
    };
    return (
      <Handle key={port.name} id={port.name} type="target" position={Position.Left}
        className={`port ${port.signal}${from ? " connected" : ""}${portIssue(port.name) ? " port-error" : ""}`}
        style={{ top }} onPointerDownCapture={pickUp} onMouseDownCapture={pickUp}
        title={portIssue(port.name) ?? (from ? `${port.label}: drag to move or detach the wire from ${from}` : `${port.label} (${port.signal})`)} />
    );
  };
  // Option-drag on an output moves its newest wire; a plain drag adds one.
  const moveOutputWire = (event: React.PointerEvent | React.MouseEvent) => {
    if (event.button !== 0 || !event.altKey || data.outgoing.length === 0) return;
    pickUpWire(event, data.outgoing[data.outgoing.length - 1], "source");
  };

  if (descriptor.variadic_input) {
    const variadic = descriptor.variadic_input;
    const weights = (node.params[variadic.weight.name] as number[]) ?? [];
    weights.forEach((weight, index) => {
      const port = ports[index];
      rows.push(
        <ParamControl key={port.name} id={`${node.id}-${port.name}`} descriptor={variadic.weight}
          label={`${port.label} weight`} value={weight} invalid={paramIssue(variadic.weight.name)}
          onBeginContinuous={actions.beginContinuous}
          onChange={(value, continuous) => {
            const next = [...weights];
            next[index] = value;
            actions.setParam(node.id, variadic.weight.name, next, continuous);
          }} />,
      );
      handles.push(inputHandle(port, rowPortTop(index)));
    });
  }

  if (node.kind === "envelope") {
    const gate = ports.find((port) => port.name === "gate")!;
    const body = EnvelopeBody({
      node, descriptor, gateFrom: sources.gate, rowTop: rowPortTop,
      gateHandle: (top) => inputHandle(gate, top),
      paramIssue,
      setParam: (name, value, continuous) => actions.setParam(node.id, name, value, continuous),
      beginContinuous: actions.beginContinuous,
    });
    rows.push(...body.rows);
    handles.push(...body.handles);
  }

  if (node.kind === "sequencer") {
    const clock = ports.find((port) => port.name === "clock")!;
    const body = SequencerBody({
      node, descriptor, clockFrom: sources.clock, rowTop: rowPortTop,
      clockHandle: (top) => inputHandle(clock, top),
      setParam: (name, value, continuous) => actions.setParam(node.id, name, value, continuous),
      beginContinuous: actions.beginContinuous,
    });
    rows.push(...body.rows);
    handles.push(...body.handles);
  }

  if (node.kind !== "envelope" && node.kind !== "sequencer") descriptor.params.forEach((param) => {
    const index = rows.length;
    const control = ports.find((port) => port.signal === "control");
    const isCutoff = param.name === "cutoff_hz";
    const isOscillatorFrequency = param.name === "frequency_hz";
    const modulated = isCutoff && control && sources[control.name];
    if (modulated) {
      const { base_hz, depth_hz } = session.cutoff_modulation;
      const low = pitchFromHz(base_hz - depth_hz);
      const high = pitchFromHz(base_hz + depth_hz);
      const readout = modulatedReadout(base_hz, depth_hz, frequencyMode === "note" && low && high
        ? noteName(low.midi) + " – " + noteName(high.midi)
        : formatHz(base_hz) + " ± " + Math.round(depth_hz) + " Hz");
      rows.push(
        <div key={param.name} className="param modulated">
          <div className="param-top">
            <span>{param.label}</span>
            {pitchVariant === "A" && <FrequencyModeSelect label={param.label} mode={frequencyMode} onChange={setFrequencyMode} />}
          </div>
          <span className="value">{readout}</span>
          <p className="param-note">from {label(nodes.get(sources[control.name]), sources[control.name])} · fixed engine mapping</p>
        </div>,
      );
    } else if (pitchVariant !== "A" && (isCutoff || isOscillatorFrequency)) {
      rows.push(
        <PrototypeFrequency key={param.name} id={`${node.id}-${param.name}`} nodeId={node.id} descriptor={param}
          role={data.outgoing.some((wire) => wire.endsWith(":gate")) ? "gate" : data.outgoing.some((wire) => wire.endsWith(":cutoff")) ? "control" : "audio"}
          value={Number(node.params[param.name])} invalid={paramIssue(param.name)}
          onBeginContinuous={actions.beginContinuous}
          onChange={(value, continuous) => actions.setParam(node.id, param.name, value, continuous)} />,
      );
    } else {
      rows.push(
        <ParamControl key={param.name} id={`${node.id}-${param.name}`} descriptor={param}
          value={Number(node.params[param.name] ?? param.default)} invalid={paramIssue(param.name)}
          frequency={isCutoff || isOscillatorFrequency
            ? { mode: frequencyMode, onModeChange: setFrequencyMode, semitones: isOscillatorFrequency }
            : undefined}
          onBeginContinuous={actions.beginContinuous}
          onChange={(value, continuous) => actions.setParam(node.id, param.name, value, continuous)} />,
      );
    }
    if (isCutoff && control) handles.push(inputHandle(control, rowPortTop(index)));
  });

  return (
    <div className={className}>
      <div className="sf-node-header">
        <KindIcon kind={node.kind} />
        <PrototypeTitle node={node} kind={descriptor.signature.split("(")[0]} onRename={actions.renameNode} />
      </div>
      <div className="sf-node-io">
        <span>{audioIn ? "in" : ""}</span>
        <span>{descriptor.has_output ? "out" : node.kind === "output" ? "mono → device" : ""}</span>
      </div>
      {rows}
      {descriptor.variadic_input && (
        <div className="sf-node-footer nodrag">
          <button type="button" onClick={() => actions.addMixInput(node.id)}>+ Input</button>
          <button type="button" disabled={ports.length <= descriptor.variadic_input.min}
            onClick={() => actions.removeMixInput(node.id)}>− Input</button>
        </div>
      )}
      {node.kind === "output" && (
        <OutputMeter ceilingDb={data.ceilingDb} onCeiling={actions.setCeiling} />
      )}
      {(errors[0] ?? warning) && (
        <p className={`sf-node-issue ${errors[0] ? "error" : "warning"}`}>{(errors[0] ?? warning)!.message}</p>
      )}
      {audioIn && inputHandle(audioIn, IO_PORT_TOP)}
      {handles}
      {descriptor.has_output && (
        <Handle id="out" type="source" position={Position.Right} className="port audio connected"
          style={{ top: IO_PORT_TOP }} onPointerDownCapture={moveOutputWire} onMouseDownCapture={moveOutputWire}
          title={data.outgoing.length > 0 ? "Output: drag to add a wire, ⌥-drag to hand one to another output" : "Output: drag to connect"} />
      )}
    </div>
  );
}

export const NodeView = memo(NodeViewImpl);
