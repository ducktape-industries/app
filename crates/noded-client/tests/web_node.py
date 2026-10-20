#!/usr/bin/env python3
# A fake node for noded-client's browser tests (src/net/web/tests.rs): each request's path names what it answers.
import base64, hashlib, sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

KEPT = {}
GUID = b'258EAFA5-E914-47DA-95CA-C5AB0DC85B11'


class Node(BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.1'

    def log_message(self, *args):
        pass

    def head(self, status, length, kind='application/octet-stream', extra=()):
        self.send_response(status)
        self.send_header('access-control-allow-origin', '*')
        self.send_header('content-type', kind)
        self.send_header('content-length', str(length))
        for name, value in extra:
            self.send_header(name, value)
        self.end_headers()

    def until_closed(self):
        try:
            while self.connection.recv(4096):
                pass
        except OSError:
            pass
        self.close_connection = True

    def do_OPTIONS(self):
        self.send_response(204)
        for name in ('origin', 'methods', 'headers'):
            self.send_header(f'access-control-allow-{name}', '*')
        self.send_header('content-length', '0')
        self.end_headers()

    def do_GET(self):
        self.answer(b'')

    def do_POST(self):
        self.answer(self.rfile.read(int(self.headers.get('content-length') or 0)))

    def answer(self, sent):
        parts = self.path.strip('/').split('/')
        case = parts[0]
        if case == 'answer':
            self.pieces(bytes.fromhex(parts[1]))
        elif case == 'keep':
            KEPT[parts[1]] = sent
            self.pieces(bytes.fromhex(parts[2]))
        elif case == 'seen':
            self.pieces(KEPT.get(parts[1], b''))
        elif case == 'busy':
            self.head(503, 4, 'text/plain; charset=utf-8')
            self.wfile.write(b'busy')
        elif case == 'redirect':
            self.head(302, 0, extra=[('location', 'http://example.invalid/')])
        elif case == 'silent':
            self.head(200, 100)
            self.wfile.flush()
            self.until_closed()
        elif case == 'mute':
            self.until_closed()
        elif case == 'socket' and self.headers.get('upgrade', '').lower() == 'websocket':
            self.socket([bytes.fromhex(item) for item in parts[1].split(',')])
        else:
            self.head(404, 0)

    def pieces(self, data):
        self.head(200, len(data))
        for at in range(0, len(data), 3):
            self.wfile.write(data[at:at + 3])
            self.wfile.flush()

    def socket(self, messages):
        accept = base64.b64encode(hashlib.sha1(self.headers['sec-websocket-key'].encode() + GUID).digest())
        self.wfile.write(b'HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n'
                         b'Sec-WebSocket-Accept: ' + accept + b'\r\n\r\n')
        for message in messages:
            self.wfile.write(bytes([0x82, len(message)]) + message)
        self.wfile.write(bytes([0x88, 2, 0x03, 0xE8]))
        self.wfile.flush()
        self.until_closed()


server = ThreadingHTTPServer(('127.0.0.1', int(sys.argv[1])), Node)
print('listening', flush=True)
server.serve_forever()
