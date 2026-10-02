# CodingMcp

File operations, bash execution, LSP code intelligence, and web tools. This is the workhorse server for coding tasks.

**Flags:** `--root-dir <path>` (optional workspace root), `--rules-dir <path>` (repeatable read-rule directories), and `--disable-lsp` (disable LSP-backed tools and daemon connections)

## Table of Contents

<!-- START doctoc generated TOC please keep comment here to allow auto update -->
<!-- DON'T EDIT THIS SECTION, INSTEAD RE-RUN doctoc TO UPDATE -->

- [Tools](#tools)
  - [File Operations](#file-operations)
  - [Bash](#bash)
  - [Web](#web)
  - [LSP (Language Server Protocol)](#lsp-language-server-protocol)
- [Read-Before-Edit Safety](#read-before-edit-safety)

<!-- END doctoc generated TOC please keep comment here to allow auto update -->

## Tools

### File Operations

| Tool | Description |
|------|-------------|
| `read_file` | Read a file with optional line offset and limit. Returns content with line numbers. Max 2000 lines by default. |
| `write_file` | Write content to a file. Creates parent directories automatically. File must be read first (safety check). |
| `edit_file` | Find-and-replace in a file. Matches exact strings, supports `replace_all`. File must be read first. |

### Bash

Use shell commands through `bash` for text search, file discovery, structural search, and directory listings.

| Tool | Description |
|------|-------------|
| `bash` | Execute a shell command with optional timeout. Set `runInBackground: true` to return an MCP Task; the client must support the Tasks extension and receives complete output when the task finishes. |

### Web

| Tool | Description |
|------|-------------|
| `web_fetch` | Fetch a URL and convert HTML to markdown. |
| `web_search` | Search the web via Brave Search API. Requires `BRAVE_SEARCH_API_KEY` env var. Supports domain allow/block lists. |

### LSP (Language Server Protocol)

These tools provide code-aware navigation. They require a running language server for the target language.

| Tool | Description |
|------|-------------|
| `lsp_symbol` | Go-to-definition (including into dependencies), find references, or hover info for a symbol. |
| `lsp_check_errors` | Get compiler diagnostics (errors, warnings) for a file or the entire workspace. |
| `lsp_rename` | Rename a symbol across the project. |

## Read-Before-Edit Safety

`write_file` and `edit_file` require that the file has been read with `read_file` first. This prevents blind overwrites and ensures the agent has seen the current contents before making changes.
