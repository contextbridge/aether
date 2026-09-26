import type {
  CreateElicitationRequest,
  CreateElicitationResponse,
  ElicitationContentValue,
  ElicitationPropertySchema,
  ElicitationSchema,
  EnumOption,
} from "@agentclientprotocol/sdk/experimental/v2";
import { type SubmitEvent, useState } from "react";
import { hasType } from "@/aether/acp";
import {
  answerElicitation,
  type PendingElicitation,
  useAether,
} from "@/aether/store";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";

/** Answers the agent's elicitations one at a time; dismissing the dialog cancels the request. */
export function ElicitationDialog() {
  const pending = useAether((state) => state.elicitations[0]);
  return (
    <Dialog
      open={pending !== undefined}
      onOpenChange={(open) => {
        if (!open && pending)
          answerElicitation(pending.id, { action: "cancel" });
      }}
    >
      {pending && <ElicitationForm key={pending.id} pending={pending} />}
    </Dialog>
  );
}

type Values = Record<string, ElicitationContentValue>;

function ElicitationForm({ pending }: { pending: PendingElicitation }) {
  const { request } = pending;
  const schema = formSchema(request);
  const [values, setValues] = useState<Values>(() => defaults(schema));
  const respond = (response: CreateElicitationResponse) =>
    answerElicitation(pending.id, response);
  const submit = (event: SubmitEvent) => {
    event.preventDefault();
    respond(
      schema ? { action: "accept", content: values } : { action: "accept" },
    );
  };

  return (
    <DialogContent>
      <form onSubmit={submit} className="flex flex-col gap-4">
        <DialogHeader>
          <DialogTitle>{schema?.title ?? "The agent needs input"}</DialogTitle>
          <DialogDescription className="whitespace-pre-wrap">
            {request.message}
          </DialogDescription>
        </DialogHeader>
        {isUrl(request) && (
          <a
            className="text-primary text-sm break-all underline"
            href={request.url}
            target="_blank"
            rel="noreferrer"
          >
            {request.url}
          </a>
        )}
        {Object.entries(schema?.properties ?? {}).map(([name, property]) => (
          <Field
            key={name}
            name={name}
            property={property}
            required={schema?.required?.includes(name) ?? false}
            value={values[name]}
            onChange={(value) =>
              setValues((current) => {
                if (value === undefined) {
                  const { [name]: _, ...rest } = current;
                  return rest;
                }
                return { ...current, [name]: value };
              })
            }
          />
        ))}
        <DialogFooter>
          <Button
            type="button"
            variant="ghost"
            onClick={() => respond({ action: "cancel" })}
          >
            Cancel
          </Button>
          <Button
            type="button"
            variant="outline"
            onClick={() => respond({ action: "decline" })}
          >
            Decline
          </Button>
          <Button type="submit">{isUrl(request) ? "Done" : "Submit"}</Button>
        </DialogFooter>
      </form>
    </DialogContent>
  );
}

interface FieldProps {
  name: string;
  property: ElicitationPropertySchema;
  required: boolean;
  value: ElicitationContentValue | undefined;
  onChange: (value: ElicitationContentValue | undefined) => void;
}

function Field({ name, property, required, value, onChange }: FieldProps) {
  const title = typeof property.title === "string" ? property.title : name;
  const description =
    typeof property.description === "string" ? property.description : null;
  return (
    <label className="flex flex-col gap-1.5 text-sm">
      <span className="font-medium">
        {title}
        {required && <span className="text-destructive"> *</span>}
      </span>
      <FieldInput
        property={property}
        required={required}
        value={value}
        onChange={onChange}
      />
      {description && (
        <span className="text-muted-foreground text-xs">{description}</span>
      )}
    </label>
  );
}

function FieldInput({
  property,
  required,
  value,
  onChange,
}: Omit<FieldProps, "name">) {
  if (hasType(property, "boolean")) {
    return (
      <input
        type="checkbox"
        className="size-4 self-start"
        checked={value === true}
        onChange={(event) => onChange(event.target.checked)}
      />
    );
  }
  if (hasType(property, "number") || hasType(property, "integer")) {
    return (
      <Input
        type="number"
        required={required}
        step={property.type === "integer" ? 1 : "any"}
        min={property.minimum ?? undefined}
        max={property.maximum ?? undefined}
        value={typeof value === "number" ? value : ""}
        onChange={(event) =>
          onChange(
            event.target.value === "" ? undefined : Number(event.target.value),
          )
        }
      />
    );
  }
  if (hasType(property, "array")) {
    const selected = Array.isArray(value) ? value : [];
    return (
      <div className="flex flex-col gap-1">
        {options(property.items).map((option) => (
          <label key={option.const} className="flex items-center gap-2">
            <input
              type="checkbox"
              className="size-4"
              checked={selected.includes(option.const)}
              onChange={(event) =>
                onChange(
                  event.target.checked
                    ? [...selected, option.const]
                    : selected.filter((item) => item !== option.const),
                )
              }
            />
            {option.title}
          </label>
        ))}
      </div>
    );
  }
  if (hasType(property, "string")) {
    const choices =
      property.oneOf ??
      property.enum?.map((item) => ({ const: item, title: item }));
    if (choices) {
      return (
        <select
          className="border-input h-9 rounded-md border bg-transparent px-2"
          required={required}
          value={typeof value === "string" ? value : ""}
          onChange={(event) => onChange(event.target.value || undefined)}
        >
          <option value="" />
          {choices.map((choice) => (
            <option key={choice.const} value={choice.const}>
              {choice.title}
            </option>
          ))}
        </select>
      );
    }
    return (
      <Input
        type={
          property.format === "email"
            ? "email"
            : property.format === "uri"
              ? "url"
              : "text"
        }
        required={required}
        minLength={property.minLength ?? undefined}
        maxLength={property.maxLength ?? undefined}
        pattern={property.pattern ?? undefined}
        value={typeof value === "string" ? value : ""}
        onChange={(event) => onChange(event.target.value || undefined)}
      />
    );
  }
  return (
    <p className="text-muted-foreground text-xs">
      Unsupported field type {property.type}
    </p>
  );
}

type FormRequest = Extract<CreateElicitationRequest, { mode: "form" }>;
type UrlRequest = Extract<CreateElicitationRequest, { mode: "url" }>;

function formSchema(
  request: CreateElicitationRequest,
): ElicitationSchema | null {
  return request.mode === "form"
    ? (request as FormRequest).requestedSchema
    : null;
}

function isUrl(request: CreateElicitationRequest): request is UrlRequest {
  return request.mode === "url";
}

function defaults(schema: ElicitationSchema | null): Values {
  const values: Values = {};
  for (const [name, property] of Object.entries(schema?.properties ?? {})) {
    const value = property.default;
    if (value !== undefined && value !== null)
      values[name] = value as ElicitationContentValue;
  }
  return values;
}

function options(
  items: Extract<ElicitationPropertySchema, { type: "array" }>["items"],
): EnumOption[] {
  if ("anyOf" in items && Array.isArray(items.anyOf))
    return items.anyOf as EnumOption[];
  if ("enum" in items && Array.isArray(items.enum)) {
    return (items.enum as string[]).map((item) => ({
      const: item,
      title: item,
    }));
  }
  return [];
}
