import type { EditorDocument } from "../model/document";

export interface History {
  doc: EditorDocument;
  past: EditorDocument[];
  future: EditorDocument[];
}

export type HistoryAction =
  /** A user edit. `record: false` for continuous ones such as dragging. */
  | { type: "edit"; change: (doc: EditorDocument) => EditorDocument; record?: boolean }
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
      if (action.record === false) return { ...state, doc };
      return { doc, past: [...state.past, state.doc].slice(-LIMIT), future: [] };
    }
    case "checkpoint":
      // Called before a continuous edit starts so it undoes as one step.
      return { ...state, past: [...state.past, state.doc].slice(-LIMIT), future: [] };
    case "undo": {
      const previous = state.past.at(-1);
      if (!previous) return state;
      return { doc: previous, past: state.past.slice(0, -1), future: [state.doc, ...state.future] };
    }
    case "redo": {
      const [next, ...future] = state.future;
      if (!next) return state;
      return { doc: next, past: [...state.past, state.doc], future };
    }
    case "replace":
      return { doc: action.doc, past: [], future: [] };
  }
}
