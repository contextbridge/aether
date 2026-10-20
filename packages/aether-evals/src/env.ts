import { AetherEvalError } from "./errors.js";

export const CONTAINER_AETHER_HOME = "/root/.aether";

export function evalEnvVars(
  model: string,
  hostEnv: Record<string, string | undefined> = process.env,
): Record<string, string> {
  const env: Record<string, string> = {};
  for (const key of new Set(model.split(",").flatMap(providerEnvVars))) {
    const value = hostEnv[key];
    if (value !== undefined) env[key] = value;
  }
  env.AETHER_HOME = CONTAINER_AETHER_HOME;
  return env;
}

const AWS_ENV_VARS = [
  "AWS_ACCESS_KEY_ID",
  "AWS_BEARER_TOKEN_BEDROCK",
  "AWS_DEFAULT_REGION",
  "AWS_PROFILE",
  "AWS_REGION",
  "AWS_SECRET_ACCESS_KEY",
  "AWS_SESSION_TOKEN",
];

const PROVIDER_ENV_VARS: Record<string, string[]> = {
  anthropic: ["ANTHROPIC_API_KEY"],
  "azure-foundry": ["AZURE_OPENAI_API_KEY"],
  bedrock: AWS_ENV_VARS,
  codex: [],
  deepseek: ["DEEPSEEK_API_KEY"],
  fireworks: ["FIREWORKS_API_KEY"],
  gemini: ["GEMINI_API_KEY"],
  llamacpp: ["LLAMA_CPP_HOST"],
  moonshot: ["MOONSHOT_API_KEY"],
  ollama: ["OLLAMA_HOST"],
  openai: ["OPENAI_API_KEY"],
  openrouter: ["OPENROUTER_API_KEY"],
  xiaomi: ["XIAOMI_API_KEY"],
  zai: ["ZAI_API_KEY"],
};

function providerEnvVars(model: string): string[] {
  const provider = model.trim().split(":")[0] ?? "";
  const envVars = PROVIDER_ENV_VARS[provider];
  if (!envVars) {
    throw new AetherEvalError(
      "configuration_error",
      `unknown provider in model spec: ${model}`,
    );
  }
  return envVars;
}
