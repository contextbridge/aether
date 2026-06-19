`Form` supports two interaction modes based on field count:

- **Single-step mode** (`0` or `1` fields)
- **Multi-step mode** (`2+` fields)

In both modes, each [`FormField`] renders through its backing widget and all values can be serialized with [`to_json()`](Form::to_json).

# Modes

## Single-step mode (`0` or `1` fields)

The form renders only the active prompt content and footer hints.

- No tab bar
- No review/submit pane
- `Enter` submits immediately

This mode is ideal for yes/no confirmations and other one-question prompts.

## Multi-step mode (`2+` fields)

A tab bar is shown with all fields plus a virtual **Submit** tab.

- Navigate with `Tab` / `BackTab` or arrows (for non-text fields)
- `Enter` advances to the next tab while editing fields
- `Enter` on the **Submit** tab emits submit

The Submit tab shows a review summary of all field values.

# Messages

`Form` implements [`Component`](crate::Component) with `Message = FormMessage`:

- **`FormMessage::Close`** — Emitted on `Esc`.
- **`FormMessage::Submit`** — Emitted on `Enter`:
  - immediately in single-step mode
  - only from the Submit tab in multi-step mode

# `FormField`

A single field within the form:

- **`name`** — Machine-readable key (used in JSON output).
- **`label`** — Human-readable label shown in UI.
- **`description`** — Optional help text shown below the field.
- **`required`** — If `true`, an asterisk is shown next to the label.
- **`kind`** — The backing widget, as a [`FormFieldKind`].

# `FormFieldKind`

- **`Text(TextField)`** — Single-line text input.
- **`Number(NumberField)`** — Numeric input.
- **`Boolean(Checkbox)`** — Toggle rendered as `[x]` / `[ ]`.
- **`SingleSelect(RadioSelect)`** — Radio button list.
- **`MultiSelect(MultiSelect)`** — Checkbox list.

# See also

- [`TextField`](crate::TextField)
- [`Checkbox`](crate::Checkbox)
- [`RadioSelect`](crate::RadioSelect)
- [`MultiSelect`](crate::MultiSelect)
- [`FocusRing`](crate::FocusRing)
