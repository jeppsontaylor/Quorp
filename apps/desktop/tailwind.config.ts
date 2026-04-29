import type { Config } from "tailwindcss";

const config: Config = {
  darkMode: ["class", "[data-theme='quorp-dark']"],
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  theme: {
    extend: {
      colors: {
        // Mapped to CSS custom properties in src/styles/tokens.css so
        // theme switching (Dark / High-contrast / No-color) flips
        // every Tailwind utility through the same token.
        bg: {
          base: "var(--quorp-bg-base)",
          surface: "var(--quorp-bg-surface)",
          elevated: "var(--quorp-bg-surface-elevated)",
          overlay: "var(--quorp-bg-overlay)",
        },
        fg: {
          primary: "var(--quorp-fg-primary)",
          secondary: "var(--quorp-fg-secondary)",
          muted: "var(--quorp-fg-muted)",
          disabled: "var(--quorp-fg-disabled)",
        },
        border: {
          subtle: "var(--quorp-border-subtle)",
          strong: "var(--quorp-border-strong)",
        },
        diff: {
          "add-fg": "var(--quorp-diff-add-fg)",
          "add-bg": "var(--quorp-diff-add-bg)",
          "add-bg-strong": "var(--quorp-diff-add-bg-strong)",
          "del-fg": "var(--quorp-diff-del-fg)",
          "del-bg": "var(--quorp-diff-del-bg)",
          "del-bg-strong": "var(--quorp-diff-del-bg-strong)",
          "context-bg": "var(--quorp-diff-context-bg)",
          gutter: "var(--quorp-diff-gutter-bg)",
        },
        status: {
          success: "var(--quorp-status-success)",
          info: "var(--quorp-status-info)",
          warning: "var(--quorp-status-warning)",
          danger: "var(--quorp-status-danger)",
          attention: "var(--quorp-status-attention)",
        },
        risk: {
          low: "var(--quorp-risk-low)",
          medium: "var(--quorp-risk-medium)",
          high: "var(--quorp-risk-high)",
          critical: "var(--quorp-risk-critical)",
        },
        ring: {
          focus: "var(--quorp-focus-ring)",
          "focus-strong": "var(--quorp-focus-ring-strong)",
        },
      },
      fontFamily: {
        mono: "var(--quorp-font-mono)",
        ui: "var(--quorp-font-ui)",
      },
      borderRadius: {
        sm: "var(--quorp-radius-sm)",
        md: "var(--quorp-radius-md)",
        lg: "var(--quorp-radius-lg)",
      },
      spacing: {
        "rail": "56px",
        "panel-min": "280px",
        "panel-max": "340px",
        "inspector-min": "360px",
        "inspector-max": "420px",
        "mission-bar": "44px",
        "composer": "140px",
      },
    },
  },
  plugins: [],
};

export default config;
