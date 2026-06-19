# SurveyMcp

Human-in-the-loop elicitation. Present structured forms to the user and collect their responses.

## Table of Contents

<!-- START doctoc generated TOC please keep comment here to allow auto update -->
<!-- DON'T EDIT THIS SECTION, INSTEAD RE-RUN doctoc TO UPDATE -->

- [Tools](#tools)
- [ask_user](#ask_user)
  - [Input](#input)
  - [Output](#output)
  - [Example](#example)

<!-- END doctoc generated TOC please keep comment here to allow auto update -->

## Tools

| Tool | Description |
|------|-------------|
| `ask_user` | Present a JSON Schema form to the user and collect their structured response. |

## ask_user

### Input

| Field | Type | Description |
|-------|------|-------------|
| `message` | string | The question or prompt to show the user. |
| `schema` | JSON Schema | Object schema with `properties` defining the form fields. |

### Output

| Field | Type | Description |
|-------|------|-------------|
| `accepted` | bool | Whether the user accepted (`true`) or cancelled (`false`). |
| `data` | object or null | The structured response data, if accepted. |

### Example

```json
{
  "message": "What database should we use?",
  "schema": {
    "type": "object",
    "properties": {
      "database": {
        "type": "string",
        "title": "Database",
        "enum": ["postgres", "sqlite", "mysql"]
      },
      "reason": {
        "type": "string",
        "title": "Reason"
      }
    }
  }
}
```
