#!/usr/bin/env python3
"""Regenerate synthetic RPC fixtures from the pinned official schema.

Requires protoc and the Python protobuf runtime. Opens no device and sends no RPC.
"""

import argparse
import hashlib
import importlib
import json
import pathlib
import subprocess
import sys
import tempfile

REVISION = "ea4f185f5eaa265955c520eae2832887ee6aa5e4"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--schema", type=pathlib.Path, required=True)
    parser.add_argument("--output", type=pathlib.Path, required=True)
    args = parser.parse_args()
    schema = args.schema.resolve()
    actual_revision = subprocess.check_output(
        ["git", "-C", str(schema), "rev-parse", "HEAD"], text=True, timeout=10
    ).strip()
    if actual_revision != REVISION:
        raise SystemExit(f"Schema must be checked out at {REVISION}")
    subprocess.run(
        ["git", "-C", str(schema), "diff", "--exit-code", "HEAD", "--", "*.proto"],
        check=True, capture_output=True, timeout=10,
    )
    with tempfile.TemporaryDirectory(prefix="airhound-rpc-goldens-") as generated:
        protos = sorted(schema.glob("*.proto"))
        subprocess.run(
            ["protoc", "-I", str(schema), "--python_out=" + generated, *map(str, protos)],
            check=True, capture_output=True, timeout=30,
        )
        sys.path.insert(0, generated)
        pb = importlib.import_module("flipper_pb2")
        app = importlib.import_module("application_pb2")
        system = importlib.import_module("system_pb2")
        from google.protobuf.internal.encoder import _VarintBytes

        cases = [
            ("start", "app_start_request", app.StartRequest(
                name="/ext/apps/GPIO/airhound_bridge.fap", args="RPC"), 1, 0),
            ("exit", "app_exit_request", app.AppExitRequest(), 2, 0),
            ("data", "app_data_exchange_request", app.DataExchangeRequest(
                data=bytes(range(256))), 3, 0),
            ("event", "app_data_exchange_request", app.DataExchangeRequest(
                data=b"AHFB\x01\x82"), 0, 0),
            ("ack", "empty", pb.Empty(), 3, 0),
            ("busy", "empty", pb.Empty(), 1, 17),
            ("started", "app_state_response", app.AppStateResponse(
                state=app.APP_STARTED), 0, 0),
            ("stop_session", "stop_session", pb.StopSession(), 4, 0),
            ("ping", "system_ping_request", system.PingRequest(
                data=b"airhound-bridge-v1"), 5, 0),
            ("pong", "system_ping_response", system.PingResponse(
                data=b"airhound-bridge-v1"), 5, 0),
        ]
        args.output.mkdir(parents=True, exist_ok=True)
        frames = []
        for name, field, body, command, status in cases:
            message = pb.Main(command_id=command, command_status=status)
            getattr(message, field).CopyFrom(body)
            encoded = message.SerializeToString(deterministic=True)
            framed = _VarintBytes(len(encoded)) + encoded
            (args.output / (name + ".bin")).write_bytes(framed)
            frames.append({
                "name": name, "field": field, "command_id": command,
                "command_status": status,
                "sha256": hashlib.sha256(framed).hexdigest(),
            })
        (args.output / "provenance.json").write_text(json.dumps({
            "schema_revision": REVISION,
            "generator": "official protobuf Python modules regenerated from the pinned schema",
            "fixtures": frames,
        }, indent=2) + "\n")


if __name__ == "__main__":
    main()
