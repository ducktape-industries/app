#!/usr/bin/env python3
"""Walk the app through its accessibility door (#114) with a typed-question judge.

usage: walk.py <scenario.json> [<scenario.json> ...] --out <dir> [--params file.json]
               [--param k=v ...] [--keep-going] [--keyboard]

A scenario is {name, include?, params?, rig, steps}. A `ui` step asks Jev
(TypeSafe System One) a `choice` over the door's closed action list, acts, then
asks a `noul` on the delta and the tree. `wait`, `shell`, `launch`, `stop` and
the `private_*` steps make no model call. Several scenarios run on ONE rig, in
turn: a scenario's `include`d preludes run once before it, a failing scenario is
recorded and the next still runs (one that needs a prelude that did not pass is
skipped). `--keyboard` offers only what a keyboard has: move focus, activate the
focused node, type into the focused field, Escape, named shortcuts. Plain Python
3 stdlib; see ops/qa/README.md for the format and the rules.

The Jev key comes from env JEV_API_KEY and goes nowhere but the request header.
Rig secrets are resolved at run time and are written only as `{secret:<name>}`.
Private text the door reveals (rig.private only) is held in memory and goes
nowhere but back into a secure input of the app: no file, no request to Jev.
Every child runs in its own session; teardown signals only the process groups
this run recorded, and only while /proc/<pid>/exe still says they are ours.

Every invocation writes a new <out>/run-NN/ (transcript, ledger, result, failing
trees); <out>/rig is kept, so a later run on the same --out starts on that rig.

Exit: 0 PASS, 1 FAIL, 2 FAIL-UNJUDGED, 3 the scenario or the command line is bad.
"""
import argparse
import http.client
import json
import os
import re
import secrets as token_source
import shutil
import signal
import subprocess
import sys
import time
import urllib.request
from pathlib import Path

JEV_URL = 'https://api.typesafe.ai/v1/systemone'
PRICE = 0.042 / 1e6  # USD per Jev input token
PASS_AT = 0.7
KINDS = ('ui', 'wait', 'shell', 'launch', 'stop', 'private_remember', 'private_copy')
DOOR_WAIT_CAP_MS = 60_000  # the door answers one /wait within this at most (ax_door.rs); longer = ask again
KEY_BUDGET = 40  # keys one keyboard step may press before it fails
MOVES = ('tab', 'shift-tab', 'up', 'down', 'left', 'right')
KEYS = {'enter': 'activate the focused node', 'space': 'activate or toggle the focused node',
        'escape': 'close or cancel', 'backspace': 'delete the character before the caret'}
SHORTCUT = re.compile(r'^(ctrl|alt|super|cmd|platform|function|fn)-|^f\d+$')  # a named shortcut, not typing
TEXT_ROLES = ('TextInput', 'MultilineTextInput', 'SearchInput', 'EmailInput', 'NumberInput', 'PasswordInput',
              'PhoneNumberInput', 'UrlInput')
# How the door writes a tree (PR #120), so the judge reads state by the door's rule.
TREE = ('`screen` is the app\'s accessibility tree as a flat list of nodes {id, role, name, value?, state[], '
        'actions[], in}. `state` lists flags such as disabled, focused, selected, checked, expanded: a control '
        'whose state does not list disabled is enabled. A private value reads •••. ')
HOLE = re.compile(r'\{(param|rig|secret):([A-Za-z0-9_]+)\}')


class Unjudged(Exception):
    """The judge or the door could not answer: the step has no verdict."""


class Refused(Exception):
    """The scenario or the command line cannot be walked."""


def diff(before, after):
    """What changed between two compact trees, by id (the door's own rule)."""
    old = {node['id']: node for node in before}
    new = {node['id'] for node in after}
    return {'appeared': [n for n in after if n['id'] not in old],
            'disappeared': [n['id'] for n in before if n['id'] not in new],
            'changed': [n for n in after if n['id'] in old and old[n['id']] != n]}


def summary(delta):
    ids = lambda nodes: [n['id'] if isinstance(n, dict) else n for n in nodes][:8]
    return {k: {'n': len(v), 'ids': ids(v)} for k, v in delta.items()}


def matches(spec, node):
    """A {role, name, in, ids_prefix} matcher, by the door's wait rule: role exact
    (any case), name a substring (any case), `in` a window or its view."""
    scope = spec.get('in')
    return ((not spec.get('role') or spec['role'].lower() == node['role'].lower())
            and spec.get('name', '').lower() in node['name'].lower()
            and (not scope or node['in'] == scope or node['in'].startswith(scope + '/'))
            and node['id'].startswith(spec.get('ids_prefix', '')))


def find(spec, tree):
    """The id `spec` names: an id as is, else the first matching node's; None if none."""
    if isinstance(spec, str):
        return spec
    return next((node['id'] for node in tree if matches(spec, node)), None)


def merge_rig(specs):
    """One rig for every scenario of a run: a display or reveal if any asks,
    secrets and env by name (the same name must mean the same source)."""
    rig = {'display': any(s.get('display', True) for s in specs),
           'private': any(s.get('private') for s in specs), 'secrets': {}, 'env': {}}
    for spec in specs:
        for key in ('secrets', 'env'):
            for name, value in (spec.get(key) or {}).items():
                if rig[key].setdefault(name, value) != value:
                    raise Refused(f'rig {key} {name}: two scenarios define it differently')
    return rig


def load(path, _within=()):
    """A scenario file with its includes resolved, relative to the file: the
    top-level `include` names preludes (each runs once per rig, before it), a
    step `{"include": file}` stands for that file's steps and rig."""
    path = Path(path).resolve()
    if path in _within:
        raise Refused(f'{path.name} includes itself')
    scenario = json.loads(path.read_text())
    scenario['preludes'] = [load(path.parent / name, _within + (path,)) for name in scenario.get('include', [])]
    steps = []
    for step in scenario['steps']:
        if 'include' not in step:
            steps.append(step)
            continue
        inner = load(path.parent / step['include'], _within + (path,))
        steps += inner['steps']
        if 'rig' in inner:
            scenario['rig'] = merge_rig([scenario.get('rig') or {}, inner['rig']])
    scenario['steps'] = steps
    return scenario


def start_time(pid):
    """Field 22 of /proc/<pid>/stat: with the pid, names one process for good."""
    stat = Path(f'/proc/{pid}/stat').read_text()
    return int(stat.rsplit(')', 1)[1].split()[19])


class Rig:
    """Private HOME/XDG/DUCKTAPE_HOME under <out>/rig, an own Xvfb, recorded children."""

    def __init__(self, root, spec, params):
        self.root = Path(root).resolve()
        self.paths = {name: self.root / sub for name, sub in (
            ('dir', '.'), ('home', 'home'), ('config', 'xdg/config'), ('data', 'xdg/data'),
            ('cache', 'xdg/cache'), ('state', 'xdg/state'), ('run', 'xdg/run'),
            ('ducktape_home', 'ducktape'), ('bin', 'bin'), ('logs', 'logs'))}
        for path in self.paths.values():
            path.mkdir(parents=True, exist_ok=True)
        self.paths['run'].chmod(0o700)
        self.params = params
        self.records, self.procs, self.app = [], {}, None
        self.secrets = {}
        for name, source in (spec.get('secrets') or {}).items():
            if 'file' in source:
                self.secrets[name] = Path(self.fill(source['file'])).read_text().strip()
            elif source.get('generate') == 'password':
                self.secrets[name] = token_source.token_urlsafe(18)
            else:
                raise Refused(f'secret {name}: give "file" or "generate": "password"')
        self.display = None
        self.env = {
            'PATH': '/usr/bin:/bin', 'LANG': 'C.UTF-8', 'HOME': str(self.paths['home']),
            'XDG_CONFIG_HOME': str(self.paths['config']), 'XDG_DATA_HOME': str(self.paths['data']),
            'XDG_CACHE_HOME': str(self.paths['cache']), 'XDG_STATE_HOME': str(self.paths['state']),
            'XDG_RUNTIME_DIR': str(self.paths['run']), 'DUCKTAPE_HOME': str(self.paths['ducktape_home']),
            'DUCKTAPE_AX_DOOR': '0', 'XMODIFIERS': '@im=none',
            'DBUS_SESSION_BUS_ADDRESS': f"unix:path={self.paths['run']}/no-bus"}
        if spec.get('private'):  # the door's `reveal`, for the private_* steps
            self.env['DUCKTAPE_AX_DOOR_PRIVATE'] = '1'
        self.env.update({k: self.fill(v) for k, v in (spec.get('env') or {}).items()})
        if spec.get('display', True):
            self.start_display()

    def fill(self, text, secrets=False):
        """Params and rig paths always; secret values only when `secrets`."""
        def one(match):
            kind, name = match.groups()
            if kind == 'param':
                return self.params[name]
            if kind == 'rig':
                return str(self.paths[name])
            return self.secrets[name] if secrets else match.group(0)
        return HOLE.sub(one, text)

    def redact(self, text):
        for name, value in self.secrets.items():
            if value:
                for form in (value, json.dumps(value)[1:-1]):
                    text = text.replace(form, '{secret:%s}' % name)
        return text

    def start_display(self):
        taken = lambda n: Path(f'/tmp/.X11-unix/X{n}').exists() or Path(f'/tmp/.X{n}-lock').exists()
        number = next((n for n in range(240, 400) if not taken(n)), None)
        if number is None:
            raise Refused('no free X display in :240-:399')
        xvfb = shutil.which('Xvfb', path=self.env['PATH'])
        if not xvfb:
            raise Refused('Xvfb is not installed')
        self.xvfb = self.spawn('xvfb', [xvfb, f':{number}', '-screen', '0', '1280x800x24', '-nolisten', 'tcp'])
        socket = Path(f'/tmp/.X11-unix/X{number}')
        for _ in range(100):
            if socket.exists():
                break
            time.sleep(0.1)
        else:
            self.stop(self.xvfb[0])
            raise Refused(f'Xvfb :{number} did not come up')
        self.display = f':{number}'
        self.env['DISPLAY'] = self.display

    def spawn(self, name, argv, capture=False):
        """Starts argv in a new session under the rig env and records pid, exe and start time."""
        log = open(self.paths['logs'] / f'{name}.log', 'ab') if not capture else None
        proc = subprocess.Popen(argv, env=self.env, cwd=self.root, stdin=subprocess.DEVNULL,
                                stdout=subprocess.PIPE if capture else log, stderr=subprocess.STDOUT,
                                start_new_session=True)
        if log:
            log.close()
        record = {'name': name, 'pid': proc.pid, 'exe': os.path.realpath(f'/proc/{proc.pid}/exe'),
                  'start': start_time(proc.pid)}
        self.records.append(record)
        self.procs[proc.pid] = proc
        (self.root / 'pids.json').write_text(json.dumps(self.records, indent=1))
        return record, proc

    def ours(self, record):
        """True only for a live process this run started: same pid and start time,
        its exe under the rig (the launcher execs into the rig's install) or,
        for the display, still the Xvfb binary it was started as."""
        pid = record['pid']
        try:
            exe = os.path.realpath(os.readlink(f'/proc/{pid}/exe'))
            same = start_time(pid) == record['start'] and os.getpgid(pid) == pid
        except OSError:
            return False
        inside = exe == str(self.root) or exe.startswith(str(self.root) + os.sep)
        xvfb = record['name'] == 'xvfb' and exe == record['exe']
        return same and (inside or xvfb)

    def stop(self, record, patience=15):
        """SIGTERM to the recorded process group, then wait. Never a harder signal."""
        proc = self.procs.get(record['pid'])
        if proc is not None and proc.poll() is not None:
            return 'exited'
        if not self.ours(record):
            return 'not ours: left alone'
        try:
            os.killpg(record['pid'], signal.SIGTERM)
        except ProcessLookupError:
            return 'exited'
        try:
            if proc is not None:
                proc.wait(timeout=patience)
            return 'stopped'
        except subprocess.TimeoutExpired:
            return 'still running after SIGTERM'

    def teardown(self):
        return {f"{r['name']}:{r['pid']}": self.stop(r) for r in reversed(self.records)}

    def door_file(self):
        return self.paths['run'] / 'ducktape' / 'ax-door.json'


class Door:
    """The app's loopback test door (PR #120): GET /tree, /actions; POST /act, /wait."""

    def __init__(self, path):
        self.path = path

    def call(self, method, target, body=None, timeout=30):
        try:
            door = json.loads(Path(self.path).read_text())
            conn = http.client.HTTPConnection('127.0.0.1', int(door['port']), timeout=timeout)
            conn.request(method, target, body=None if body is None else json.dumps(body),
                         headers={'Authorization': 'Bearer ' + door['token'], 'Content-Type': 'application/json'})
            reply = conn.getresponse()
            status, data = reply.status, reply.read()
            conn.close()
            return status, json.loads(data or b'null')
        except (OSError, ValueError, KeyError, http.client.HTTPException) as error:
            raise Unjudged(f'door: {type(error).__name__}: {error}') from None

    def must(self, method, target, body=None, timeout=30):
        status, value = self.call(method, target, body, timeout)
        if status != 200:
            raise Unjudged(f'door {method} {target}: {status} {json.dumps(value)[:200]}')
        return value

    def tree(self):
        return self.must('GET', '/tree?compact=1')

    def actions(self):
        return self.must('GET', '/actions')

    def act(self, node, action, value=None):
        return self.must('POST', '/act', {'id': node, 'action': action, 'value': value})

    def reveal(self, node):
        """The on-screen text of one private node. The caller keeps it in memory only."""
        value = self.must('POST', '/reveal', {'id': node})
        return value.get('value') or value.get('name') or ''

    def wait(self, spec, deadline_ms):
        """The door answers one wait within DOOR_WAIT_CAP_MS: ask again until OUR deadline."""
        body = {k: spec[k] for k in ('role', 'name', 'state', 'in', 'gone') if k in spec}
        end = time.monotonic() + deadline_ms / 1000
        while True:
            ask = min(max(0, round((end - time.monotonic()) * 1000)), DOOR_WAIT_CAP_MS)
            status, value = self.call('POST', '/wait', dict(body, deadline_ms=ask), timeout=ask / 1000 + 10)
            if status != 408 or time.monotonic() >= end:
                return status, value

    def key(self, keys='', text=''):
        """Keys (`tab`, `shift-tab`, `enter`, `ctrl-k`) then text, through the app's own key dispatch."""
        return self.must('POST', '/key', {'keys': keys, 'text': text})

    def shortcuts(self):
        return self.must('GET', '/keys')


class Jev:
    """One typed question per call; every call leaves a ledger line."""

    def __init__(self, ledger, redact):
        self.ledger, self.redact, self.usd = ledger, redact, 0.0

    def ask(self, kind, state, question):
        key = os.environ.get('JEV_API_KEY', '')
        tokens, error = 0, None
        try:
            if not key:
                raise Unjudged('JEV_API_KEY is not set')
            body = self.redact(json.dumps({'state': state, 'model': 'jev-latest', 'questions': {'q': question}}))
            request = urllib.request.Request(JEV_URL, data=body.encode(), headers={
                'Authorization': 'Bearer ' + key, 'Content-Type': 'application/json', 'User-Agent': 'ducktape-qa-walk/1'})
            with urllib.request.urlopen(request, timeout=40) as reply:
                value = json.load(reply)
            tokens = int(value['usage']['input_tokens'])
            return value['answers']['q']
        except Unjudged as failure:
            error = str(failure)
            raise
        except Exception as failure:  # fail closed: any transport or shape error has no verdict
            error = f'{type(failure).__name__}: {str(failure)[:160]}'
            raise Unjudged(f'jev {kind}: {error}') from None
        finally:
            usd = tokens * PRICE
            self.usd += usd
            line = {'ts': round(time.time(), 3), 'type': kind, 'input_tokens': tokens, 'usd': usd}
            if error:
                line['error'] = error
            with open(self.ledger, 'a') as handle:
                handle.write(self.redact(json.dumps(line)) + '\n')


class Walk:
    def __init__(self, scenarios, out, params, keep_going=False, keyboard=False):
        self.out, self.keep_going, self.keyboard = Path(out), keep_going, keyboard
        # every scenario after the preludes it includes, each once: (scenario, the preludes it needs)
        self.plan, named = [], {}

        def add(scenario):
            needs = []
            for prelude in scenario.get('preludes', []):
                needs += add(prelude) + [prelude['name']]
            name = scenario['name']
            if name not in named:
                named[name] = scenario['steps']
                self.plan.append((scenario, list(dict.fromkeys(needs))))
            elif named[name] != scenario['steps']:
                raise Refused(f'two different scenarios are named {name}')
            return needs

        for scenario in scenarios if isinstance(scenarios, list) else [scenarios]:
            add(scenario)
        everything = json.dumps([s for s, _ in self.plan])
        missing = sorted({m.group(2) for m in HOLE.finditer(everything)
                          if m.group(1) == 'param' and m.group(2) not in params})
        if missing:
            raise Refused('missing --param ' + ', '.join(missing))
        spec = merge_rig([s['rig'] for s, _ in self.plan if 'rig' in s] or [{}])
        for scenario, _ in self.plan:
            for n, step in enumerate(scenario['steps'], 1):
                at = f"{scenario['name']} step {n}"
                if step.get('kind', 'ui') not in KINDS:
                    raise Refused(f'{at}: kind must be one of {KINDS}')
                if step.get('kind', '').startswith('private_') and not spec['private']:
                    raise Refused(f'{at}: a private step needs "rig": {{"private": true}}')
                if step.get('kind') == 'private_copy' and step.get('how', 'type') not in ('type', 'set_value'):
                    raise Refused(f'{at}: how must be "type" or "set_value"')
        self.out.mkdir(parents=True, exist_ok=True)
        runs = [int(d.name[4:]) for d in self.out.glob('run-*') if d.name[4:].isdigit()]
        self.run_dir = self.out / f'run-{max(runs, default=0) + 1:02d}'
        self.run_dir.mkdir()
        self.rig = Rig(self.out / 'rig', spec, params)
        unknown = sorted({m.group(2) for m in HOLE.finditer(everything)
                          if m.group(1) == 'secret' and m.group(2) not in self.rig.secrets})
        unknown += [step['secret'] for s, _ in self.plan for step in s['steps']
                    if step.get('secret') and step['secret'] not in self.rig.secrets]
        if unknown:
            self.rig.teardown()
            raise Refused('undeclared rig secret(s): ' + ', '.join(sorted(set(unknown))))
        self.door = Door(self.rig.door_file())
        self.jev = Jev(self.run_dir / 'ledger.jsonl', self.rig.redact)
        self.last_tree = self.offers = None
        self.memory = {}  # private_remember: never written, never sent

    def write(self, name, value, mode='w'):
        with open(self.run_dir / name, mode) as handle:
            handle.write(self.rig.redact(json.dumps(value, ensure_ascii=False)) + '\n')

    def run(self):
        results, done = [], {}
        try:
            for scenario, needs in self.plan:
                blocked = [name for name in needs if done.get(name) != 'PASS']
                if blocked:
                    entry = {'scenario': scenario['name'], 'result': 'SKIPPED', 'steps_passed': 0,
                             'steps_total': len(scenario['steps']), 'usd': 0.0, 'failures': [],
                             'reason': 'needs ' + ', '.join(blocked) + ' to pass first'}
                else:
                    entry = self.run_one(scenario)
                    if entry['result'] != 'PASS' and self.rig.app is not None:
                        try:  # leave no dialog of a failed scenario over the next one's start
                            self.door.key('escape escape')
                        except Unjudged:
                            pass
                done[scenario['name']] = entry['result']
                results.append(entry)
        finally:
            stopped = self.rig.teardown()
            for log in self.rig.paths['logs'].iterdir():  # children wrote these unfiltered
                log.write_text(self.rig.redact(log.read_text(errors='replace')))
        verdicts = [entry['result'] for entry in results]
        value = {'run': str(self.run_dir), 'mode': 'keyboard' if self.keyboard else 'pointer',
                 'result': ('FAIL' if 'FAIL' in verdicts else 'PASS' if set(verdicts) == {'PASS'}
                            else 'FAIL-UNJUDGED'),
                 'steps_passed': sum(entry['steps_passed'] for entry in results),
                 'steps_total': sum(entry['steps_total'] for entry in results),
                 'usd': round(self.jev.usd, 8), 'scenarios': results, 'teardown': stopped}
        self.write('result.json', value)
        return value

    def run_one(self, scenario):
        name, steps, passed, verdicts, failures = scenario['name'], scenario['steps'], 0, [], []
        spent = self.jev.usd
        for n, step in enumerate(steps, 1):
            began = time.monotonic()
            # what Jev reads names the run's own things: params and rig paths, never a secret
            step = {**step, **{k: self.rig.fill(step[k]) for k in ('say', 'expect') if k in step}}
            line = {'scenario': name, 'n': n, 'kind': step.get('kind', 'ui'), 'say': step['say']}
            self.last_tree = self.offers = None
            try:
                verdict = getattr(self, 'step_' + line['kind'])(step, line)
            except Unjudged as error:
                verdict, line['reason'] = 'unjudged', str(error)
            except OSError as error:  # a step's program or file is not there
                verdict, line['reason'] = 'fail', f'{type(error).__name__}: {error}'
            line['verdict'], line['ms'] = verdict, round((time.monotonic() - began) * 1000)
            self.write('transcript.jsonl', line, 'a')
            verdicts.append(verdict)
            if verdict == 'pass':
                passed += 1
                continue
            failure = {'step': n, 'say': step['say'], 'verdict': verdict}
            failure.update({k: line[k] for k in ('chosen', 'keys', 'noul', 'reason') if k in line})
            if self.offers is not None:
                failure['offers'] = self.offers
            if self.last_tree is not None:
                tree = 'failing-tree-%s-%02d.json' % (re.sub(r'[^A-Za-z0-9_.-]', '-', name), n)
                self.write(tree, {'scenario': name, 'step': n, 'say': step['say'], 'tree': self.last_tree})
                failure['tree'] = str(self.run_dir / tree)
            failures.append(failure)
            if not self.keep_going:
                break
        result = ('FAIL' if 'fail' in verdicts else 'FAIL-UNJUDGED' if 'unjudged' in verdicts
                  else 'PASS' if passed == len(steps) else 'FAIL-UNJUDGED')
        return {'scenario': name, 'result': result, 'steps_passed': passed, 'steps_total': len(steps),
                'usd': round(self.jev.usd - spent, 8), 'failures': failures}

    def choose(self, step, state, instructions, options):
        """The judge's pick among `options` (or None for `none of these`), as an index."""
        criteria = {f'o{i}': text for i, text in enumerate(options)}
        criteria['none'] = 'none of these'
        self.offers = options
        answer = self.jev.ask('choice', state, {'type': 'choice', 'instructions': TREE + instructions,
                                                'criteria': criteria})
        chosen = answer.get('choice') if isinstance(answer, dict) else None
        if not isinstance(chosen, str) or chosen not in criteria:
            raise Unjudged(f'the judge answered {str(chosen)[:40]!r}, not an option')
        return None if chosen == 'none' else int(chosen[1:])

    def text_for(self, step, line, action):
        """What a fill types: the step's text or a rig secret, never the model's."""
        if 'text' in step:
            return self.rig.fill(step['text'])
        if step.get('secret'):
            line['value'] = '{secret:%s}' % step['secret']
            return self.rig.secrets[step['secret']]
        raise Unjudged(f'{action} chosen but the step gives no text or secret')

    def step_ui(self, step, line):
        if self.keyboard:
            return self.keyboard_ui(step, line)
        tree, offers = self.door.tree(), self.door.actions()
        self.last_tree = tree
        # A control that can be pressed or filled is never the answer as a bare `focus`;
        # offering both made the judge pick focus for "select"/"open" steps.
        acts = {o['id'] for o in offers if o['action'] != 'focus'}
        offers = [o for o in offers if o['action'] != 'focus' or o['id'] not in acts]
        options = [f"{o['id']} {o['action']} {o['label']}" for o in offers]
        line['options'] = len(options)
        chosen = self.choose(step, {'step': step['say'], 'screen': tree},
                             f'Which ONE option performs this step: "{step["say"]}"? Each option is an element id, '
                             'the action, then the element\'s role and name. Answer `none` if no option performs it.',
                             options)
        if chosen is None:
            line['chosen'], line['reason'] = 'none', 'no offered action performs this step'
            return 'fail'
        offer, line['chosen'] = offers[chosen], options[chosen]
        value = None
        if offer['action'] in ('set_value', 'type'):
            value = self.text_for(step, line, offer['action'])
        delta = self.door.act(offer['id'], offer['action'], value)
        after = self.door.tree()
        if not step.get('expect'):
            # Nothing observable to judge (a masked field was filled): the act landing is the verdict,
            # and the next step's expectation (an enabled button) proves the value took.
            self.last_tree, line['delta'] = after, summary(delta)
            return 'pass'
        return 'pass' if self.met(step, line, line['chosen'], tree, delta, after) else 'fail'

    def keyboard_ui(self, step, line):
        """Keyboard only: one key at a time from what a keyboard user has — move
        focus, activate the focused node, type into the focused field, Escape, a
        named shortcut — until the step's expectation holds or KEY_BUDGET runs out."""
        before, pressed = self.door.tree(), []
        line['keys'] = pressed
        for _ in range(step.get('keys', KEY_BUDGET)):
            tree = self.last_tree = self.door.tree()
            shortcuts = [s for s in self.door.shortcuts() if SHORTCUT.search(s['keys'])]  # they follow focus
            focused = next((n for n in tree if 'focused' in n['state']), None)
            offers = [(key, f'press {key}: move focus') for key in MOVES]
            offers += [(key, f'press {key}: {what}') for key, what in KEYS.items()]
            offers += [(s['keys'], f"press {s['keys']}: {s['action']}") for s in shortcuts
                       if s['keys'] not in KEYS]
            if focused and focused['role'] in TEXT_ROLES and ('text' in step or step.get('secret')):
                offers.append((None, f"type the step's text into the focused {focused['role']} {focused['name']}"))
            chosen = self.choose(
                step, {'step': step['say'], 'focused': focused, 'pressed': pressed, 'screen': tree},
                'The user works the app with the KEYBOARD ONLY. `focused` is the node that holds keyboard focus '
                '(null: none yet) and `pressed` the keys already pressed for this step. Which ONE key performs '
                f'this step now, or moves focus toward the control that performs it: "{step["say"]}"? Tab and '
                'Shift-Tab move focus through the controls in order, arrows move within a list or group, Enter '
                'and Space activate the focused node. Answer `none` if no key can.',
                [label for _, label in offers])
            if chosen is None:
                line['chosen'], line['reason'] = 'none', 'no key performs this step'
                return 'fail'
            key, line['chosen'] = offers[chosen]
            if key is None:
                self.door.key(text=self.text_for(step, line, 'type'))
                pressed.append('type')
            else:
                self.door.key(key)
                pressed.append(key)
            if key in MOVES:
                continue
            after = self.door.tree()
            if not step.get('expect'):
                if key is None:  # a fill without expect passes on the act, as with a pointer
                    self.last_tree, line['delta'] = after, summary(diff(before, after))
                    return 'pass'
                continue
            if self.met(step, line, line['chosen'], before, diff(before, after), after):
                return 'pass'
        line['reason'] = f"not done within {len(pressed)} keys" + (f"; {line['reason']}" if 'reason' in line else '')
        return 'fail'

    def met(self, step, line, chosen, before, delta, after):
        """The judge's noul on `expect`, once more after `settle_ms` when under PASS_AT."""
        score = self.judge(step, chosen, delta, after)
        if score < PASS_AT:
            settle = step.get('settle_ms', 1500)
            if step.get('wait'):
                self.door.wait(step['wait'], settle)
            else:
                time.sleep(settle / 1000)
            later = self.door.tree()
            delta, after = diff(before, later), later
            score = self.judge(step, chosen, delta, after)
            line['retried'] = True
        self.last_tree = after
        line['delta'], line['noul'] = summary(delta), score
        if score < PASS_AT:
            line['reason'] = f"the judge scored {score:.2f} (< {PASS_AT}) for: {step['expect']}"
        else:
            line.pop('reason', None)
        return score >= PASS_AT

    def judge(self, step, chosen, delta, tree):
        answer = self.jev.ask('noul', {'step': step['say'], 'action': chosen, 'delta': delta, 'screen': tree}, {
            'type': 'noul',
            'instructions': (TREE + 'The user performed `step` with `action`; `delta` is what changed in the tree '
                             f'(appeared, disappeared, changed nodes) and `screen` is the tree now. '
                             f'Is this expectation met: "{step["expect"]}"?'),
            'criteria': {'true': f'The screen now shows: {step["expect"]}',
                         'false': 'The screen does not show that, or shows an error instead'}})
        try:
            return float(answer['noul'])
        except (TypeError, KeyError, ValueError):
            raise Unjudged('the judge gave no noul') from None

    def step_wait(self, step, line):
        status, value = self.door.wait(step['wait'], step.get('deadline_ms', 5000))
        if status == 200:
            return 'pass'
        if status == 408:
            self.last_tree = value.get('tree')
            line['reason'] = 'deadline passed'
            return 'fail'
        raise Unjudged(f'door wait: {status}')

    def step_shell(self, step, line):
        argv = [self.rig.fill(a, secrets=True) for a in step['argv']]
        line['argv'] = step['argv']
        name = f"step-{line['n']:02d}"
        if step.get('background'):
            record, proc = self.rig.spawn(name, argv)
            time.sleep(step.get('settle_ms', 3000) / 1000)
            line['pid'] = record['pid']
            if proc.poll() is None:
                return 'pass'
            line['reason'] = f'exited {proc.returncode}'
            return 'fail'
        _, proc = self.rig.spawn(name, argv, capture=True)
        try:
            output = proc.communicate(timeout=step.get('timeout_ms', 120000) / 1000)[0].decode('utf-8', 'replace')
        except subprocess.TimeoutExpired:
            line['reason'] = 'timed out; left to teardown'
            return 'fail'
        (self.rig.paths['logs'] / f'{name}.log').write_text(self.rig.redact(output))
        line['exit'], line['tail'] = proc.returncode, self.rig.redact(output[-200:])
        pattern = step.get('expect_output')
        if pattern is not None:  # a param in a pattern is literal text (a version's `+`)
            pattern = HOLE.sub(lambda m: re.escape(self.rig.fill(m.group(0))), pattern)
        good = proc.returncode == step.get('expect_exit', 0) and (
            pattern is None or re.search(pattern, output, re.M) is not None)
        return 'pass' if good else 'fail'

    def step_launch(self, step, line):
        argv = [self.rig.fill(a, secrets=True) for a in step['argv']]
        exe = Path(argv[0]).resolve()
        if not str(exe).startswith(str(self.rig.root) + os.sep):
            # teardown signals only an exe under the rig: run a link to it from there
            local = self.rig.paths['bin'] / exe.name
            local.unlink(missing_ok=True)
            try:
                os.link(exe, local)
            except OSError:
                shutil.copy2(exe, local)
            argv[0] = str(local)
        line['argv'], line['display'] = step['argv'], self.rig.display
        self.rig.door_file().unlink(missing_ok=True)
        record, proc = self.rig.spawn('app', argv)
        self.rig.app = record
        line['pid'] = record['pid']
        deadline = time.monotonic() + step.get('deadline_ms', 60000) / 1000
        while time.monotonic() < deadline:
            if proc.poll() is not None:
                line['reason'] = f'the app exited {proc.returncode}'
                return 'fail'
            if self.rig.door_file().exists():
                try:
                    if self.door.tree():
                        return 'pass'
                except Unjudged:
                    pass
            time.sleep(0.25)
        line['reason'] = 'the door did not answer by the deadline'
        return 'fail'

    def step_private_remember(self, step, line):
        """Reveals every node `match` names and holds its text under `as`, keyed by
        the number that ends its id (`phrase-word/7` → 7)."""
        tree = self.last_tree = self.door.tree()
        held = {}
        for node in (n for n in tree if matches(step['match'], n)):
            number = re.search(r'(\d+)$', node['id'])
            if not number:
                raise Unjudged(f"{node['id']}: no number ends the id to remember it by")
            held[int(number.group(1))] = self.door.reveal(node['id'])
        self.memory[step['as']] = held
        line['remembered'] = f"{step['as']}: {len(held)} items"
        if not held:
            line['reason'] = 'no node matched'
            return 'fail'
        return 'pass'

    def step_private_copy(self, step, line):
        """Puts private text into a secure input: revealed from `from` (ids or
        matchers), or with `from_indexed` the remembered items whose numbers the
        visible prompt asks for, in its order. Only how much is recorded."""
        tree = self.last_tree = self.door.tree()
        into = find(step['into'], tree)
        target = next((n for n in tree if n['id'] == into), None)
        if target is None or target['role'] != 'PasswordInput':
            # anywhere else the door would show the text, and the judge would read it
            line['reason'] = f'into {into}: not a showing secure input'
            return 'fail'
        if 'from_indexed' in step:
            spec = step['from_indexed']
            # A plain label carries its text as the VALUE (its name is empty), so read both.
            prompt = ' '.join(f"{n.get('name') or ''} {n.get('value') or ''}" for n in tree
                              if n['role'] in ('Label', 'Heading') and n.get('in') == spec['prompt'].get('in', n.get('in')))
            asked = [int(k) for m in re.finditer(spec['regex'], prompt)
                     for k in re.findall(r'\d+', m.group(1 if m.re.groups else 0))]
            held = self.memory.get(spec['memory'], {})
            missing = [k for k in asked if k not in held]
            if not asked or missing:
                line['reason'] = f"asked {asked}; not remembered in {spec['memory']}: {missing}"
                return 'fail'
            texts, source = [held[k] for k in asked], f"{spec['memory']} #{','.join(map(str, asked))}"
        else:
            ids = [find(s, tree) for s in (step['from'] if isinstance(step['from'], list) else [step['from']])]
            if None in ids:
                line['reason'] = 'from: a matcher found no node'
                return 'fail'
            texts, source = [self.door.reveal(i) for i in ids], ','.join(ids)
        text = ' '.join(texts)
        if not self.keyboard:
            self.door.act(into, step.get('how', 'type'), text)
        elif self.tab_to(into):
            self.door.key(text=text)
        else:
            line['reason'] = f'into {into}: Tab never puts focus on it'
            return 'fail'
        line['copied'] = f'private_copy from {source} into {into}: {len(text)} chars'
        return 'pass'

    def tab_to(self, target):
        """Presses Tab (no model call) until `target` holds focus; False once focus comes round again."""
        seen = []
        while True:
            now = next((n['id'] for n in self.door.tree() if 'focused' in n['state']), None)
            if now == target:
                return True
            if now in seen or len(seen) > 500:
                return False
            seen.append(now)
            self.door.key('tab')

    def step_stop(self, step, line):
        if self.rig.app is None:
            raise Unjudged('no app was launched')
        line['stopped'] = self.rig.stop(self.rig.app)
        return 'pass' if line['stopped'] in ('stopped', 'exited') else 'fail'


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    parser.add_argument('scenarios', nargs='+', metavar='scenario')
    parser.add_argument('--out', required=True)
    parser.add_argument('--params', metavar='FILE', help='a JSON object of params; --param overrides')
    parser.add_argument('--param', action='append', default=[], metavar='K=V')
    parser.add_argument('--keep-going', action='store_true', help='run the rest of a scenario after a failed step')
    parser.add_argument('--keyboard', action='store_true', help='drive every ui step by keys only')
    args = parser.parse_args(argv)
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(143))  # so teardown runs
    try:
        params = json.loads(Path(args.params).read_text()) if args.params else {}
        params.update(dict(p.split('=', 1) for p in args.param))
        walk = Walk([load(path) for path in args.scenarios], args.out, params, args.keep_going, args.keyboard)
    except (Refused, ValueError, OSError, KeyError) as error:
        print(f'walk: {error}', file=sys.stderr)
        return 3
    result = walk.run()
    print(json.dumps(dict({k: result[k] for k in ('result', 'steps_passed', 'steps_total', 'usd', 'run')},
                          scenarios={e['scenario']: e['result'] for e in result['scenarios']})))
    return {'PASS': 0, 'FAIL': 1}.get(result['result'], 2)


if __name__ == '__main__':
    sys.exit(main())
