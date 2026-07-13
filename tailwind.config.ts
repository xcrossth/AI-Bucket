import type { Config } from "tailwindcss";

export default {
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  theme: {
    extend: {
      colors: {
        panel: "var(--panel)",
        panelAlt: "var(--panel-alt)",
        border: "var(--border)",
        ink: "var(--ink)",
        muted: "var(--muted)"
      },
      boxShadow: {
        soft: "0 16px 48px rgba(0, 0, 0, 0.22)"
      }
    }
  },
  plugins: []
} satisfies Config;
