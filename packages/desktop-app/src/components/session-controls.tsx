import type {
  SessionConfigOption,
  SessionConfigSelectGroup,
  SessionConfigSelectOption,
} from "@agentclientprotocol/sdk";
import { BotIcon, CpuIcon } from "lucide-react";
import { useMemo, type ReactNode } from "react";
import { useShallow } from "zustand/shallow";
import {
  ModelSelector,
  type ModelOption,
} from "@/components/assistant-ui/model-selector";
import { useChatStore } from "@/acp-store";

export function SessionControls() {
  const { configOptions, setConfigOption } = useChatStore(
    useShallow((state) => ({
      configOptions: state.configOptions,
      setConfigOption: state.setConfigOption,
    })),
  );

  const mode = findSelect(configOptions, "mode", "mode");
  const model = findSelect(configOptions, "model", "model");
  const reasoning = findSelect(
    configOptions,
    "thought_level",
    "reasoning_effort",
  );

  const reasoningOptions = useMemo(
    () => reasoning && flattenValues(reasoning.options),
    [reasoning],
  );

  return (
    <div className="flex min-w-0 items-center gap-1">
      {mode && (
        <ConfigSelector
          config={mode}
          icon={<BotIcon />}
          searchable={false}
          onChange={(value) => void setConfigOption(mode.id, value)}
        />
      )}
      {model && (
        <ConfigSelector
          config={model}
          icon={<CpuIcon />}
          searchable
          effort={reasoning?.currentValue}
          effortOptions={reasoningOptions}
          onChange={(value) => void setConfigOption(model.id, value)}
          onEffortChange={
            reasoning
              ? (value) => void setConfigOption(reasoning.id, value)
              : undefined
          }
        />
      )}
      {!model && reasoning && (
        <ConfigSelector
          config={reasoning}
          searchable={false}
          onChange={(value) => void setConfigOption(reasoning.id, value)}
        />
      )}
    </div>
  );
}

type SelectConfig = Extract<SessionConfigOption, { type: "select" }>;

type ConfigValue = SessionConfigSelectOption & {
  group?: string;
};

function ConfigSelector({
  config,
  icon,
  searchable,
  effort,
  effortOptions,
  onChange,
  onEffortChange,
}: {
  config: SelectConfig;
  icon?: ReactNode;
  searchable: boolean;
  effort?: string;
  effortOptions?: ConfigValue[];
  onChange: (value: string) => void;
  onEffortChange?: (value: string) => void;
}) {
  const values = useMemo(() => flattenValues(config.options), [config.options]);
  const models = useMemo<ModelOption[]>(
    () =>
      values.map((option) => ({
        id: option.value,
        name: option.name,
        description: option.description ?? undefined,
        disabled:
          option.value.startsWith("__unavailable:") ||
          option.description?.startsWith("Unavailable:") === true,
        keywords: option.group ? [option.group] : undefined,
        icon,
        efforts: effortOptions?.map((level) => ({
          id: level.value,
          name: level.name,
        })),
      })),
    [effortOptions, icon, values],
  );

  return (
    <ModelSelector.Root
      models={models}
      value={config.currentValue}
      onValueChange={onChange}
      effort={effort}
      onEffortChange={onEffortChange}
    >
      <ModelSelector.Trigger
        variant="ghost"
        size="sm"
        aria-label={config.category === "mode" ? "Agent mode" : config.name}
        className="max-w-52 text-muted-foreground hover:text-foreground"
      />
      <ModelSelector.Content side="top" searchable={searchable}>
        {searchable && <ModelSelector.Search />}
        <ModelSelector.List>
          <ModelSelector.Empty />
          {groupModels(models, values).map(({ name, models: group }) => (
            <ModelSelector.Group key={name} heading={name || undefined}>
              {group.map((option) => (
                <ModelSelector.Item key={option.id} model={option} />
              ))}
            </ModelSelector.Group>
          ))}
        </ModelSelector.List>
        {onEffortChange && <ModelSelector.Effort label="Reasoning" />}
      </ModelSelector.Content>
    </ModelSelector.Root>
  );
}

function findSelect(
  options: SessionConfigOption[],
  category: string,
  id: string,
): SelectConfig | undefined {
  return options.find(
    (option): option is SelectConfig =>
      option.type === "select" &&
      (option.category === category || option.id === id),
  );
}

function flattenValues(options: SelectConfig["options"]): ConfigValue[] {
  return options.flatMap((option) =>
    isGroup(option)
      ? option.options.map((value) => ({ ...value, group: option.name }))
      : [option],
  );
}

function isGroup(
  option: SessionConfigSelectOption | SessionConfigSelectGroup,
): option is SessionConfigSelectGroup {
  return "options" in option;
}

function groupModels(models: ModelOption[], values: ConfigValue[]) {
  const groups = new Map<string, ModelOption[]>();
  models.forEach((model, index) => {
    const group = values[index]?.group ?? "";
    groups.set(group, [...(groups.get(group) ?? []), model]);
  });
  return [...groups].map(([name, group]) => ({ name, models: group }));
}
