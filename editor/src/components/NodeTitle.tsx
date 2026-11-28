import { useRef, useState } from "react";

import { type DocNode, nodeLabel } from "../model/document";

export function NodeTitle({ node, onRename, inProbe = false }: {
  node: DocNode;
  onRename: (id: string, name: string) => void;
  inProbe?: boolean;
}) {
  const [renaming, setRenaming] = useState(false);
  const restoreFocus = useRef(false);
  const name = node.displayName?.trim();
  const label = nodeLabel(node);
  const titleClass = inProbe ? "probe-title" : `sf-node-title${name ? "" : " sf-node-id"}`;

  return (
    renaming ? (
      <input className={inProbe ? "probe-title-input" : "sf-node-title sf-node-title-input nodrag"}
        autoFocus aria-label={`Display name for ${node.id}`} defaultValue={name ?? ""} placeholder={node.id}
        onFocus={(event) => event.currentTarget.select()}
        onBlur={(event) => {
          if (!restoreFocus.current) onRename(node.id, event.currentTarget.value);
          setRenaming(false);
        }}
        onKeyDown={(event) => {
          if (event.nativeEvent.isComposing) return;
          if (event.key === "Enter" || event.key === "Escape") {
            event.preventDefault();
            event.stopPropagation();
            restoreFocus.current = true;
            if (event.key === "Enter") onRename(node.id, event.currentTarget.value);
            setRenaming(false);
          }
        }} />
    ) : (
      <>
        <button type="button" className={`node-title-button ${titleClass}`}
          title={`${label} · ${inProbe ? "click" : "double-click"} to rename`} aria-label={`Rename node ${label}`}
          onClick={(event) => {
            // Native keyboard activation has no click count. Pointer clicks
            // on the canvas title retain the usual node selection and drag.
            if (inProbe || event.detail === 0) setRenaming(true);
          }}
          onDoubleClick={inProbe ? undefined : () => setRenaming(true)}
          onKeyDown={(event) => {
            if (event.key === "Enter" || event.key === " ") event.stopPropagation();
          }}
          ref={(button) => {
            if (button && restoreFocus.current) {
              restoreFocus.current = false;
              button.focus();
            }
          }}>
          {inProbe ? `${label} › out` : name || node.id}
        </button>
        {!inProbe && name && <span className="sf-node-id sf-node-id-chip" title={node.id}>{node.id}</span>}
      </>
    )
  );
}
