import { defineCollection } from "astro:content";
import { docsLoader } from "@astrojs/starlight/loaders";
import { docsSchema } from "@astrojs/starlight/schema";
import { settingsSchemaLoader } from "./loaders/settings-schema";

const docs = docsLoader();
const settingsSchema = settingsSchemaLoader();

export const collections = {
  docs: defineCollection({
    loader: {
      name: "docs-with-settings-schema",
      async load(context) {
        await docs.load(context);
        await settingsSchema.load(context);
      },
    },
    schema: docsSchema(),
  }),
};
