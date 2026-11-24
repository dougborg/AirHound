"""Test-only causal child readiness; deadlines are outer hang safeguards."""
import json
import os
from pathlib import Path
import select
import time


class ReadyPipe:
    def __init__(self, root):
        self.path = Path(root) / 'ready.fifo'
        os.mkfifo(self.path, 0o600)
        # Keep both ends open so setup never depends on writer scheduling.
        self.fd = os.open(self.path, os.O_RDWR | os.O_NONBLOCK)
        self.buffer = bytearray()

    def receive(self):
        # One infrastructure watchdog covers the entire message, not each chunk.
        deadline = time.monotonic() + 30
        while b'\n' not in self.buffer:
            remaining = deadline - time.monotonic()
            if remaining <= 0 or not select.select([self.fd], [], [], remaining)[0]:
                raise AssertionError('Child did not publish readiness (outer hang guard)')
            self.buffer.extend(os.read(self.fd, 4096))
            if len(self.buffer) > 8192:
                raise AssertionError('Oversized fixture readiness')
        line, _, remaining = self.buffer.partition(b'\n')
        self.buffer = bytearray(remaining)
        return json.loads(line)

    def write_code(self, expression):
        return f"with open({str(self.path)!r}, 'w') as ready: ready.write(json.dumps({expression})+'\\n'); ready.flush()"

    def close(self):
        os.close(self.fd)
