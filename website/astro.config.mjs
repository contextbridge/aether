// @ts-check
import { defineConfig } from "astro/config";
import starlight from "@astrojs/starlight";
import icon from "astro-icon";
import tailwindcss from "@tailwindcss/vite";
import { GITHUB_URL } from "./src/consts.ts";

export default defineConfig({
  integrations: [
    icon(),
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
            { label: "Introduction", slug: "getting-started/overview" },
          ],
        },
        {
          label: "Aether",
          items: [
            {
              label: "Configuration",
              items: [
                { label: "Agents", slug: "aether/configuration/agent-settings" },
                { label: "LLMs", slug: "aether/configuration/llm-providers" },
                { label: "Prompts", slug: "aether/configuration/system-prompts" },
                { label: "Tools", slug: "aether/configuration/mcp-servers" },
              ],
            },
            {
              label: "Built-in MCP Servers",
              items: [
                { label: "Coding", slug: "aether/built-in-servers/coding" },
                { label: "LSP", slug: "aether/built-in-servers/lsp" },
                { label: "Skills & Commands", slug: "aether/built-in-servers/skills-commands" },
                { label: "Tasks", slug: "aether/built-in-servers/tasks" },
                { label: "Sub-Agents", slug: "aether/built-in-servers/subagents" },
                { label: "Survey", slug: "aether/built-in-servers/survey" },
                { label: "Plan", slug: "aether/built-in-servers/plan" },
              ],
            },
            {
              label: "Terminal UI",
              items: [
                { label: "Overview", slug: "aether/terminal/overview" },
                { label: "Keybindings & Commands", slug: "aether/terminal/keybindings" },
                { label: "Git Diff View", slug: "aether/terminal/git-diff" },
                { label: "Settings & Themes", slug: "aether/terminal/settings" },
                { label: "Sessions", slug: "aether/terminal/sessions" },
              ],
            },
            { label: "IDE (ACP)", slug: "aether/running/editor-integration" },
            { label: "Headless", slug: "aether/running/headless" },
          ],
        },
        {
          label: "Wisp Standalone",
          collapsed: true,
          items: [
            { label: "Using with Other Agents", slug: "wisp-standalone/using-with-other-agents" },
            { label: "Embedding as a Library", slug: "wisp-standalone/embedding" },
          ],
        },
        {
          label: "Libraries",
          items: [
            { label: "Architecture", slug: "libraries/architecture" },
            {
              label: "aether-core",
              collapsed: true,
              items: [
                { label: "Agent Builder", slug: "libraries/aether-core/agent-builder" },
                { label: "Events & Streaming", slug: "libraries/aether-core/events" },
                { label: "MCP Integration", slug: "libraries/aether-core/mcp-integration" },
              ],
            },
            {
              label: "llm",
              collapsed: true,
              items: [
                { label: "Provider Interface", slug: "libraries/llm/provider-interface" },
                { label: "Custom Providers", slug: "libraries/llm/custom-providers" },
              ],
            },
            {
              label: "mcp-servers",
              collapsed: true,
              items: [
                { label: "Embedding Servers", slug: "libraries/mcp-servers/embedding" },
              ],
            },
            {
              label: "tui",
              collapsed: true,
              items: [
                { label: "Components", slug: "libraries/tui/components" },
                { label: "Rendering", slug: "libraries/tui/rendering" },
              ],
            },
            {
              label: "crucible",
              collapsed: true,
              items: [
                { label: "Writing Evals", slug: "libraries/crucible/evals" },
              ],
            },
          ],
        },
      ],
    }),
  ],
  vite: { plugins: [tailwindcss()] },
});
