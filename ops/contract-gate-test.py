#!/usr/bin/env python3
# Tests for contract-gate.py against a local stand-in node answering /v1/status as duckpond did (a contract-9 status recorded from it), and the checkout's own wire.rs.
# Run it with `python3 ops/contract-gate-test.py`; it needs only python3.
import http.server
import importlib.util
import shutil
import subprocess
import sys
import tempfile
import threading
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("contract_gate", HERE / "contract-gate.py")
gate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gate)

STATUS = bytes.fromhex("080000006475636b706f6e64c036714aa1010000e8030000000000004000000000000000dc13020000000000dfc0f2967d951a2b07c824d9ab7806acc4819901eddb9cc59c9682cb42bd2596ea3b3309239a3defceeb6060eefeb6a8b5e446f8c8fdcf7a6357c417943a9d984f080000000000002000000075bf265d1e35103677d009f4fbba0401996c437f56e0481b3a9f9c1e53afe37d09000000725bcad16a1005eef8efe8db1d84ec5d58d00a6b4fd38d6446e5a3d845f71fb5090000000600000061636365737306000000616363657373060000006177616974730600000061776169747305000000626c6f627305000000626c6f627306000000656772657373060000006567726573730400000068616c6f0400000068616c6f080000006964656e74697479080000006964656e746974790800000070726f6772616d730800000070726f6772616d73090000007363686564756c6572090000007363686564756c65720a00000076616c696461746f72730600000076616c736574")


class Node:
    def __init__(self, status, body):
        node = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                node.asked.append((self.path, self.headers.get("User-Agent")))
                self.send_response(node.status)
                self.send_header("Content-Length", str(len(node.body)))
                self.end_headers()
                self.wfile.write(node.body)

            def log_message(self, *args):
                pass

        self.status = status
        self.body = body
        self.asked = []
        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.thread = threading.Thread(target=self.server.serve_forever)
        self.thread.start()
        self.url = f"http://127.0.0.1:{self.server.server_address[1]}"

    def close(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join()


class Gate(unittest.TestCase):
    def run_gate(self, contract, status=200, body=STATUS):
        tree = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, tree)
        (tree / "ops").mkdir()
        shutil.copy(HERE / "contract-gate.py", tree / "ops" / "contract-gate.py")
        wire = tree / "crates/noded-client/src"
        wire.mkdir(parents=True)
        (wire / "wire.rs").write_text(f"use abi::Root;\n\npub const NODE_CONTRACT: u32 = {contract};\npub const FRAME_NAMESPACE: &[u8] = b\"ducktape:frame\";\n")
        node = Node(status, body)
        self.addCleanup(node.close)
        ran = subprocess.run([sys.executable, str(tree / "ops" / "contract-gate.py"), node.url + "/"], capture_output=True, text=True)
        return ran, node

    def test_a_status_with_the_apps_contract_lets_the_build_go(self):
        ran, node = self.run_gate(9)
        self.assertEqual(ran.returncode, 0, ran.stderr)
        self.assertIn("speaks contract 9, as this app does", ran.stdout)
        self.assertEqual(node.asked, [("/v1/status", "ducktape-app-contract-gate")])

    def test_a_node_on_an_older_contract_stops_the_build(self):
        ran, _ = self.run_gate(10)
        self.assertEqual(ran.returncode, 1)
        self.assertIn("speaks contract 9 and this app speaks 10: not built", ran.stderr)

    def test_a_node_on_a_newer_contract_stops_the_build(self):
        ran, _ = self.run_gate(8)
        self.assertEqual(ran.returncode, 1)
        self.assertIn("speaks contract 9 and this app speaks 8: not built", ran.stderr)

    def test_an_answer_that_is_no_status_stops_the_build(self):
        ran, _ = self.run_gate(9, body=b"<html>bad gateway</html>")
        self.assertEqual(ran.returncode, 1)
        self.assertIn("gave no status", ran.stderr)

    def test_a_refusal_stops_the_build(self):
        ran, _ = self.run_gate(9, status=502, body=b"")
        self.assertEqual(ran.returncode, 1)
        self.assertIn("gave no status (HTTP Error 502", ran.stderr)

    def test_the_recorded_status_reads_as_contract_9(self):
        self.assertEqual(gate.contract_of(STATUS), 9)

    def test_this_checkouts_wire_names_one_contract(self):
        self.assertIsInstance(gate.ours(), int)


if __name__ == "__main__":
    unittest.main()
