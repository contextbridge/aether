import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import type { Loader, LoaderContext } from "astro/loaders";

/**
 * Renders the `AetherSettings` JSON Schema into a
 * Starlight reference page. The Rust struct (annotated with `schemars::JsonSchema`
 * and `///` doc comments) is the source of truth.
 */
export function settingsSchemaLoader(): Loader {
  const here = dirname(fileURLToPath(import.meta.url));
  const root = resolve(here, "../../../..");
  const snapshotPath = resolve(here, "../data/aether-settings.schema.json");

  return {
    name: "aether-settings-schema",
    async load({
      store,
      parseData,
      renderMarkdown,
      generateDigest,
      watcher,
      logger,
    }: LoaderContext) {
      const schema = generateSchema(root, snapshotPath);
      const body = renderReference(schema);
      const id = "aether/settings/reference";
      const digest = generateDigest(body);

      const existing = store.get(id);
      if (existing?.digest === digest) return;

      const data = await parseData({
        id,
        data: {
          title: "Settings reference",
          description: "Reference for every field in the Aether settings file.",
        },
      });
      const rendered = await renderMarkdown(body);
      store.set({ id, data, body, rendered, digest });
      logger.info("Generated settings reference from AetherSettings schema");

      watcher?.add(snapshotPath);
    },
  };
}

type JsonSchema = Record<string, any>;

function generateSchema(root: string, snapshotPath: string): JsonSchema {
  if (existsSync(snapshotPath)) {
    return JSON.parse(readFileSync(snapshotPath, "utf8")) as JsonSchema;
  }

  const schemaText = execFileSync(
    "cargo",
    ["run", "-q", "-p", "aether-project", "--bin", "aether-settings-schema"],
    { cwd: root, encoding: "utf8" },
  );

  const schema = JSON.parse(schemaText) as JsonSchema;
  mkdirSync(dirname(snapshotPath), { recursive: true });
  writeFileSync(snapshotPath, `${JSON.stringify(schema, null, 2)}\n`);
  return schema;
}

function renderReference(schema: JsonSchema): string {
  const defs: Record<string, JsonSchema> = schema.$defs ?? {};
  const parts: string[] = [];

  parts.push(renderDef(schema.title ?? "AetherSettings", schema));
  for (const name of Object.keys(defs).sort()) {
    parts.push(renderDef(name, defs[name]));
  }

  return parts.join("\n");
}

function renderDef(name: string, def: JsonSchema): string {
  const lines: string[] = [`## ${name}`, ""];
  if (def.description) {
    lines.push(def.description.trim(), "");
  }

  if (Array.isArray(def.enum)) {
    lines.push("Enum. One of:", "");
    for (const value of def.enum) {
      lines.push(`- \`${JSON.stringify(value)}\``);
    }
    lines.push("");
    return lines.join("\n");
  }

  const variants: JsonSchema[] | undefined =
    def.oneOf ?? (def.properties ? undefined : def.anyOf);
  if (variants) {
    lines.push("One of:", "");
    const inlineObjects: Array<{ label: string; schema: JsonSchema }> = [];
    for (const variant of variants) {
      if (variant.type === "object" && variant.properties) {
        const label = variantLabel(name, variant, inlineObjects.length);
        inlineObjects.push({ label, schema: variant });
        lines.push(`- [${label}](#${anchor(label)})`);
      } else {
        lines.push(`- ${typeLabel(variant)}`);
      }
    }
    lines.push("");
    for (const { label, schema } of inlineObjects) {
      lines.push(`### ${label}`, "", renderObjectTable(schema), "");
    }
    return lines.join("\n");
  }

  if (def.properties) {
    lines.push(renderObjectTable(def), "");
    const constraint = anyOfConstraint(def);
    if (constraint) lines.push(constraint, "");
    return lines.join("\n");
  }

  lines.push(`Type: ${typeLabel(def)}`, "");
  return lines.join("\n");
}

function renderObjectTable(def: JsonSchema): string {
  const required: string[] = def.required ?? [];
  const props: Record<string, JsonSchema> = def.properties ?? {};
  const rows = [
    "| Field | Type | Required | Default | Description |",
    "| ----- | ---- | -------- | ------- | ----------- |",
  ];

  for (const field of Object.keys(props)) {
    const prop = props[field];
    const isRequired = required.includes(field) ? "yes" : "no";
    const def_ =
      "default" in prop ? `\`${JSON.stringify(prop.default)}\`` : "—";
    const desc = prop.description ? escapeProse(prop.description) : "—";
    rows.push(
      `| \`${field}\` | ${escapeCell(typeLabel(prop))} | ${isRequired} | ${escapeCell(def_)} | ${desc} |`,
    );
  }

  return rows.join("\n");
}

function typeLabel(schema: JsonSchema | undefined): string {
  if (!schema) return "any";
  if (schema.$ref) {
    const name = refName(schema.$ref);
    return `[${name}](#${anchor(name)})`;
  }
  if (schema.const !== undefined) return `\`${JSON.stringify(schema.const)}\``;
  if (Array.isArray(schema.enum))
    return schema.enum
      .map((e: unknown) => `\`${JSON.stringify(e)}\``)
      .join(" | ");

  const union = schema.oneOf ?? schema.anyOf;
  if (union) return (union as JsonSchema[]).map(typeLabel).join(" | ");

  const type = schema.type;
  if (Array.isArray(type))
    return type.map((t: string) => primitive(t)).join(" | ");
  if (type === "array") return `${typeLabel(schema.items)}[]`;
  if (type === "object") {
    const extra = schema.additionalProperties;
    if (extra && typeof extra === "object")
      return `Record<string, ${typeLabel(extra)}>`;
    return "object";
  }
  return primitive(type);
}

function primitive(type: string | undefined): string {
  return type ?? "any";
}

function anyOfConstraint(def: JsonSchema): string | null {
  if (!Array.isArray(def.anyOf)) return null;
  const fields = def.anyOf
    .flatMap((branch: JsonSchema) => branch.required ?? [])
    .map((field: string) => `\`${field}\``);
  if (fields.length === 0) return null;
  return `**Constraint:** at least one of ${[...new Set(fields)].join(", ")} must be set.`;
}

function variantLabel(
  parent: string,
  variant: JsonSchema,
  index: number,
): string {
  const discriminant = variant.properties?.type?.const;
  if (typeof discriminant === "string")
    return `${parent} (type: ${discriminant})`;
  return `${parent} variant ${index + 1}`;
}

function refName(ref: string): string {
  return ref.split("/").pop() ?? ref;
}

function anchor(text: string): string {
  return text
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
}

function escapeProse(text: string): string {
  return text
    .replace(/\s*\n\s*/g, " ")
    .replace(/\|/g, "\\|")
    .trim();
}

function escapeCell(text: string): string {
  return text.replace(/\|/g, "\\|");
}
