import { defineConfig } from "vize";

export default defineConfig({
  compiler: {
    vapor: true,
  },
  files: ["src/**/*.vue", "src/**/*.ts"],
  formatter: {
    printWidth: 100,
    sortAttributes: false,
  },
  linter: {
    preset: "essential",
    rules: {
      "vue/no-unsafe-url": "off",
    },
  },
  name: "quadratic-web",
  typeChecker: {
    enabled: true,
    tsconfig: "tsconfig.json",
  },
  vite: {
    scanPatterns: ["src/**/*.vue"],
  },
});
