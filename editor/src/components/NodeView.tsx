import { Handle, type NodeProps, Position } from "@xyflow/react";
import { memo } from "react";

import type { NodeSpec, PortDescriptor } from "../api/types";
import { wireId } from "../model/document";
import { formatHz } from "../model/signal";
import { IO_PORT_TOP, type SfNode, pickUpWire, rowPortTop } from "./flow";
import { KindIcon } from "./icons";
import { NodeTitle } from "./NodeTitle";
import { OutputMeter } from "./OutputMeter";
import { ParamControl } from "./ParamControl";
import { FrequencyControl } from "./FrequencyControl";

function NodeViewImpl({ data, selected }: NodeProps<SfNode>) {
  const { node, descriptor, ports, sources, issues, actions, session } = data;
  const errors = issues.filter((issue) => issue.severity === "error");
  const warning = issues.find((issue) => issue.severity === "warning");
  const portIssue = (port: string) => errors.find((issue) => issue.port === port)?.message;
  const paramIssue = (param: string) => errors.find((issue) => issue.param === param)?.message;
  const audioIn = ports.find((port) => port.name === "in");
  const frequencyIn = ports.find((port) => port.name === "frequency");
  const className = [
    "sf-node",
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
    const wire = from ? wireId({ from: from.id, to: { node: node.id, port: port.name } }) : null;
    const pickUp = (event: React.PointerEvent | React.MouseEvent) => {
      if (wire && event.button === 0) pickUpWire(event, wire, "target");
    };
    return (
      <Handle key={port.name} id={port.name} type="target" position={Position.Left}
        className={`port ${port.signal}${from ? " connected" : ""}${portIssue(port.name) ? " port-error" : ""}`}
        style={{ top }} onPointerDownCapture={pickUp} onMouseDownCapture={pickUp}
        title={portIssue(port.name) ?? (from ? `${port.label}: drag to move or detach the wire from ${from.label}` : `${port.label} (${port.signal})`)} />
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

  const gate = ports.find((port) => port.name === "gate");
  if (gate) {
    const source = sources[gate.name];
    handles.push(inputHandle(gate, rowPortTop(rows.length)));
    rows.push(
      <div key={gate.name} className="param modulated envelope-gate">
        <div className="param-top">
          <span>{gate.label}</span>
          <EnvelopeThumbnail params={node.params} />
          <span title="The gate opens above zero and closes at zero or below">opens &gt; 0</span>
        </div>
        <p className="param-note" title={source?.label}>
          {source ? `from ${source.label}` : "Not connected · output stays silent"}
        </p>
      </div>,
      <div key="adsr" className="envelope-params">
        {descriptor.params.map((param) => (
          <ParamControl key={param.name} id={`${node.id}-${param.name}`} descriptor={param}
            value={Number(node.params[param.name])} invalid={paramIssue(param.name)}
            onBeginContinuous={actions.beginContinuous}
            onChange={(value, continuous) => actions.setParam(node.id, param.name, value, continuous)} />
        ))}
      </div>,
    );
  }

  if (!gate) descriptor.params.forEach((param) => {
    const index = rows.length;
    const control = ports.find((port) => port.signal === "control");
    const isCutoff = param.frequency === "cutoff";
    const modulated = isCutoff && control && sources[control.name];
    if (modulated) {
      const { base_hz, depth_hz } = session.cutoff_modulation;
      const readout = formatHz(base_hz) + " ± " + Math.round(depth_hz) + " Hz";
      rows.push(
        <div key={param.name} className="param modulated">
          <div className="param-top">
            <span>{param.label}</span>
            <span className="value">{readout}</span>
          </div>
          <p className="param-note" title={`from ${modulated.label} · fixed engine mapping`}>
            from {modulated.label} · fixed engine mapping
          </p>
        </div>,
      );
    } else {
      const props = {
        id: `${node.id}-${param.name}`,
        descriptor: param,
        value: Number(node.params[param.name] ?? param.default),
        invalid: paramIssue(param.name),
        onBeginContinuous: actions.beginContinuous,
        onChange: (value: number, continuous: boolean) => actions.setParam(node.id, param.name, value, continuous),
      };
      rows.push(param.frequency ? (
        <FrequencyControl key={param.name} {...props} sampleRate={session.sample_rate_hz}
          role={data.frequencyRole} override={node.frequencyMode}
          onOverride={(mode) => actions.setFrequencyMode(node.id, mode)} />
      ) : <ParamControl key={param.name} {...props} />);
    }
    if (isCutoff && control) handles.push(inputHandle(control, rowPortTop(index)));
  });

  return (
    <div className={className}>
      <div className="sf-node-header">
        <KindIcon kind={node.kind} />
        <NodeTitle node={node} onRename={actions.renameNode} />
        <span className="sf-node-kind">{descriptor.signature.split("(")[0]}</span>
      </div>
      <div className="sf-node-io">
        {frequencyIn ? (
          <span title="Control −1 halves the base frequency, 0 leaves it alone, and +1 doubles it. The result stays below Nyquist.">
            FM ±1 octave
          </span>
        ) : <span>{audioIn ? "in" : ""}</span>}
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
      {frequencyIn && inputHandle(frequencyIn, IO_PORT_TOP)}
      {handles}
      {descriptor.has_output && (
        <Handle id="out" type="source" position={Position.Right} className="port audio connected"
          style={{ top: IO_PORT_TOP }} onPointerDownCapture={moveOutputWire} onMouseDownCapture={moveOutputWire}
          title={data.outgoing.length > 0 ? "Output: drag to add a wire, ⌥-drag to hand one to another output" : "Output: drag to connect"} />
      )}
    </div>
  );
}

/** Relative log widths keep short stages visible in the thumbnail. */
function EnvelopeThumbnail({ params }: { params: NodeSpec["params"] }) {
  const width = (name: string) => {
    const seconds = Number(params[name]);
    return 1 + Math.log10(1 + 1_000 * (Number.isFinite(seconds) ? Math.min(10, Math.max(seconds, 0)) : 0));
  };
  const attack = width("attack_seconds");
  const decay = width("decay_seconds");
  const release = width("release_seconds");
  const held = 2;
  const dx = 54 / (attack + decay + held + release);
  const sustain = Number(params.sustain_level);
  const y = 15 - 14 * (Number.isFinite(sustain) ? Math.min(1, Math.max(sustain, 0)) : 0);
  return (
    <svg className="envelope-thumbnail" width="56" height="16" viewBox="0 0 56 16" aria-hidden="true">
      <path d={`M1 15 L${1 + attack * dx} 1 L${1 + (attack + decay) * dx} ${y} L${55 - release * dx} ${y} L55 15`} />
    </svg>
  );
}

export const NodeView = memo(NodeViewImpl);
