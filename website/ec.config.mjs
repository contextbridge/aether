// @ts-check
import { defineEcConfig } from "astro-expressive-code";

export default defineEcConfig({
  themes: [
    "ayu-dark",
    "everforest-dark",
    "github-dark",
    "dracula",
    "nord",
    "catppuccin-mocha",
    "tokyo-night",
    "rose-pine",
  ],
  useDarkModeMediaQuery: false,
  themeCssSelector: (theme) => {
    /** @type {Record<string, string | false>} */
    const map = {
      "ayu-dark": "ayu-dark",
      "everforest-dark": "everforest-dark",
      "github-dark": false,
      dracula: "dracula",
      nord: "nord",
      "catppuccin-mocha": "catppuccin",
      "tokyo-night": "tokyo-night",
      "rose-pine": "rose-pine",
    };
    const val = map[theme.name];
    if (!val) return false;
    return `[data-color-theme='${val}']`;
  },
  styleOverrides: {
    borderRadius: "0.5rem",
    borderWidth: "1px",
    codeFontFamily: "'IBM Plex Mono', monospace",
    uiFontFamily: "'IBM Plex Mono', monospace",
    frames: {
      frameBoxShadowCssValue: "0 1px 3px rgba(0, 0, 0, 0.3)",
    },
  },
});
