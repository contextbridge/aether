UI components that make up the Wisp terminal interface.

All components implement the [`Component`](tui::Component) trait from the [`tui`] crate, following its event-message pattern: [`on_event`](tui::Component::on_event) handles input and produces typed messages, [`render`](tui::Component::render) produces a [`Frame`](tui::Frame).

# Component tree

```text
App
 ├─ ScreenRouter
 │   ├─ ConversationScreen
 │   │   ├─ ConversationWindow (message history)
 │   │   ├─ PromptComposer
 │   │   │   ├─ TextInput
 │   │   │   ├─ CommandPicker (modal)
 │   │   │   └─ FilePicker (modal)
 │   │   ├─ ToolCallStatuses
 │   │   ├─ PlanTracker / PlanView
 │   │   ├─ ProgressIndicator
 │   │   ├─ ElicitationForm (modal)
 │   │   └─ SessionPicker (modal)
 │   └─ GitDiffView
 │       ├─ PatchRenderer
 │       └─ SplitPatchRenderer
 ├─ SettingsOverlay (modal)
 └─ StatusLine
```

# Visibility

Components marked `pub` are part of the public API. Components marked `pub(crate)` are internal implementation details used only within wisp.
