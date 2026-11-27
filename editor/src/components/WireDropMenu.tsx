import { useEffect, useRef, useState } from "react";

import type { NodeKind } from "../api/types";
import type { Catalog, EditorDocument } from "../model/document";

/**
 * Opens where a wire was dropped on empty canvas. It lists only nodes with an
 * input; the pick is added there, already wired.
 */
export function WireDropMenu({ x, y, from, doc, catalog, onPick, onClose }: {
  x: number;
  y: number;
  from: string;
  doc: EditorDocument;
  catalog: Catalog;
  onPick: (kind: NodeKind) => void;
  onClose: () => void;
}) {
  const [filter, setFilter] = useState("");
  const ref = useRef<HTMLDivElement>(null);
  const hasOutput = doc.nodes.some((node) => node.kind === "output");
  const options = [...catalog.values()].filter(
    (descriptor) =>
      (descriptor.inputs.length > 0 || descriptor.variadic_input) &&
      descriptor.label.toLowerCase().includes(filter.toLowerCase()),
  );

  useEffect(() => {
    const close = (event: PointerEvent) => {
      if (!ref.current?.contains(event.target as Node)) onClose();
    };
    window.addEventListener("pointerdown", close);
    return () => window.removeEventListener("pointerdown", close);
  }, [onClose]);

  return (
    <div ref={ref} className="drop-menu" role="menu" aria-label={`Connect ${from} to`}
      style={{ left: Math.min(x + 8, window.innerWidth - 300), top: Math.min(y, window.innerHeight - 320) }}>
      <div className="drop-menu-title">Connect <code>{from} › out</code> to…</div>
      <input autoFocus type="search" aria-label="Filter nodes" placeholder="Filter" value={filter}
        onChange={(event) => setFilter(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === "Escape") onClose();
          if (event.key === "Enter" && options[0]) onPick(options[0].kind);
        }} />
      <div className="drop-menu-items">
        {options.map((descriptor) => (
          <button key={descriptor.kind} type="button" role="menuitem" onClick={() => onPick(descriptor.kind)}>
            <span>{descriptor.label}</span>
            <span className="hint">
              {descriptor.kind === "output" && hasOutput ? "replaces its input" : descriptor.signature}
            </span>
          </button>
        ))}
      </div>
      <p className="drop-menu-foot">Sources are hidden: they have no input.</p>
    </div>
  );
}
