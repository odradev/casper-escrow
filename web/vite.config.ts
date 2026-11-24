import { defineConfig } from "vite";

export default defineConfig({
  // Relative asset paths, so the build works from any directory of a static host.
  base: "./",
  build: { target: "es2022" },
  // The wasm packages load their .wasm files relative to their own modules: leave them as they are.
  optimizeDeps: { exclude: ["escrow-wasm-client", "casper-escrow-storage-reader"] },
  server: {
    // The wasm packages and the contract schemas live above web/.
    fs: { allow: [".."] },
    // A local NCTL node sends no CORS headers: the dev server passes the requests on.
    proxy: {
      "/nctl/rpc": { target: "http://localhost:11101", rewrite: () => "/rpc" },
    },
  },
});
