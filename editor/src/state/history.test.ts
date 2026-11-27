import { describe, expect, it } from "vitest";

import type { EditorDocument } from "../model/document";
import { historyReducer } from "./history";

const doc = (name: string): EditorDocument => ({
  format: "sonicfusion-editor",
  version: 1,
  name,
  nodes: [],
  wires: [],
});

describe("history", () => {
  it("undoes and redoes recorded edits", () => {
    let state = historyReducer({ doc: doc("a"), past: [], future: [] }, { type: "edit", change: () => doc("b") });
    state = historyReducer(state, { type: "edit", change: () => doc("c") });
    state = historyReducer(state, { type: "undo" });
    expect(state.doc.name).toBe("b");
    state = historyReducer(state, { type: "redo" });
    expect(state.doc.name).toBe("c");
  });

  it("groups continuous edits behind one checkpoint", () => {
    let state = historyReducer({ doc: doc("a"), past: [], future: [] }, { type: "checkpoint" });
    for (const name of ["b", "c", "d"]) {
      state = historyReducer(state, { type: "edit", change: () => doc(name), record: false });
    }
    expect(state.past.length).toBe(1);
    expect(historyReducer(state, { type: "undo" }).doc.name).toBe("a");
  });

  it("ignores edits that change nothing", () => {
    const start = { doc: doc("a"), past: [], future: [] };
    expect(historyReducer(start, { type: "edit", change: (same) => same })).toBe(start);
  });
});
