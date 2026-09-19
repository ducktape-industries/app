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
import time
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


SHORTCUTS = [{'keys': 'tab', 'action': 'root::Tab'}, {'keys': 'ctrl-k', 'action': 'shell::OpenPalette'}]


def fake_door(nodes, leak=False, wait_cap_ms=None):
    """A door over `nodes`; an act on JOIN swaps the screen, an act on FIELD
    answers with the raw value when `leak` (a door that forgot to mask). Keys:
    `tab` moves focus along the nodes that have actions, `enter` on JOIN acts
    on it, text types into a focused FIELD. A wait answers like the door's: at
    most `wait_cap_ms` (its 60 s cap, scaled down), then 408."""
    screen = {'nodes': json.loads(json.dumps(nodes))}

    def route(method, path, headers, body):
        if headers.get('Authorization') != 'Bearer ' + TOKEN:
            return 401, {'error': 'token'}
        if path.startswith('/tree'):
            return 200, screen['nodes']
        if path.startswith('/actions'):
            return 200, [{'id': n['id'], 'action': a, 'label': f"{n['role']} {n['name']}"}
                         for n in screen['nodes'] for a in n['actions']]
        if path.startswith('/keys'):
            return 200, SHORTCUTS
        request = json.loads(body)
        if path == '/wait':
            ms = request['deadline_ms'] if wait_cap_ms is None else min(request['deadline_ms'], wait_cap_ms)
            time.sleep(ms / 1000)
            found = [n for n in screen['nodes'] if request.get('name', '') in n['name']]
            return (200, {'node': found[0]}) if found else (408, {'error': 'deadline passed', 'tree': screen['nodes']})
        if path == '/key':
            stops = [n for n in screen['nodes'] if n['actions']]
            at = next((i for i, n in enumerate(stops) if 'focused' in n['state']), -1)
            if request['keys'] == 'tab' and stops:
                for n in stops:
                    n['state'] = [flag for flag in n['state'] if flag != 'focused']
                stops[(at + 1) % len(stops)]['state'].append('focused')
            elif request['keys'] == 'enter' and at >= 0 and stops[at]['id'] == JOIN['id']:
                screen['nodes'] = [dict(FIELD, state=[])]
            return 200, {'appeared': [], 'disappeared': [], 'changed': []}
        if request['id'] not in {n['id'] for n in screen['nodes']}:
            return 404, {'error': 'no such node'}
        if request['id'] == JOIN['id']:
            screen['nodes'] = [FIELD]
            return 200, {'appeared': [FIELD], 'disappeared': [JOIN['id']], 'changed': []}
        value = request['value'] if leak else '•••'
        return 200, {'appeared': [], 'disappeared': [], 'changed': [dict(FIELD, value=value)]}
    server = Server(route)
    server.screen = screen
    return server


def door_nodes(door):
    """The fake door's live screen, for a test that changes it mid-walk."""
    return door.screen['nodes']


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

    def walk(self, steps, door, judge, secrets=None, scenarios=None, params=None, **kw):
        scenario = {'name': 't', 'rig': {'display': False, 'secrets': secrets or {}}, 'steps': steps}
        with mock.patch.object(walk, 'JEV_URL', judge.url('/v1/systemone')), \
                mock.patch.dict(os.environ, {'JEV_API_KEY': 'test-key'}):
            run = walk.Walk(scenarios or scenario, self.out, params or {}, **kw)
            door_file = run.rig.door_file()
            door_file.parent.mkdir(parents=True, exist_ok=True)
            door_file.write_text(json.dumps({'port': door.server_address[1], 'token': TOKEN}))
            result = run.run()
        for server in (door, judge):
            server.shutdown()
            server.server_close()
        return result

    def run_dir(self):
        return sorted(self.out.glob('run-*'))[-1]

    def lines(self, name):
        return [json.loads(line) for line in (self.run_dir() / name).read_text().splitlines()]

    def test_choice_can_only_pick_from_the_list(self):
        step = {'say': 'join with an invitation', 'expect': 'the invitation field shows'}
        door = fake_door([JOIN])
        result = self.walk([step], door, fake_jev([jev('choice', {'choice': 'o7'})]))
        self.assertEqual(result['result'], 'FAIL-UNJUDGED')
        self.assertFalse([c for c in door.calls if c[0] == 'POST'], 'an invented option must never reach the door')
        failure = result['scenarios'][0]['failures'][0]
        self.assertTrue(Path(failure['tree']).exists())
        self.assertEqual(failure['offers'], ['onboarding:network-join press Button Join with invitation'])
        first = (self.run_dir() / 'transcript.jsonl').read_text()

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
        # a second run on the same --out is numbered: the first one's output stays as it was
        self.assertEqual(sorted(d.name for d in self.out.glob('run-*')), ['run-01', 'run-02'])
        self.assertEqual((self.out / 'run-01' / 'transcript.jsonl').read_text(), first)
        self.assertEqual(json.loads((self.out / 'run-01' / 'result.json').read_text())['result'], 'FAIL-UNJUDGED')

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
        self.assertEqual(json.loads((self.run_dir() / 'result.json').read_text())['usd'], result['usd'])

    def test_private_text_fills_the_asked_words_and_goes_nowhere_else(self):
        words = {n: f'SECRETword{n}x' for n in range(1, 7)}
        phrase = [{'id': f'onboarding:phrase-word/{n}', 'role': 'Label', 'name': '•••', 'value': '•••',
                   'state': [], 'actions': [], 'in': 'onboarding'} for n in (1, 4, 2, 5, 3, 6)]  # two columns
        saved = {'id': 'onboarding:phrase-saved', 'role': 'Button', 'name': 'I wrote it down',
                 'state': [], 'actions': ['press'], 'in': 'onboarding'}
        prompt = {'id': 'onboarding:Type words 5, 2 and 3', 'role': 'Label',
                  'name': 'Type words 5, 2 and 3 — in that order, separated by spaces.',
                  'state': [], 'actions': [], 'in': 'onboarding'}
        answer = {'id': 'onboarding:phrase-answer', 'role': 'PasswordInput', 'name': 'Requested words',
                  'value': '•••', 'state': [], 'actions': ['type', 'set_value'], 'in': 'onboarding'}
        screen = {'nodes': phrase + [saved]}

        def route(method, path, headers, body):
            if path.startswith('/tree'):
                return 200, screen['nodes']
            if path.startswith('/actions'):
                return 200, [{'id': n['id'], 'action': a, 'label': f"{n['role']} {n['name']}"}
                             for n in screen['nodes'] for a in n['actions']]
            request = json.loads(body)
            if path == '/reveal':
                node = next(n for n in screen['nodes'] if n['id'] == request['id'])
                if node['role'] == 'PasswordInput':
                    return 403, {'error': 'secure input'}
                return 200, dict(node, value=words[int(node['id'].rsplit('/', 1)[1])])
            if request['id'] == saved['id']:
                screen['nodes'] = [prompt, answer]
            return 200, {'appeared': [], 'disappeared': [], 'changed': []}

        door = Server(route)
        steps = [{'kind': 'private_remember', 'say': 'remember the phrase',
                  'match': {'ids_prefix': 'onboarding:phrase-word/'}, 'as': 'phrase'},
                 {'say': 'press I wrote it down', 'expect': 'the confirm step asks for words'},
                 {'kind': 'private_copy', 'say': 'type the asked words', 'how': 'type',
                  'from_indexed': {'memory': 'phrase', 'prompt': {'name': 'Type words', 'in': 'onboarding'},
                                   'regex': r'Type words ([0-9, and]+)'},
                  'into': {'role': 'PasswordInput', 'ids_prefix': 'onboarding:phrase-answer'}},
                 {'kind': 'private_copy', 'say': 'a secure input is never revealed',
                  'from': 'onboarding:phrase-answer', 'into': 'onboarding:phrase-answer'},
                 {'kind': 'private_copy', 'say': 'never into a field the door shows',
                  'from': 'onboarding:phrase-word/1', 'into': prompt['id']}]
        judge = fake_jev([jev('choice', {'choice': 'o0'}), jev('noul', {'noul': 0.9})])
        with self.assertRaises(walk.Refused):  # a private step needs the rig to ask for reveal
            walk.Walk({'name': 't', 'rig': {'display': False}, 'steps': steps}, self.out, {})
        with mock.patch.object(walk, 'JEV_URL', judge.url('/v1/systemone')), \
                mock.patch.dict(os.environ, {'JEV_API_KEY': 'test-key'}):
            run = walk.Walk({'name': 't', 'rig': {'display': False, 'private': True}, 'steps': steps},
                            self.out, {}, keep_going=True)
            self.assertEqual(run.rig.env['DUCKTAPE_AX_DOOR_PRIVATE'], '1')
            run.rig.door_file().parent.mkdir(parents=True, exist_ok=True)
            run.rig.door_file().write_text(json.dumps({'port': door.server_address[1], 'token': TOKEN}))
            run.run()
        for server in (door, judge):
            server.shutdown()
            server.server_close()
        acts = [json.loads(c[2]) for c in door.calls if c[1] == '/act']
        self.assertEqual(acts[-1], {'id': answer['id'], 'action': 'type',
                                    'value': 'SECRETword5x SECRETword2x SECRETword3x'})
        lines = self.lines('transcript.jsonl')
        self.assertEqual([l['verdict'] for l in lines], ['pass', 'pass', 'pass', 'unjudged', 'fail'])
        self.assertEqual(lines[2]['copied'], f"private_copy from phrase #5,2,3 into {answer['id']}: 38 chars")
        self.assertIn('403', lines[3]['reason'])
        self.assertIn('not a showing secure input', lines[4]['reason'])
        self.assertTrue(judge.calls, 'the judge was asked while the phrase was held')
        for method, path, body in judge.calls:
            self.assertNotIn('SECRET', body, 'a Jev request carried private text')
        for path in self.out.rglob('*'):
            if path.is_file():
                self.assertNotIn('SECRET', path.read_text(errors='replace'), path)

    def test_the_judge_reads_the_runs_own_names_not_the_holes(self):
        steps = [{'say': 'join as {param:walk_tag}', 'expect': 'the field holds qa-{param:walk_tag}'}]
        judge = fake_jev([jev('choice', {'choice': 'o0'}, 10), jev('noul', {'noul': 0.9}, 10)])
        result = self.walk(steps, fake_door([JOIN]), judge, params={'walk_tag': 'abc1234'})
        self.assertEqual(result['result'], 'PASS')
        sent = ' '.join(c[2] for c in judge.calls)
        self.assertIn('join as abc1234', sent)
        self.assertIn('the field holds qa-abc1234', sent)
        self.assertNotIn('{param:', sent)

    def test_a_wait_outlasts_the_door_cap(self):
        """The door answers one wait within 60 s; the runner asks again until ITS deadline."""
        member = {'id': 'onboarding:open', 'role': 'Button', 'name': 'Open Ducktape', 'state': [],
                  'actions': ['press'], 'in': 'onboarding'}
        door = fake_door([JOIN], wait_cap_ms=100)
        shown = threading.Timer(0.35, lambda: door_nodes(door).append(member))
        shown.start()
        step = {'kind': 'wait', 'say': 'the member node is ready', 'wait': {'name': 'Open Ducktape'},
                'deadline_ms': 3000}
        with mock.patch.object(walk, 'DOOR_WAIT_CAP_MS', 100):
            result = self.walk([step], door, fake_jev([]))
        self.assertEqual(result['result'], 'PASS')
        asked = [json.loads(c[2])['deadline_ms'] for c in door.calls if c[1] == '/wait']
        self.assertGreater(len(asked), 2, 'asked again after the cap')
        self.assertTrue(all(ms <= 100 for ms in asked), asked)

        began = time.monotonic()
        with mock.patch.object(walk, 'DOOR_WAIT_CAP_MS', 100):
            result = self.walk([dict(step, deadline_ms=450)], fake_door([JOIN], wait_cap_ms=100), fake_jev([]))
        self.assertEqual(result['result'], 'FAIL')
        self.assertGreaterEqual(time.monotonic() - began, 0.45, 'failed at the runner deadline, not the door cap')
        self.assertEqual(self.lines('transcript.jsonl')[0]['reason'], 'deadline passed')

    def test_a_suite_runs_scenarios_on_one_rig(self):
        folder = Path(tempfile.mkdtemp(prefix='walk-suite-'))
        self.addCleanup(shutil.rmtree, folder, True)
        rig = {'display': False}
        files = {
            'setup.json': {'name': 'setup', 'rig': rig, 'steps': [
                {'kind': 'wait', 'say': 'the join button shows', 'wait': {'name': 'Join'}, 'deadline_ms': 200}]},
            'a.json': {'name': 'a', 'include': ['setup.json'], 'steps': [{'say': 'open settings', 'expect': 'x'}]},
            'join.json': {'name': 'join', 'rig': rig, 'steps': [
                {'say': 'join with an invitation', 'expect': 'the invitation field shows'}]},
            'b.json': {'name': 'b', 'include': ['setup.json'], 'steps': [{'include': 'join.json'}]},
        }
        for name, value in files.items():
            (folder / name).write_text(json.dumps(value))
        scenarios = [walk.load(folder / 'a.json'), walk.load(folder / 'b.json')]
        judge = fake_jev([jev('choice', {'choice': 'none'}, 10), jev('choice', {'choice': 'o0'}, 20),
                          jev('noul', {'noul': 0.9}, 30)])
        result = self.walk(None, fake_door([JOIN]), judge, scenarios=scenarios)
        verdicts = {e['scenario']: e['result'] for e in result['scenarios']}
        self.assertEqual(verdicts, {'setup': 'PASS', 'a': 'FAIL', 'b': 'PASS'}, 'setup once, then a, then b')
        self.assertEqual(result['result'], 'FAIL')
        self.assertEqual([(l['scenario'], l['n']) for l in self.lines('transcript.jsonl')],
                         [('setup', 1), ('a', 1), ('b', 1)])
        failed = result['scenarios'][1]
        self.assertEqual(failed['failures'][0]['reason'], 'no offered action performs this step')
        self.assertEqual(failed['failures'][0]['offers'], ['onboarding:network-join press Button Join with invitation'])
        self.assertTrue(Path(failed['failures'][0]['tree']).exists())
        self.assertAlmostEqual(failed['usd'], 10 * walk.PRICE)
        self.assertAlmostEqual(result['scenarios'][2]['usd'], 50 * walk.PRICE)

        # a prelude that fails keeps every scenario that needs it from running
        door = fake_door([FIELD])
        result = self.walk(None, door, fake_jev([]), scenarios=scenarios)
        self.assertEqual([e['result'] for e in result['scenarios']], ['FAIL', 'SKIPPED', 'SKIPPED'])
        self.assertIn('needs setup', result['scenarios'][1]['reason'])
        self.assertEqual(result['result'], 'FAIL')

        (folder / 'loop.json').write_text(json.dumps({'name': 'loop', 'include': ['loop.json'], 'steps': []}))
        self.assertRaises(walk.Refused, walk.load, folder / 'loop.json')

    def test_keyboard_mode_offers_only_keys(self):
        steps = [{'say': 'join with an invitation', 'expect': 'the invitation field shows'},
                 {'say': 'paste the invitation', 'secret': 'invite'}]
        secret = self.out.parent / f'{self.out.name}-kbd'
        secret.write_text('INVITE-by-keys\n')
        self.addCleanup(secret.unlink)
        door = fake_door([FIELD, JOIN])
        judge = fake_jev([jev('choice', {'choice': 'o0'}), jev('choice', {'choice': 'o0'}),
                          jev('choice', {'choice': 'o6'}), jev('noul', {'noul': 0.9}),
                          jev('choice', {'choice': 'o0'}), jev('choice', {'choice': 'o11'})])
        result = self.walk(steps, door, judge, secrets={'invite': {'file': str(secret)}}, keyboard=True)
        self.assertEqual(result['result'], 'PASS', self.lines('transcript.jsonl'))
        self.assertEqual(result['mode'], 'keyboard')
        self.assertFalse([c for c in door.calls if c[1] == '/act'], 'a keyboard walk never acts on a node')
        keys = [json.loads(c[2]) for c in door.calls if c[1] == '/key']
        self.assertEqual([(k['keys'], k['text']) for k in keys],
                         [('tab', ''), ('tab', ''), ('enter', ''), ('tab', ''), ('', 'INVITE-by-keys')])
        asked = [json.loads(c[2])['questions']['q'] for c in judge.calls]
        first = asked[0]['criteria']
        self.assertEqual(first['o0'], 'press tab: move focus')
        self.assertIn('press ctrl-k: shell::OpenPalette', first.values())
        self.assertFalse([v for v in first.values() if 'network-join' in v], 'no node is offered to act on')
        self.assertFalse([v for v in first.values() if v.startswith('type')], 'no typing while no field has focus')
        self.assertTrue(asked[-1]['criteria']['o11'].startswith("type the step's text into the focused PasswordInput"))
        self.assertEqual(self.lines('transcript.jsonl')[0]['keys'], ['tab', 'tab', 'enter'])
        self.assertNotIn('INVITE-by-keys', ''.join(c[2] for c in judge.calls))

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
