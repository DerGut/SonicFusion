import type { EditorDocument } from "../model/document";

export interface History {
  doc: EditorDocument;
  past: EditorDocument[];
  future: EditorDocument[];
  /** Where a continuous edit (drag, slider) started, until it changes something. */
  pending?: EditorDocument;
}

export type HistoryAction =
  /** A user edit. `record: false` for steps of a continuous edit. */
  | { type: "edit"; change: (doc: EditorDocument) => EditorDocument; record?: boolean }
  /** A continuous edit is about to start; it undoes as one step. */
  | { type: "checkpoint" }
  | { type: "undo" }
  | { type: "redo" }
  | { type: "replace"; doc: EditorDocument };

const LIMIT = 100;

export function historyReducer(state: History, action: HistoryAction): History {
  switch (action.type) {
    case "edit": {
      const doc = action.change(state.doc);
      if (doc === state.doc) return state;
      if (action.record === false) {
        // Only the first real change of a continuous edit becomes a step.
        if (!state.pending) return { ...state, doc };
        return { doc, past: push(state.past, state.pending), future: [] };
      }
      return { doc, past: push(state.past, state.doc), future: [] };
    }
    case "checkpoint":
      return { ...state, pending: state.doc };
    case "undo": {
      const previous = state.past.at(-1);
      if (!previous) return state;
      return { doc: previous, past: state.past.slice(0, -1), future: [state.doc, ...state.future] };
    }
    case "redo": {
      const [next, ...future] = state.future;
      if (!next) return state;
      return { doc: next, past: push(state.past, state.doc), future };
    }
    case "replace":
      return { doc: action.doc, past: [], future: [] };
  }
}

function push(past: EditorDocument[], doc: EditorDocument) {
  return [...past, doc].slice(-LIMIT);
}
