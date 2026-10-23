import json
import argparse
import os
import re
import sys
from urllib.parse import unquote, urlparse

docs = {}

parser = argparse.ArgumentParser()
parser.add_argument("--wedge-on", action="append", default=[])
parser.add_argument("--crash-on", action="append", default=[])
parser.add_argument("--fail-on", action="append", default=[])
parser.add_argument("--garble-before", action="append", default=[])
parser.add_argument("--pull-diagnostics", action="store_true")
cli_args = parser.parse_args()
wedge_methods = set(cli_args.wedge_on)
crash_methods = set(cli_args.crash_on)
fail_methods = set(cli_args.fail_on)
garble_methods = set(cli_args.garble_before)
pull_diagnostics = cli_args.pull_diagnostics
refresh_supported = False
status_supported = False
# Mimics a server still loading its workspace while the root holds a `.fake-loading` file.
loading = False
# Files containing `// hold-diagnostics` publish nothing until `fake/releaseDiagnostics`.
diagnostics_released = False
held_diagnostics = {}
root_path = None
next_request_id = 0
cancelled_since_open = set()
save_counts = {}


def write_message(msg):
    write_frame(json.dumps(msg, separators=(",", ":")).encode("utf-8"))


def write_frame(body):
    header = f"Content-Length: {len(body)}\r\n\r\n".encode("ascii")
    sys.stdout.buffer.write(header)
    sys.stdout.buffer.write(body)
    sys.stdout.buffer.flush()


def read_message():
    content_length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        line = line.decode("utf-8").strip()
        if not line:
            break
        if line.startswith("Content-Length: "):
            content_length = int(line[len("Content-Length: "):])

    if content_length is None:
        return None

    body = sys.stdin.buffer.read(content_length)
    if not body:
        return None
    return json.loads(body.decode("utf-8"))


def send_request(method, params):
    global next_request_id
    next_request_id += 1
    write_message(
        {
            "jsonrpc": "2.0",
            "id": f"fake-{next_request_id}",
            "method": method,
            "params": params,
        }
    )


def uri_to_path(uri):
    return unquote(urlparse(uri).path)


def read_disk(path):
    try:
        with open(path, encoding="utf-8") as file:
            return file.read()
    except OSError:
        return ""


def make_diagnostic(message):
    return {
        "range": {
            "start": {"line": 0, "character": 0},
            "end": {"line": 0, "character": 5},
        },
        "severity": 1,
        "message": message,
    }


def diagnostics_for(uri, text):
    """Report `error` tokens, plus errors in files named by `// depends: <path>` lines."""
    diagnostics = []
    if "error" in text.lower():
        diagnostics.append(make_diagnostic("error token"))

    directory = uri_to_path(uri).rsplit("/", 1)[0]
    for line in text.splitlines():
        if line.startswith("// depends: "):
            dependency = f"{directory}/{line[len('// depends: '):].strip()}"
            if "error" in read_disk(dependency).lower():
                diagnostics.append(make_diagnostic("dependency error"))
    return diagnostics


def publish(uri, text):
    if pull_diagnostics:
        return
    if "// hold-diagnostics" in text and not diagnostics_released:
        held_diagnostics[uri] = text
        return

    write_message(
        {
            "jsonrpc": "2.0",
            "method": "textDocument/publishDiagnostics",
            "params": {
                "uri": uri,
                "diagnostics": diagnostics_for(uri, text),
            },
        }
    )


def send_status():
    if status_supported:
        write_message(
            {
                "jsonrpc": "2.0",
                "method": "experimental/serverStatus",
                "params": {"health": "ok", "quiescent": not loading},
            }
        )


def respond(message, result):
    write_message({"jsonrpc": "2.0", "id": message["id"], "result": result})


def document(uri):
    return docs.get(uri, {"open_count": 0, "text": ""})


def make_range(start_line, start_character, end_line, end_character):
    return {
        "start": {"line": start_line, "character": start_character},
        "end": {"line": end_line, "character": end_character},
    }


def make_location(uri, start_line, start_character, end_line, end_character):
    return {
        "uri": uri,
        "range": make_range(
            start_line, start_character, end_line, end_character
        ),
    }


def word_at(text, line, character):
    rows = text.splitlines()
    if line >= len(rows):
        return ""
    row = rows[line]
    start = end = character
    while start > 0 and (row[start - 1].isalnum() or row[start - 1] == "_"):
        start -= 1
    while end < len(row) and (row[end].isalnum() or row[end] == "_"):
        end += 1
    return row[start:end]


def occurrences(uri, position):
    """Whole-word matches of the identifier under `position` in the open document."""
    text = document(uri)["text"]
    word = word_at(text, position["line"], position["character"])
    if not word:
        return []
    pattern = re.compile(rf"\b{re.escape(word)}\b")
    return [
        make_location(uri, line, match.start(), line, match.end())
        for line, row in enumerate(text.splitlines())
        for match in pattern.finditer(row)
    ]


def make_symbol(name, kind, uri, line):
    return {
        "name": name,
        "kind": kind,
        "location": make_location(uri, line, 0, line, len(name)),
    }


while True:
    message = read_message()
    if message is None:
        break

    method = message.get("method")
    params = message.get("params", {})

    if method is None:
        continue

    if method in crash_methods:
        sys.exit(1)

    if method in fail_methods:
        write_message(
            {
                "jsonrpc": "2.0",
                "id": message.get("id"),
                "error": {"code": -32603, "message": "fake server rejected request"},
            }
        )
        continue

    if method in wedge_methods:
        write_message({"jsonrpc": "2.0", "method": "aether/testing/requestReceived", "params": {"method": method}})
        continue

    if method in garble_methods:
        write_frame(b"{not json")

    if method == "initialize":
        root_path = uri_to_path(params["rootUri"])
        loading = os.path.exists(os.path.join(root_path, ".fake-loading"))
        experimental = params.get("capabilities", {}).get("experimental") or {}
        status_supported = experimental.get("serverStatusNotification", False)
        capabilities = {"hoverProvider": True}
        if pull_diagnostics:
            capabilities["diagnosticProvider"] = {
                "interFileDependencies": True,
                "workspaceDiagnostics": False,
            }
            client_workspace = params.get("capabilities", {}).get("workspace", {})
            refresh_supported = client_workspace.get("diagnostics", {}).get("refreshSupport", False)
        write_message(
            {
                "jsonrpc": "2.0",
                "id": message["id"],
                "result": {"capabilities": capabilities},
            }
        )
    elif method == "initialized":
        send_status()
        if pull_diagnostics:
            send_request(
                "client/registerCapability",
                {
                    "registrations": [
                        {
                            "id": "fake-watcher",
                            "method": "workspace/didChangeWatchedFiles",
                            "registerOptions": {"watchers": [{"globPattern": f"{root_path}/**/*"}]},
                        }
                    ]
                },
            )
    elif method == "workspace/didChangeWatchedFiles":
        if pull_diagnostics and refresh_supported:
            send_request("workspace/diagnostic/refresh", None)
    elif method == "textDocument/diagnostic":
        # TypeScript 7 rejects explicit nulls in these optional fields.
        if any(key in params and params[key] is None for key in ("identifier", "previousResultId")):
            write_message(
                {
                    "jsonrpc": "2.0",
                    "id": message["id"],
                    "error": {"code": -32602, "message": "null value is not allowed"},
                }
            )
            continue
        uri = params["textDocument"]["uri"]
        text = docs[uri]["text"] if uri in docs else read_disk(uri_to_path(uri))
        # Mimics a server still loading its project: the first pull after each open is cancelled.
        if "// cancel-first-pull" in text and uri not in cancelled_since_open:
            cancelled_since_open.add(uri)
            write_message(
                {
                    "jsonrpc": "2.0",
                    "id": message["id"],
                    "error": {"code": -32802, "message": "server cancelled", "data": {"retriggerRequest": True}},
                }
            )
            continue
        write_message(
            {
                "jsonrpc": "2.0",
                "id": message["id"],
                "result": {"kind": "full", "items": diagnostics_for(uri, text)},
            }
        )
    elif method == "textDocument/didOpen":
        document_item = params["textDocument"]
        uri = document_item["uri"]
        cancelled_since_open.discard(uri)
        state = document(uri)
        docs[uri] = {
            "open_count": state["open_count"] + 1,
            "text": document_item["text"],
        }
        publish(uri, document_item["text"])
    elif method == "textDocument/didChange":
        text_document = params["textDocument"]
        uri = text_document["uri"]
        text = params["contentChanges"][-1]["text"]
        state = document(uri)
        docs[uri] = {
            "open_count": max(state["open_count"], 1),
            "text": text,
        }
        publish(uri, text)
    elif method == "textDocument/didSave":
        uri = params["textDocument"]["uri"]
        save_counts[uri] = save_counts.get(uri, 0) + 1
    elif method == "textDocument/didClose":
        uri = params["textDocument"]["uri"]
        state = document(uri)
        open_count = max(state["open_count"] - 1, 0)
        if open_count == 0:
            docs.pop(uri, None)
        else:
            docs[uri] = {
                "open_count": open_count,
                "text": state["text"],
            }
    elif method == "fake/finishLoading":
        loading = False
        send_status()
        respond(message, None)
    elif method == "fake/releaseDiagnostics":
        diagnostics_released = True
        for uri, text in held_diagnostics.items():
            publish(uri, text)
        held_diagnostics.clear()
        respond(message, None)
    elif method == "textDocument/hover" and loading:
        respond(message, None)
    elif method == "textDocument/hover":
        uri = params["textDocument"]["uri"]
        state = document(uri)
        position = params["position"]
        value = (
            f"open_count={state['open_count']}; line={position['line']}; "
            f"character={position['character']}; save_count={save_counts.get(uri, 0)}; "
            f"text={state['text']}"
        )
        respond(message, {"contents": {"kind": "plaintext", "value": value}})
    elif method == "textDocument/definition":
        respond(message, occurrences(params["textDocument"]["uri"], params["position"])[:1])
    elif method == "textDocument/references":
        respond(message, occurrences(params["textDocument"]["uri"], params["position"]))
    elif method == "textDocument/documentSymbol":
        uri = params["textDocument"]["uri"]
        write_message(
            {
                "jsonrpc": "2.0",
                "id": message["id"],
                "result": [
                    make_symbol("ExampleStruct", 23, uri, 0),
                    make_symbol("example_fn", 12, uri, 0),
                ],
            }
        )
    elif method == "textDocument/rename":
        uri = params["textDocument"]["uri"]
        edits = [
            {"range": location["range"], "newText": params["newName"]}
            for location in occurrences(uri, params["position"])
        ]
        respond(message, {"changes": {uri: edits}})
    elif method == "shutdown":
        write_message(
            {
                "jsonrpc": "2.0",
                "id": message["id"],
                "result": None,
            }
        )
    else:
        if "id" in message:
            write_message(
                {
                    "jsonrpc": "2.0",
                    "id": message["id"],
                    "result": None,
                }
            )
