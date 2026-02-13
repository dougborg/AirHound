#!/bin/bash
# Auto-format Rust files after Claude edits or creates them.
# Receives JSON on stdin with the tool input.
FILE_PATH=$(jq -r '.tool_input.file_path // empty')
if [[ "$FILE_PATH" == *.rs ]]; then
  cargo fmt -- "$FILE_PATH" 2>/dev/null
fi
exit 0
