import { describe, expect, it } from "vitest";

import type { EditorDocument } from "../model/document";
import { type History, historyReducer } from "./history";

const doc = (name: string): EditorDocument => ({
  format: "sonicfusion-editor",
  version: 1,
  name,
  nodes: [],
  wires: [],
});

const start = (): History => ({ doc: doc("a"), past: [], future: [] });

describe("history", () => {
  it("undoes and redoes recorded edits", () => {
    let state = historyReducer(start(), { type: "edit", change: () => doc("b") });
    state = historyReducer(state, { type: "edit", change: () => doc("c") });
    state = historyReducer(state, { type: "undo" });
    expect(state.doc.name).toBe("b");
    state = historyReducer(state, { type: "redo" });
    expect(state.doc.name).toBe("c");
  });

  it("groups continuous edits behind one checkpoint", () => {
    let state = historyReducer(start(), { type: "checkpoint" });
    for (const name of ["b", "c", "d"]) {
      state = historyReducer(state, { type: "edit", change: () => doc(name), record: false });
    }
    expect(state.past.length).toBe(1);
    expect(historyReducer(state, { type: "undo" }).doc.name).toBe("a");
  });

  it("doesn't record checkpoints that change nothing", () => {
    let state = historyReducer(start(), { type: "edit", change: () => doc("b") });
    state = historyReducer(state, { type: "undo" });
    state = historyReducer(state, { type: "checkpoint" });
    state = historyReducer(state, { type: "checkpoint" });
    expect(state.past.length).toBe(0);
    expect(state.future.length).toBe(1);
  });

  it("ignores edits that change nothing", () => {
    const initial = start();
    expect(historyReducer(initial, { type: "edit", change: (same) => same })).toBe(initial);
  });
});
