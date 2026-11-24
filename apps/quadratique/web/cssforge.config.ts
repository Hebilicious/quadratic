import { defineConfig } from "@hebilicious/cssforge";

const theme = (palette: Record<string, string>, settings?: { atRule: string }) => ({
  value: {
    ui: {
      value: {
        surface: "var(--surface)",
        raised: "var(--raised)",
        sunken: "var(--sunken)",
        line: "var(--line)",
        ink: "var(--ink)",
        muted: "var(--muted)",
        faint: "var(--faint)",
        accent: "var(--accent)",
        backlog: "var(--backlog)",
        unstarted: "var(--unstarted)",
        started: "var(--started)",
        completed: "var(--completed)",
        canceled: "var(--canceled)",
        urgent: "var(--urgent)",
      },
      variables: palette,
      settings: { variantNameOnly: true },
    },
  },
  ...(settings === undefined ? {} : { settings }),
});

export default defineConfig({
  colors: {
    palette: {
      value: {
        stone: {
          value: {
            0: { hex: "#ffffff" },
            50: { hex: "#f7f7f8" },
            100: { hex: "#eeeef0" },
            200: { hex: "#dddde1" },
            400: { hex: "#9a9aa3" },
            500: { hex: "#6e6e78" },
            700: { hex: "#3a3a42" },
            800: { hex: "#26262c" },
            850: { hex: "#1d1d22" },
            900: { hex: "#16161a" },
            950: { hex: "#0f0f12" },
            1000: { hex: "#e8e8ec" },
          },
        },
        indigo: { value: { 400: { hex: "#8b8cf6" }, 600: { hex: "#5b5bd6" } } },
        amber: { value: { 400: { hex: "#f2b84b" }, 600: { hex: "#c98a10" } } },
        green: { value: { 400: { hex: "#4cc38a" }, 600: { hex: "#2f9e66" } } },
        red: { value: { 400: { hex: "#f2785c" }, 600: { hex: "#d9482b" } } },
      },
    },
    theme: {
      light: theme({
        surface: "palette.stone.0",
        raised: "palette.stone.50",
        sunken: "palette.stone.100",
        line: "palette.stone.200",
        ink: "palette.stone.900",
        muted: "palette.stone.500",
        faint: "palette.stone.400",
        accent: "palette.indigo.600",
        backlog: "palette.stone.400",
        unstarted: "palette.stone.500",
        started: "palette.amber.600",
        completed: "palette.green.600",
        canceled: "palette.stone.400",
        urgent: "palette.red.600",
      }),
      dark: theme(
        {
          surface: "palette.stone.950",
          raised: "palette.stone.900",
          sunken: "palette.stone.850",
          line: "palette.stone.800",
          ink: "palette.stone.1000",
          muted: "palette.stone.400",
          faint: "palette.stone.500",
          accent: "palette.indigo.400",
          backlog: "palette.stone.500",
          unstarted: "palette.stone.400",
          started: "palette.amber.400",
          completed: "palette.green.400",
          canceled: "palette.stone.500",
          urgent: "palette.red.400",
        },
        { atRule: "@media (prefers-color-scheme: dark)" },
      ),
    },
  },
  primitives: {
    font: {
      value: {
        family: {
          value: {
            body: 'ui-sans-serif, system-ui, -apple-system, "Segoe UI", sans-serif',
            mono: 'ui-monospace, SFMono-Regular, "JetBrains Mono", Menlo, Consolas, monospace',
          },
        },
        size: { value: { small: "12px", body: "13px", title: "15px" } },
      },
    },
    corner: { value: { radius: { value: { small: "4px", large: "8px" } } } },
  },
  spacing: {
    custom: {
      size: {
        value: {
          1: "0.25rem",
          2: "0.5rem",
          3: "0.75rem",
          4: "1rem",
          6: "1.5rem",
          8: "2rem",
        },
      },
    },
  },
});
