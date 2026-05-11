# CodingMcp

File operations, code search, bash execution, LSP integration, and web tools. This is the workhorse server for coding tasks.

**Flags:** `--root-dir <path>` (optional workspace root), `--rules-dir <path>` (repeatable read-rule directories), and `--disable-lsp` (disable LSP-backed tools and daemon connections)

## Table of Contents

<!-- START doctoc generated TOC please keep comment here to allow auto update -->
<!-- DON'T EDIT THIS SECTION, INSTEAD RE-RUN doctoc TO UPDATE -->

- [Tools](#tools)
  - [File Operations](#file-operations)
  - [Search](#search)
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
| `list_files` | List files and directories at a path with metadata (size, type, permissions, modified time). |

### Search

| Tool | Description |
|------|-------------|
| `grep` | Search file contents with regex. Supports glob filters, file type filters, context lines, case-insensitive mode, and multiline matching. Output modes: `Content`, `FilesWithMatches`, `Count`. |
| `find` | Find files by glob pattern. Recursive directory walk. |

### Bash

| Tool | Description |
|------|-------------|
| `bash` | Execute a shell command with optional timeout (max 10 minutes, default 2 minutes). Supports background execution. |
| `read_background_bash` | Read output from a background bash process by its shell ID. Supports regex filtering. |

### Web

| Tool | Description |
|------|-------------|
| `web_fetch` | Fetch a URL and convert HTML to markdown. |
| `web_search` | Search the web via Brave Search API. Requires `BRAVE_SEARCH_API_KEY` env var. Supports domain allow/block lists. |

### LSP (Language Server Protocol)

These tools provide code-aware navigation. They require a running language server for the target language.

| Tool | Description |
|------|-------------|
| `lsp_symbol` | Go-to-definition, find references, find implementations, hover info, or incoming/outgoing call hierarchy for a symbol. |
| `lsp_workspace_search` | Search for symbols across the workspace when you do not know the file path. |
| `lsp_document` | Get all symbols in a document (functions, structs, traits, etc.) with nested structure. |
| `lsp_check_errors` | Get compiler diagnostics (errors, warnings) for a file or the entire workspace. |
| `lsp_rename` | Rename a symbol across the project. |

## Read-Before-Edit Safety

`write_file` and `edit_file` require that the file has been read with `read_file` first. This prevents blind overwrites and ensures the agent has seen the current contents before making changes.
