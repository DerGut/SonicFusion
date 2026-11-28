import { useEffect, useRef, useState } from "react";

import type { NodeKind } from "../api/types";
import type { Catalog, EditorDocument } from "../model/document";

/**
 * Adds a node where it was opened. Dropped from a wire (`from`), it lists only
 * nodes with an input and adds the pick already wired; opened with Tab, it
 * lists every node.
 */
export function WireDropMenu({ x, y, from, doc, catalog, onPick, onClose }: {
  x: number;
  y: number;
  from?: string;
  doc: EditorDocument;
  catalog: Catalog;
  onPick: (kind: NodeKind) => void;
  onClose: () => void;
}) {
  const [filter, setFilter] = useState("");
  const [active, setActive] = useState(0);
  const ref = useRef<HTMLDivElement>(null);
  const hasOutput = doc.nodes.some((node) => node.kind === "output");
  const options = [...catalog.values()].filter(
    (descriptor) =>
      (from ? descriptor.inputs.length > 0 || descriptor.variadic_input : !(descriptor.kind === "output" && hasOutput)) &&
      `${descriptor.label} ${descriptor.signature}`.toLowerCase().includes(filter.trim().toLowerCase()),
  );
  const highlighted = Math.min(active, Math.max(0, options.length - 1));

  useEffect(() => {
    const close = (event: PointerEvent) => {
      if (!ref.current?.contains(event.target as Node)) onClose();
    };
    window.addEventListener("pointerdown", close);
    return () => window.removeEventListener("pointerdown", close);
  }, [onClose]);

  const title = from ? `Connect ${from} › out to` : "Add node";
  return (
    <div ref={ref} className="drop-menu" role="dialog" aria-label={title}
      style={{ left: Math.min(x + 8, window.innerWidth - 300), top: Math.min(y, window.innerHeight - 320) }}
      onKeyDown={(event) => {
        if (event.key === "Escape") {
          event.stopPropagation();
          onClose();
        } else if (event.key === "ArrowDown" || event.key === "ArrowUp") {
          event.preventDefault();
          const step = event.key === "ArrowDown" ? 1 : -1;
          setActive((highlighted + step + options.length) % Math.max(1, options.length));
        } else if (event.key === "Enter" && options[highlighted]) {
          event.preventDefault();
          onPick(options[highlighted].kind);
        }
      }}>
      <div className="drop-menu-title">
        {from ? <>Connect <code>{from} › out</code> to…</> : "Add node"}
      </div>
      <input autoFocus type="search" aria-label="Filter nodes" placeholder="Filter" value={filter}
        aria-controls="drop-menu-options" aria-activedescendant={options[highlighted] ? `drop-option-${options[highlighted].kind}` : undefined}
        onChange={(event) => {
          setFilter(event.target.value);
          setActive(0);
        }} />
      <ul id="drop-menu-options" className="drop-menu-items" role="listbox" aria-label="Nodes">
        {options.map((descriptor, index) => (
          <li key={descriptor.kind} id={`drop-option-${descriptor.kind}`} role="option" aria-selected={index === highlighted}>
            <button type="button" className={index === highlighted ? "active" : ""} tabIndex={-1}
              onMouseEnter={() => setActive(index)} onClick={() => onPick(descriptor.kind)}>
              <span>{descriptor.label}</span>
              <span className="hint">
                {descriptor.kind === "output" && hasOutput ? "replaces its input" : descriptor.signature}
              </span>
            </button>
          </li>
        ))}
      </ul>
      <p className="drop-menu-foot">
        {from ? "Only nodes with inputs are shown." : "↑↓ choose · ↵ add · Esc close"}
      </p>
    </div>
  );
}
