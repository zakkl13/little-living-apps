#!/usr/bin/env python3
import json
import os
import sys
import time
from pathlib import Path


def parse_arg(args, flag):
    for i, token in enumerate(args):
        if token == flag and i + 1 < len(args):
            return args[i + 1]
    return None


def read_call_index(path):
    if not path.exists():
        return 0
    try:
        return int(path.read_text(encoding="utf-8").strip() or "0")
    except Exception:
        return 0


def write_call_index(path, value):
    path.write_text(str(value), encoding="utf-8")


def append_log(path, payload):
    with path.open("a", encoding="utf-8") as f:
        f.write(json.dumps(payload, ensure_ascii=False) + "\n")


def emit(event):
    print(json.dumps(event, ensure_ascii=False), flush=True)


def main() -> int:
    args = sys.argv[1:]
    stdin_text = sys.stdin.read()

    events_path = os.environ.get("CODEX_MOCK_EVENTS")
    log_path = os.environ.get("CODEX_MOCK_LOG")
    call_index_path = os.environ.get("CODEX_MOCK_CALL_INDEX")
    delay_ms = int(os.environ.get("CODEX_MOCK_STREAM_DELAY_MS", "0"))
    infinite = os.environ.get("CODEX_MOCK_INFINITE") == "1"
    exit_code = int(os.environ.get("CODEX_MOCK_EXIT_CODE", "0"))
    enforce_git_check = os.environ.get("CODEX_MOCK_ENFORCE_GIT_CHECK", "1") == "1"

    call_index = 0
    if call_index_path:
        call_index_file = Path(call_index_path)
        call_index = read_call_index(call_index_file)
        write_call_index(call_index_file, call_index + 1)

    output_schema_path = parse_arg(args, "--output-schema")
    output_schema_exists = False
    output_schema = None
    if output_schema_path:
        schema_path = Path(output_schema_path)
        output_schema_exists = schema_path.exists()
        if output_schema_exists:
            try:
                output_schema = json.loads(schema_path.read_text(encoding="utf-8"))
            except Exception:
                output_schema = "__invalid_json__"

    if log_path:
        append_log(
            Path(log_path),
            {
                "args": args,
                "stdin": stdin_text,
                "env": dict(os.environ),
                "call_index": call_index,
                "output_schema_path": output_schema_path,
                "output_schema_exists": output_schema_exists,
                "output_schema": output_schema,
            },
        )

    if enforce_git_check and "--cd" in args and "--skip-git-repo-check" not in args:
        print("Not inside a trusted directory", file=sys.stderr, flush=True)
        return 2

    if infinite:
        thread_id = os.environ.get("CODEX_MOCK_THREAD_ID", "thread_infinite")
        emit({"type": "thread.started", "thread_id": thread_id})
        emit({"type": "turn.started"})
        i = 0
        while True:
            emit(
                {
                    "type": "item.updated",
                    "item": {
                        "id": f"cmd_{i}",
                        "type": "command_execution",
                        "command": "sleep 1",
                        "aggregated_output": "",
                        "status": "in_progress",
                    },
                }
            )
            i += 1
            if delay_ms > 0:
                time.sleep(delay_ms / 1000.0)
    else:
        events = []
        if events_path:
            try:
                loaded = json.loads(Path(events_path).read_text(encoding="utf-8"))
                if isinstance(loaded, list):
                    events = loaded
            except Exception:
                events = []

        selected = []
        if call_index < len(events) and isinstance(events[call_index], list):
            selected = events[call_index]

        for event in selected:
            if isinstance(event, dict):
                emit(event)
            if delay_ms > 0:
                time.sleep(delay_ms / 1000.0)

    if exit_code != 0:
        print("mock codex failure", file=sys.stderr, flush=True)

    return exit_code


if __name__ == "__main__":
    raise SystemExit(main())
