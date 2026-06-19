Renders the scrollable conversation history.

`ConversationWindow` takes a [`ConversationBuffer`] and renders its segments as styled terminal lines. The buffer accumulates streamed content — user messages, assistant text chunks, thoughts, and tool call labels — and the window handles scroll position and viewport clipping.

# Segment types

- **`UserMessage`** — the user's submitted prompt, rendered with a distinctive style.
- **`Text`** — assistant markdown, rendered with syntax highlighting via [`render_markdown_result`](tui::render_markdown_result).
- **`Thought`** — reasoning/thinking content, displayed in a collapsible block.
- **`ToolCall`** — a label identifying a tool invocation.

# See also

- `ConversationScreen` — the parent that owns this window
