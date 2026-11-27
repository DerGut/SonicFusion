import { useEffect, useRef, useState } from "react";

/** Shows the DSL the engine would run, for `live_runtime` or the preview tool. */
export function ExportDialog({ name, source, onClose }: { name: string; source: string; onClose: () => void }) {
  const [copied, setCopied] = useState(false);
  const dialog = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    dialog.current?.showModal();
  }, []);
  return (
    <dialog ref={dialog} className="export" onClose={onClose} aria-label="Export .sf">
      <h2>{name}.sf</h2>
      <p className="hint">Save this as a .sf file and run it with <code>live_runtime</code> or <code>scripts/preview</code>.</p>
      <textarea readOnly value={source} rows={Math.min(16, source.split("\n").length + 1)} aria-label="DSL source" />
      <div className="export-actions">
        <button type="button" className="button secondary" onClick={() => {
          void navigator.clipboard?.writeText(source).then(() => setCopied(true));
        }}>{copied ? "Copied" : "Copy"}</button>
        <button type="button" className="button primary" onClick={() => dialog.current?.close()}>Done</button>
      </div>
    </dialog>
  );
}
