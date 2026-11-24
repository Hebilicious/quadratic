import { fileURLToPath, URL } from "node:url";

import cssforge from "@hebilicious/cssforge-unplugin/vite";
import { vize } from "@vizejs/vite-plugin";
import { defineConfig } from "vite";

const here = (path: string) => fileURLToPath(new URL(path, import.meta.url));

export default defineConfig({
  build: {
    emptyOutDir: true,
    outDir: here("dist"),
    target: "es2024",
  },
  plugins: [
    cssforge({ config: here("cssforge.config.ts") }),
    vize({
      configFile: here("vize.config.ts"),
      scanPatterns: ["src/**/*.vue"],
      vapor: true,
    }),
  ],
  root: here("."),
});
