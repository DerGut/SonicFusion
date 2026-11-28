import type { MockScopeRequest } from "./mock";
import type { ScopeWindow } from "./types";

export type MockScopeResponse = { ok: true; scope: ScopeWindow } | { ok: false; error: unknown };

export async function renderMockScopeInWorker(request: MockScopeRequest): Promise<ScopeWindow> {
  const worker = new Worker(new URL("./mockScope.worker.ts", import.meta.url), { type: "module" });
  try {
    return await new Promise<ScopeWindow>((resolve, reject) => {
      worker.onmessage = ({ data }: MessageEvent<MockScopeResponse>) => {
        if (data.ok) resolve(data.scope);
        else reject(data.error);
      };
      worker.onerror = (event) => reject(new Error(event.message || "Scope preview worker failed"));
      worker.onmessageerror = () => reject(new Error("Could not read the Scope preview worker response"));
      worker.postMessage(request);
    });
  } finally {
    worker.terminate();
  }
}
