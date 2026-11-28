import { renderMockScope, type MockScopeRequest } from "./mock";
import type { MockScopeResponse } from "./mockScope";

self.onmessage = ({ data }: MessageEvent<MockScopeRequest>) => {
  let response: MockScopeResponse;
  try {
    response = { ok: true, scope: renderMockScope(data) };
  } catch (error) {
    response = { ok: false, error };
  }
  self.postMessage(response);
};
