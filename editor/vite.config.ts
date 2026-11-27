import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

// The catalog and fixtures live in the Rust graph-api crate, one level up.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: { port: 5173, strictPort: true, fs: { allow: [".."] } },
  envPrefix: ["VITE_", "TAURI_ENV_"],
  test: { environment: "node" },
});
