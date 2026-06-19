import json
import sys

docs = {}


def write_message(msg):
    body = json.dumps(msg, separators=(",", ":")).encode("utf-8")
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


def publish(uri, text):
    diagnostics = []
    if "error" in text.lower():
        diagnostics.append(
            {
                "range": {
                    "start": {"line": 0, "character": 0},
                    "end": {"line": 0, "character": 5},
                },
                "severity": 1,
                "message": "error token",
            }
        )

    write_message(
        {
            "jsonrpc": "2.0",
            "method": "textDocument/publishDiagnostics",
            "params": {
                "uri": uri,
                "diagnostics": diagnostics,
            },
        }
    )


def document(uri):
    return docs.get(uri, {"open_count": 0, "text": ""})


def first_uri():
    return next(iter(docs), "file:///workspace.rs")


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


def make_symbol(name, kind, uri, line, container_name=None):
    symbol = {
        "name": name,
        "kind": kind,
        "location": make_location(uri, line, 0, line, len(name)),
    }
    if container_name is not None:
        symbol["containerName"] = container_name
    return symbol


def make_call_hierarchy_item(name, uri, line):
    return {
        "name": name,
        "kind": 12,
        "uri": uri,
        "range": make_range(line, 0, line, len(name)),
        "selectionRange": make_range(line, 0, line, len(name)),
    }


while True:
    message = read_message()
    if message is None:
        break

    method = message.get("method")
    params = message.get("params", {})

    if method == "initialize":
        write_message(
            {
                "jsonrpc": "2.0",
                "id": message["id"],
                "result": {
                    "capabilities": {
                        "hoverProvider": True,
                    }
                },
            }
        )
    elif method == "initialized":
        continue
    elif method == "textDocument/didOpen":
        document_item = params["textDocument"]
        uri = document_item["uri"]
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
        continue
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
    elif method == "textDocument/hover":
        uri = params["textDocument"]["uri"]
        state = document(uri)
        write_message(
            {
                "jsonrpc": "2.0",
                "id": message["id"],
                "result": {
                    "contents": {
                        "kind": "plaintext",
                        "value": f"open_count={state['open_count']}; text={state['text']}",
                    }
                },
            }
        )
    elif method == "textDocument/definition":
        uri = params["textDocument"]["uri"]
        write_message(
            {
                "jsonrpc": "2.0",
                "id": message["id"],
                "result": [make_location(uri, 0, 0, 0, 10)],
            }
        )
    elif method == "textDocument/implementation":
        uri = params["textDocument"]["uri"]
        write_message(
            {
                "jsonrpc": "2.0",
                "id": message["id"],
                "result": [make_location(uri, 0, 0, 0, 10)],
            }
        )
    elif method == "textDocument/references":
        uri = params["textDocument"]["uri"]
        write_message(
            {
                "jsonrpc": "2.0",
                "id": message["id"],
                "result": [
                    make_location(uri, 0, 0, 0, 10),
                    make_location(uri, 4, 0, 4, 10),
                ],
            }
        )
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
    elif method == "workspace/symbol":
        uri = first_uri()
        write_message(
            {
                "jsonrpc": "2.0",
                "id": message["id"],
                "result": [
                    make_symbol("example_fn", 12, uri, 0, "module"),
                ],
            }
        )
    elif method == "textDocument/prepareCallHierarchy":
        uri = params["textDocument"]["uri"]
        write_message(
            {
                "jsonrpc": "2.0",
                "id": message["id"],
                "result": [make_call_hierarchy_item("example_fn", uri, 0)],
            }
        )
    elif method == "callHierarchy/incomingCalls":
        uri = params["item"]["uri"]
        write_message(
            {
                "jsonrpc": "2.0",
                "id": message["id"],
                "result": [
                    {
                        "from": make_call_hierarchy_item("caller_fn", uri, 4),
                        "fromRanges": [make_range(4, 0, 4, 9)],
                    }
                ],
            }
        )
    elif method == "callHierarchy/outgoingCalls":
        uri = params["item"]["uri"]
        write_message(
            {
                "jsonrpc": "2.0",
                "id": message["id"],
                "result": [
                    {
                        "to": make_call_hierarchy_item("callee_fn", uri, 6),
                        "fromRanges": [make_range(0, 0, 0, 10)],
                    }
                ],
            }
        )
    elif method == "textDocument/rename":
        uri = params["textDocument"]["uri"]
        new_name = params["newName"]
        write_message(
            {
                "jsonrpc": "2.0",
                "id": message["id"],
                "result": {
                    "changes": {
                        uri: [
                            {
                                "range": make_range(0, 0, 0, 10),
                                "newText": new_name,
                            }
                        ]
                    }
                },
            }
        )
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
