import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import tailwindcss from "@tailwindcss/vite";

export default defineConfig({
  plugins: [svelte(), tailwindcss()],
  // Tauri expects a fixed port and prints its own logs.
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  build: { target: "es2022" },
});
