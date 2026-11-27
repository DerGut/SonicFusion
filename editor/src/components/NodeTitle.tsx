import { useRef, useState } from "react";

import type { DocNode } from "../model/document";
import { Icon } from "./icons";

export function NodeTitle({ node, onRename }: { node: DocNode; onRename: (id: string, name: string) => void }) {
  const [renaming, setRenaming] = useState(false);
  const restoreFocus = useRef(false);
  const title = node.displayName?.trim() || node.id;

  return (
    renaming ? (
      <input className="sf-node-title sf-node-title-input nodrag" autoFocus
        aria-label="Node display name" defaultValue={node.displayName ?? ""} placeholder={node.id}
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
        <span className="sf-node-title" title={title}>{title}</span>
        <button type="button" className="icon-button sf-node-rename nodrag nokey"
          title={`Rename ${title}`} aria-label={`Rename node ${title}`} onClick={() => setRenaming(true)}
          ref={(button) => {
            // Return from Enter/Escape to the button. A blur from Tab or
            // a click elsewhere should let focus continue on its way.
            if (button && restoreFocus.current) {
              restoreFocus.current = false;
              button.focus();
            }
          }}>
          <Icon name="edit" size={12} />
        </button>
      </>
    )
  );
}
