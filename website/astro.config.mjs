// @ts-check
import { defineConfig } from "astro/config";
import starlight from "@astrojs/starlight";
import mermaid from "astro-mermaid";
import icon from "astro-icon";
import tailwindcss from "@tailwindcss/vite";
import { GITHUB_URL } from "./src/consts.ts";

export default defineConfig({
  site: "https://aether-agent.io",
  integrations: [
    icon(),
    mermaid({
      theme: "base",
      autoTheme: false,
      enableLog: false,
      mermaidConfig: {
        themeVariables: {
          background: "#0d1117",
          primaryColor: "#22272e",
          primaryTextColor: "#f0f1f4",
          primaryBorderColor: "#444c5c",
          lineColor: "#8891a0",
          secondaryColor: "#161b22",
          tertiaryColor: "#1c2128",
          clusterBkg: "#161b22",
          clusterBorder: "#444c5c",
          edgeLabelBackground: "#0d1117",
          fontFamily: "IBM Plex Sans, sans-serif",
        },
        flowchart: {
          curve: "basis",
        },
      },
    }),
    starlight({
      title: "Aether",
      customCss: [
        "./src/styles/global.css",
        "./src/styles/starlight.css",
        "./src/styles/themes.css",
      ],
      components: {
        Head: "./src/components/StarlightHead.astro",
        ThemeSelect: "./src/components/ThemeSwitcher.astro",
      },
      social: [
        {
          icon: "github",
          label: "GitHub",
          href: GITHUB_URL,
        },
      ],
      /* EC config in ec.config.mjs (themeCssSelector isn't JSON-serializable) */
      sidebar: [
        {
          label: "Getting Started",
          items: [
            { label: "Quickstart", slug: "getting-started/overview" },
            { label: "Introduction", slug: "aether/introduction" },
          ],
        },
        {
          label: "Settings",
          items: [
            { label: "Overview", slug: "aether/settings/overview" },
            {
              label: "User and Project Settings",
              slug: "aether/settings/user-project-settings",
            },
            { label: "LLMs", slug: "aether/settings/llm-providers" },
            {
              label: "Prompts",
              slug: "aether/settings/system-prompts",
            },
            { label: "Tools", slug: "aether/settings/mcp-servers" },
            {
              label: "Field reference",
              slug: "aether/settings/reference",
            },
          ],
        },
        {
          label: "Built-in MCP Servers",
          items: [
            { label: "Coding", slug: "aether/built-in-servers/coding" },
            {
              label: "Skills, Rules & Notes",
              slug: "aether/built-in-servers/skills-commands",
            },
            { label: "Tasks", slug: "aether/built-in-servers/tasks" },
            {
              label: "Sub-Agents",
              slug: "aether/built-in-servers/subagents",
            },
            { label: "Survey", slug: "aether/built-in-servers/survey" },
            { label: "Plan", slug: "aether/built-in-servers/plan" },
          ],
        },
        {
          label: "Terminal UI",
          items: [
            { label: "Overview", slug: "aether/terminal/overview" },
            {
              label: "Keybindings & Commands",
              slug: "aether/terminal/keybindings",
            },
            { label: "Git Diff View", slug: "aether/terminal/git-diff" },
            {
              label: "Settings & Themes",
              slug: "aether/terminal/settings",
            },
            { label: "Sessions", slug: "aether/terminal/sessions" },
          ],
        },
        { label: "IDE (ACP)", slug: "aether/running/editor-integration" },
        { label: "Headless", slug: "aether/running/headless" },
        {
          label: "Libraries",
          items: [
            { label: "Rust", slug: "libraries/rust" },
            { label: "TypeScript SDK", slug: "libraries/typescript-sdk" },
          ],
        },
      ],
    }),
  ],
  vite: { plugins: [tailwindcss()] },
});
