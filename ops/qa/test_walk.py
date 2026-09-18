"""walk.py against a fake door and a fake Jev on loopback: python3 -m unittest discover ops/qa"""
import http.server
import json
import os
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).parent))
import walk  # noqa: E402

TOKEN = 'door-token'
JOIN = {'id': 'onboarding:network-join', 'role': 'Button', 'name': 'Join with invitation',
        'state': [], 'actions': ['press'], 'in': 'onboarding'}
FIELD = {'id': 'onboarding:join_invite/field', 'role': 'PasswordInput', 'name': 'Invitation',
         'value': '•••', 'state': [], 'actions': ['set_value'], 'in': 'onboarding'}


class Server(http.server.ThreadingHTTPServer):
    """Answers every request with `route(method, path, headers, body) -> (status, value)`."""

    def __init__(self, route):
        self.route, self.calls = route, []

        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def answer(self):
                body = self.rfile.read(int(self.headers.get('Content-Length') or 0))
                self.server.calls.append((self.command, self.path, body.decode()))
                status, value = self.server.route(self.command, self.path, self.headers, body)
                data = value if isinstance(value, bytes) else json.dumps(value).encode()
                self.send_response(status)
                self.send_header('Content-Length', str(len(data)))
                self.end_headers()
                self.wfile.write(data)

            do_GET = do_POST = answer

        super().__init__(('127.0.0.1', 0), Handler)
        threading.Thread(target=self.serve_forever, daemon=True).start()

    def url(self, path=''):
        return f'http://127.0.0.1:{self.server_address[1]}{path}'


def fake_door(nodes, leak=False):
    """A door over `nodes`; an act on JOIN swaps the screen, an act on FIELD
    answers with the raw value when `leak` (a door that forgot to mask)."""
    screen = {'nodes': list(nodes)}

    def route(method, path, headers, body):
        if headers.get('Authorization') != 'Bearer ' + TOKEN:
            return 401, {'error': 'token'}
        if path.startswith('/tree'):
            return 200, screen['nodes']
        if path.startswith('/actions'):
            return 200, [{'id': n['id'], 'action': a, 'label': f"{n['role']} {n['name']}"}
                         for n in screen['nodes'] for a in n['actions']]
        request = json.loads(body)
        if request['id'] not in {n['id'] for n in screen['nodes']}:
            return 404, {'error': 'no such node'}
        if request['id'] == JOIN['id']:
            screen['nodes'] = [FIELD]
            return 200, {'appeared': [FIELD], 'disappeared': [JOIN['id']], 'changed': []}
        value = request['value'] if leak else '•••'
        return 200, {'appeared': [], 'disappeared': [], 'changed': [dict(FIELD, value=value)]}
    return Server(route)


def fake_jev(replies):
    """Answers in turn: a (status, body) pair per call."""
    queue = list(replies)
    return Server(lambda *_: queue.pop(0) if queue else (500, {'error': 'no more'}))


def jev(kind, answer, tokens=1000):
    return 200, {'answers': {'q': dict(answer, type=kind)}, 'usage': {'input_tokens': tokens, 'output_tokens': 9}}


class WalkTest(unittest.TestCase):
    def setUp(self):
        self.out = Path(tempfile.mkdtemp(prefix='walk-test-'))
        self.addCleanup(shutil.rmtree, self.out, True)

    def walk(self, steps, door, judge, secrets=None, **kw):
        scenario = {'name': 't', 'rig': {'display': False, 'secrets': secrets or {}}, 'steps': steps}
        with mock.patch.object(walk, 'JEV_URL', judge.url('/v1/systemone')), \
                mock.patch.dict(os.environ, {'JEV_API_KEY': 'test-key'}):
            run = walk.Walk(scenario, self.out, {}, **kw)
            door_file = run.rig.door_file()
            door_file.parent.mkdir(parents=True, exist_ok=True)
            door_file.write_text(json.dumps({'port': door.server_address[1], 'token': TOKEN}))
            result = run.run()
        for server in (door, judge):
            server.shutdown()
            server.server_close()
        return result

    def lines(self, name):
        return [json.loads(line) for line in (self.out / name).read_text().splitlines()]

    def test_choice_can_only_pick_from_the_list(self):
        step = {'say': 'join with an invitation', 'expect': 'the invitation field shows'}
        door = fake_door([JOIN])
        result = self.walk([step], door, fake_jev([jev('choice', {'choice': 'o7'})]))
        self.assertEqual(result['result'], 'FAIL-UNJUDGED')
        self.assertFalse([c for c in door.calls if c[0] == 'POST'], 'an invented option must never reach the door')
        self.assertTrue((self.out / 'failing-tree.json').exists())

        door = fake_door([JOIN])
        judge = fake_jev([jev('choice', {'choice': 'o0'}), jev('noul', {'noul': 0.93})])
        result = self.walk([step], door, judge)
        self.assertEqual(result['result'], 'PASS')
        acts = [json.loads(c[2]) for c in door.calls if c[0] == 'POST']
        self.assertEqual(acts, [{'id': JOIN['id'], 'action': 'press', 'value': None}])
        asked = json.loads(judge.calls[0][2])['questions']['q']
        self.assertEqual(asked['criteria'], {'o0': 'onboarding:network-join press Button Join with invitation',
                                             'none': 'none of these'})
        self.assertEqual(self.lines('transcript.jsonl')[0]['verdict'], 'pass')

    def test_none_of_these_is_a_fail_not_a_guess(self):
        step = {'say': 'open settings', 'expect': 'settings shows'}
        result = self.walk([step], fake_door([JOIN]), fake_jev([jev('choice', {'choice': 'none'})]))
        self.assertEqual(result['result'], 'FAIL')

    def test_unjudged_on_jev_error(self):
        step = {'say': 'join with an invitation', 'expect': 'the invitation field shows'}
        for reply in ((500, {'error': 'down'}), (200, b'not json'), (200, {'answers': {}})):
            door = fake_door([JOIN])
            result = self.walk([step], door, fake_jev([reply]))
            self.assertEqual(result['result'], 'FAIL-UNJUDGED', reply)
            self.assertEqual(result['steps_passed'], 0)
            self.assertIn('error', self.lines('ledger.jsonl')[-1])
        with mock.patch.dict(os.environ, {'JEV_API_KEY': ''}):
            scenario = {'name': 't', 'rig': {'display': False}, 'steps': [step]}
            run = walk.Walk(scenario, self.out, {})
            self.assertRaises(walk.Unjudged, run.jev.ask, 'noul', {}, {})

    def test_masking_passes_through_and_secrets_never_leave(self):
        secret = self.out.parent / f'{self.out.name}-invite'
        secret.write_text('INVITE-LINE-that-must-not-leak\n')
        self.addCleanup(secret.unlink)
        steps = [{'say': 'paste the invitation', 'expect': 'the field holds an invitation', 'secret': 'invite'}]
        door = fake_door([FIELD], leak=True)  # even a door that echoes the value
        judge = fake_jev([jev('choice', {'choice': 'o0'}), jev('noul', {'noul': 0.9})])
        result = self.walk(steps, door, judge, secrets={'invite': {'file': str(secret)}})
        self.assertEqual(result['result'], 'PASS')
        acts = [json.loads(c[2]) for c in door.calls if c[0] == 'POST']
        self.assertEqual(acts[0]['value'], 'INVITE-LINE-that-must-not-leak', 'the door gets the value')
        sent = ''.join(c[2] for c in judge.calls)
        self.assertIn('\\u2022\\u2022\\u2022', sent, 'the door mask reaches the judge as is')
        self.assertNotIn('INVITE-LINE', sent)
        for path in self.out.rglob('*'):
            if path.is_file():
                self.assertNotIn('INVITE-LINE', path.read_text(errors='replace'), path)
        self.assertEqual(self.lines('transcript.jsonl')[0]['value'], '{secret:invite}')

    def test_ledger_math(self):
        steps = [{'say': 'join with an invitation', 'expect': 'the invitation field shows'}]
        judge = fake_jev([jev('choice', {'choice': 'o0'}, 1_000_000), jev('noul', {'noul': 0.8}, 500_000)])
        result = self.walk(steps, fake_door([JOIN]), judge)
        ledger = self.lines('ledger.jsonl')
        self.assertEqual([(l['type'], l['input_tokens']) for l in ledger], [('choice', 1_000_000), ('noul', 500_000)])
        self.assertAlmostEqual(ledger[0]['usd'], 0.042)
        self.assertAlmostEqual(ledger[1]['usd'], 0.021)
        self.assertAlmostEqual(result['usd'], 0.063)
        self.assertEqual(json.loads((self.out / 'result.json').read_text())['usd'], result['usd'])

    def test_teardown_signals_only_recorded_processes_of_its_own(self):
        rig = walk.Rig(self.out / 'rig', {'display': False}, {})
        sleeper = rig.paths['bin'] / 'sleep'
        shutil.copy2(shutil.which('sleep'), sleeper)
        mine, mine_proc = rig.spawn('mine', [str(sleeper), '30'])
        outside, outside_proc = rig.spawn('outside', [shutil.which('sleep'), '30'])
        reused, reused_proc = rig.spawn('reused', [str(sleeper), '30'])
        reused['start'] += 1  # as if the pid now named another process
        stranger = subprocess.Popen([str(sleeper), '30'], start_new_session=True)  # never recorded
        try:
            stopped = rig.teardown()
            self.assertEqual(stopped[f"mine:{mine['pid']}"], 'stopped')
            self.assertEqual(mine_proc.returncode, -signal.SIGTERM)
            self.assertEqual(stopped[f"outside:{outside['pid']}"], 'not ours: left alone')
            self.assertEqual(stopped[f"reused:{reused['pid']}"], 'not ours: left alone')
            for proc in (outside_proc, reused_proc, stranger):
                self.assertIsNone(proc.poll(), 'teardown touched a process it must leave alone')
            recorded = json.loads((rig.root / 'pids.json').read_text())
            self.assertEqual([r['pid'] for r in recorded], [mine['pid'], outside['pid'], reused['pid']])
        finally:
            for proc in (outside_proc, reused_proc, stranger):  # this test's own children, by their pid
                proc.terminate()
                proc.wait()


if __name__ == '__main__':
    unittest.main()
