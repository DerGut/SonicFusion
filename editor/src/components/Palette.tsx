import { useState } from "react";

import type { NodeDescriptor, NodeKind } from "../api/types";
import type { Catalog } from "../model/document";
import { DRAG_KIND } from "./Canvas";
import { Icon, KindIcon } from "./icons";

const GROUPS: [NodeDescriptor["category"], string][] = [
  ["source", "Sources"],
  ["filter", "Filters"],
  ["utility", "Utility"],
  ["output", "Output"],
];

export function Palette({ catalog, hasOutput, onAdd }: {
  catalog: Catalog;
  hasOutput: boolean;
  onAdd: (kind: NodeKind) => void;
}) {
  const [query, setQuery] = useState("");
  const matches = (descriptor: NodeDescriptor) =>
    `${descriptor.label} ${descriptor.signature}`.toLowerCase().includes(query.trim().toLowerCase());
  return (
    <aside className="palette" aria-label="Node library">
      <div className="palette-head">
        <h2>Nodes</h2>
        <span className="hint">drag to canvas</span>
      </div>
      <label className="search">
        <Icon name="search" />
        <input type="search" aria-label="Search nodes" placeholder="Search nodes" value={query}
          onChange={(event) => setQuery(event.target.value)} />
      </label>
      {GROUPS.map(([category, title]) => {
        const items = [...catalog.values()].filter((descriptor) => descriptor.category === category && matches(descriptor));
        if (items.length === 0) return null;
        return (
          <section key={category} className="palette-group">
            <h3>{title}</h3>
            {items.map((descriptor) => {
              const taken = descriptor.kind === "output" && hasOutput;
              return (
                <button key={descriptor.kind} type="button" className="palette-item" draggable={!taken}
                  aria-disabled={taken} title={taken ? "A patch has one output" : "Click to add, or drag onto the canvas"}
                  onClick={() => !taken && onAdd(descriptor.kind)}
                  onDragStart={(event) => {
                    event.dataTransfer.setData(DRAG_KIND, descriptor.kind);
                    event.dataTransfer.effectAllowed = "copy";
                  }}>
                  <span className="palette-icon"><KindIcon kind={descriptor.kind} size={16} /></span>
                  <span className="palette-text">
                    <span>{descriptor.label}</span>
                    <code>{descriptor.signature}</code>
                  </span>
                  {taken && <span className="badge">in patch</span>}
                </button>
              );
            })}
          </section>
        );
      })}
      <p className="palette-foot">Drop a wire on empty canvas to add a node that's already connected.</p>
    </aside>
  );
}
