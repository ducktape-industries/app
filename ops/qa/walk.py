#!/usr/bin/env python3
"""Walk the app through its accessibility door (#114) with a typed-question judge.

usage: walk.py <scenario.json> --out <dir> [--param k=v ...] [--keep-going]

A scenario is {name, params?, rig, steps}. A `ui` step asks Jev (TypeSafe System
One) a `choice` over the door's closed action list, acts, then asks a `noul` on
the delta and the tree. `wait`, `shell`, `launch` and `stop` steps make no model
call. Plain Python 3 stdlib; see README "QA walk" for the format and the rules.

The Jev key comes from env JEV_API_KEY and goes nowhere but the request header.
Rig secrets are resolved at run time and are written only as `{secret:<name>}`.
Every child runs in its own session; teardown signals only the process groups
this run recorded, and only while /proc/<pid>/exe still says they are ours.

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
KINDS = ('ui', 'wait', 'shell', 'launch', 'stop')
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

    def wait(self, spec, deadline_ms):
        body = {k: spec[k] for k in ('role', 'name', 'state', 'in', 'gone') if k in spec}
        return self.call('POST', '/wait', dict(body, deadline_ms=deadline_ms), timeout=deadline_ms / 1000 + 10)


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
    def __init__(self, scenario, out, params, keep_going=False):
        self.scenario, self.out, self.keep_going = scenario, Path(out), keep_going
        missing = sorted({m.group(2) for m in HOLE.finditer(json.dumps(scenario))
                          if m.group(1) == 'param' and m.group(2) not in params})
        if missing:
            raise Refused('missing --param ' + ', '.join(missing))
        for n, step in enumerate(scenario['steps'], 1):
            if step.get('kind', 'ui') not in KINDS:
                raise Refused(f'step {n}: kind must be one of {KINDS}')
        self.out.mkdir(parents=True, exist_ok=True)
        for name in ('transcript.jsonl', 'ledger.jsonl', 'failing-tree.json', 'result.json'):
            (self.out / name).unlink(missing_ok=True)
        self.rig = Rig(self.out / 'rig', scenario.get('rig') or {}, params)
        unknown = sorted({m.group(2) for m in HOLE.finditer(json.dumps(scenario))
                          if m.group(1) == 'secret' and m.group(2) not in self.rig.secrets})
        unknown += [s['secret'] for s in scenario['steps'] if s.get('secret') and s['secret'] not in self.rig.secrets]
        if unknown:
            self.rig.teardown()
            raise Refused('undeclared rig secret(s): ' + ', '.join(sorted(set(unknown))))
        self.door = Door(self.rig.door_file())
        self.jev = Jev(self.out / 'ledger.jsonl', self.rig.redact)
        self.last_tree = None

    def write(self, name, value, mode='w'):
        with open(self.out / name, mode) as handle:
            handle.write(self.rig.redact(json.dumps(value, ensure_ascii=False)) + '\n')

    def run(self):
        steps, passed, verdicts = self.scenario['steps'], 0, []
        try:
            for n, step in enumerate(steps, 1):
                began, line = time.monotonic(), {'n': n, 'kind': step.get('kind', 'ui'), 'say': step['say']}
                self.last_tree = None
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
                else:
                    if self.last_tree is not None and not (self.out / 'failing-tree.json').exists():
                        self.write('failing-tree.json', {'step': n, 'say': step['say'], 'tree': self.last_tree})
                    if not self.keep_going:
                        break
        finally:
            stopped = self.rig.teardown()
            for log in self.rig.paths['logs'].iterdir():  # children wrote these unfiltered
                log.write_text(self.rig.redact(log.read_text(errors='replace')))
        result = ('FAIL' if 'fail' in verdicts else 'FAIL-UNJUDGED' if 'unjudged' in verdicts
                  else 'PASS' if passed == len(steps) else 'FAIL-UNJUDGED')
        value = {'scenario': self.scenario.get('name'), 'result': result, 'steps_passed': passed,
                 'steps_total': len(steps), 'usd': round(self.jev.usd, 8), 'teardown': stopped}
        self.write('result.json', value)
        return value

    def step_ui(self, step, line):
        tree, offers = self.door.tree(), self.door.actions()
        self.last_tree = tree
        options = [f"{o['id']} {o['action']} {o['label']}" for o in offers]
        criteria = {f'o{i}': text for i, text in enumerate(options)}
        criteria['none'] = 'none of these'
        line['options'] = len(options)
        answer = self.jev.ask('choice', {'step': step['say'], 'screen': tree}, {
            'type': 'choice',
            'instructions': (TREE + 'Which ONE option performs this step: '
                             f'"{step["say"]}"? Each option is an element id, the action, then the element\'s role '
                             'and name. Answer `none` if no option performs it.'),
            'criteria': criteria})
        chosen = answer.get('choice') if isinstance(answer, dict) else None
        if not isinstance(chosen, str) or chosen not in criteria:
            raise Unjudged(f'the judge answered {str(chosen)[:40]!r}, not an option')
        if chosen == 'none':
            line['chosen'], line['reason'] = 'none', 'no offered action performs this step'
            return 'fail'
        offer = offers[int(chosen[1:])]
        line['chosen'] = options[int(chosen[1:])]
        value = None
        if offer['action'] in ('set_value', 'type'):
            if 'text' in step:
                value = self.rig.fill(step['text'])
            elif step.get('secret'):
                value, line['value'] = self.rig.secrets[step['secret']], '{secret:%s}' % step['secret']
            else:
                raise Unjudged(f"{offer['action']} chosen but the step gives no text or secret")
        delta = self.door.act(offer['id'], offer['action'], value)
        after = self.door.tree()
        score = self.judge(step, line['chosen'], delta, after)
        if score < PASS_AT:
            settle = step.get('settle_ms', 1500)
            if step.get('wait'):
                self.door.wait(step['wait'], settle)
            else:
                time.sleep(settle / 1000)
            later = self.door.tree()
            delta, after = diff(tree, later), later
            score = self.judge(step, line['chosen'], delta, after)
            line['retried'] = True
        self.last_tree = after
        line['delta'], line['noul'] = summary(delta), score
        return 'pass' if score >= PASS_AT else 'fail'

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

    def step_stop(self, step, line):
        if self.rig.app is None:
            raise Unjudged('no app was launched')
        line['stopped'] = self.rig.stop(self.rig.app)
        return 'pass' if line['stopped'] in ('stopped', 'exited') else 'fail'


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    parser.add_argument('scenario')
    parser.add_argument('--out', required=True)
    parser.add_argument('--param', action='append', default=[], metavar='K=V')
    parser.add_argument('--keep-going', action='store_true')
    args = parser.parse_args(argv)
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(143))  # so teardown runs
    try:
        params = dict(p.split('=', 1) for p in args.param)
        walk = Walk(json.loads(Path(args.scenario).read_text()), args.out, params, args.keep_going)
    except (Refused, ValueError, OSError, KeyError) as error:
        print(f'walk: {error}', file=sys.stderr)
        return 3
    result = walk.run()
    print(json.dumps({k: result[k] for k in ('result', 'steps_passed', 'steps_total', 'usd')}))
    return {'PASS': 0, 'FAIL': 1}.get(result['result'], 2)


if __name__ == '__main__':
    sys.exit(main())
