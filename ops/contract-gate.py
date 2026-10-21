#!/usr/bin/env python3
# A release builds only for the contract the live node speaks: this checkout's noded_client::wire::NODE_CONTRACT against what a node's /v1/status answers.
# Usage: ops/contract-gate.py NODE_URL. It exits 1 on a difference and on no readable status; ops/contract-gate-test.py tests it.
import re
import sys
import urllib.request
from pathlib import Path

WIRE = Path(__file__).resolve().parent.parent / "crates/noded-client/src/wire.rs"


def ours(wire=WIRE):
    found = re.findall(r"^pub const NODE_CONTRACT: u32 = (\d+);$", wire.read_text(), re.M)
    if len(found) != 1:
        raise SystemExit(f"{wire} names NODE_CONTRACT {len(found)} times, not once")
    return int(found[0])


class Reader:
    def __init__(self, data):
        self.data = data
        self.at = 0

    def take(self, count):
        if self.at + count > len(self.data):
            raise ValueError(f"the status ends at byte {len(self.data)}, before byte {self.at + count}")
        part = self.data[self.at:self.at + count]
        self.at += count
        return part

    def u32(self):
        return int.from_bytes(self.take(4), "little")

    def vec(self):
        return self.take(self.u32())


def contract_of(status):
    read = Reader(status)
    read.vec()
    read.take(8 + 8 + 8 + 8 + 32 + 32 + 8)
    read.vec()
    return read.u32()


def theirs(node):
    request = urllib.request.Request(node.rstrip("/") + "/v1/status", headers={"User-Agent": "ducktape-app-contract-gate"})
    with urllib.request.urlopen(request) as answer:
        return contract_of(answer.read())


def main():
    if len(sys.argv) != 2:
        raise SystemExit("usage: contract-gate.py NODE_URL")
    node = sys.argv[1]
    app = ours()
    try:
        live = theirs(node)
    except (OSError, ValueError) as failed:
        raise SystemExit(f"{node} gave no status ({failed}), so nothing shows this build speaks its contract {app}: not built")
    if live != app:
        raise SystemExit(f"{node} speaks contract {live} and this app speaks {app}: not built")
    print(f"{node} speaks contract {live}, as this app does")


if __name__ == "__main__":
    main()
