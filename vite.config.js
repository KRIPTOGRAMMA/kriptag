import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

const host = process.env.TAURI_DEV_HOST;

export default defineConfig({
  plugins: [svelte()],
  clearScreen: false,
  build: {
    rollupOptions: {
      input: {
        main: "index.html",
        quickWindow: "quick-window.html",
      },
      output: {
        manualChunks: {
          codemirror: ["@codemirror/state", "@codemirror/view", "@codemirror/commands", "@codemirror/lang-markdown", "@codemirror/language", "@codemirror/autocomplete", "@lezer/common", "@lezer/markdown", "@lezer/highlight"],
        },
      },
    },
  },
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: "ws", host, port: 1421 } : undefined,
    watch: { ignored: ["**/src-tauri/**"] },
  },
});