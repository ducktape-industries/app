# QA regression suite (`ops/qa/walk.py`)

The runner drives the app only through its test door (the accessibility tree, #114) and asks Jev to pick among the
door's closed offer list and to judge each step against the tree. Plain Python 3 stdlib; its format and rules are in
the root README ("QA walk") and in `walk.py`'s docstring.

**Only the runner drives the app.** During a walk nobody clicks, types, runs `ducktape-app ax …` or starts an agent
shell against the rig: a hand on the app makes the transcript lie. Workers write and unit-test scenarios; the manager
runs walks.

## Run it on a merge

One command, one params file (copy `params.example.json`; it holds paths, never the secrets themselves):

```sh
JEV_API_KEY=… python3 ops/qa/walk.py ops/qa/scenarios/suite/*.json \
    --out ~/qa/<sha> --params ~/qa/params.json
python3 ops/qa/walk.py ops/qa/scenarios/suite/*.json --keyboard \
    --out ~/qa/<sha>-keys --params ~/qa/params-keys.json   # keyboard only: a second rig, a second invite
```

- Every suite scenario includes `scenarios/setup.json` (install the release, join by invite, member node, wallet,
  console open). In one run setup runs once, then each surface in turn on the same rig. A surface starts from an open
  console and leaves it open.
- A failing surface is recorded and the next one still runs (after two Escapes, so a dialog it left does not block
  the next). If setup does not pass, every surface is `SKIPPED`: nothing runs without a console.
- setup is WRITE-ONLY on the network: it spends the single-use invite and runs a member node. Each run needs a
  fresh invite and a fresh `--out`.
- `walk_tag` (for example the merge's short sha) names what the walk makes (the message, page, file and board), so a
  later run can tell its own from an earlier one's.
- Exit: 0 PASS, 1 FAIL, 2 FAIL-UNJUDGED (a judge or door error, or a skipped surface), 3 bad scenario or command line.

## Output

Each invocation writes a new `<out>/run-NN/`; an earlier run's files are never overwritten. `<out>/rig` is kept.

- `result.json`: `result`, `mode` (pointer | keyboard), `usd`, and per scenario `result` (PASS, FAIL,
  FAIL-UNJUDGED, SKIPPED), `steps_passed/total`, `usd`, and `failures`. Each failure lists `step`, `say`,
  `verdict`, `chosen`, `keys` (keyboard), `noul`, `reason` (the judge's score against the expectation, or "no offered
  action performs this step"), `offers` (the options the judge saw) and `tree` (the failing-tree file).
- `transcript.jsonl` (one line per step, tagged with its scenario), `ledger.jsonl` (one line per Jev call),
  `failing-tree-<scenario>-<step>.json`.

To file findings grouped by surface:

```sh
jq '.scenarios[] | select(.result != "PASS") | {scenario, result, failures: [.failures[] | {step, say, reason, tree}]}' \
    ~/qa/<sha>/run-01/result.json
```

## Cost

Jev charges $0.042 per million input tokens. Each Jev call sends the compact tree (and in a choice the offers). A
pointer `ui` step makes 2 calls, or 3 when it retries. A keyboard step makes one call per key pressed, plus a judge
call after each activation. The exact cost of a run is `usd` in `result.json` and one line per call in
`ledger.jsonl`. As a rough guide, not a measurement: a pointer suite (about 60 `ui` steps) comes to a few cents, and
a keyboard suite to several times that.

## Scenarios

- `setup.json`: the shared prelude. `reopen.json`: quit, reopen, unlock, console again (a staged release flips here).
- `suite/*.json`: one file per surface: agents, boards, chat, explorer, files, forge, governance, huddle, inbox,
  members, node, pages, palette, settings. Each expectation reads names, roles, values and states from the tree,
  never pixels; a status note is not an error.
- `registry-views.json`, `seq4-acceptance.json`, `update-path.json`: release walks on the same prelude.
  `update-path.json` also takes `next_version` and `next_sequence`. `launch-window.json` needs no network.
- Format additions: a top-level `include: [file]` names preludes (run once per rig, before the scenario). A step
  `{"include": file}` stands for that file's steps. Paths are relative to the including file.
- `gaps` (documentation; the runner ignores it) lists what the tree cannot address or show on that surface: an
  unnamed control, a missing role, content drawn as a surface, a hover-only control. These are findings. A scenario
  does not work around them, so it does not walk those flows. `jq -r '.name as $n | .gaps[] | "\($n): \(.)"'
  ops/qa/scenarios/suite/*.json` prints them all.

## Keyboard-only mode (`--keyboard`)

The same scenario files, but a `ui` step never acts on a node. The judge picks one key at a time from what a keyboard
user has:

- Tab, Shift-Tab and the arrows move focus.
- Enter and Space activate the focused node.
- Escape and Backspace.
- "Type the step's text", offered only while a text field holds focus.
- The named shortcuts the door reports: GPUI bindings reachable from focus, and the chords the seated views claim
  (Ctrl-K for the palette, for example).

It presses until the step's expectation holds, or fails after `keys` presses (40 unless the step sets it). The tree's
`focused` state says where focus is. `private_copy` presses Tab until its field has focus, then types. Keys go to the
door's `POST /key`, which feeds the app's own key dispatch (key down, then key up), never the OS; `GET /keys` lists
the shortcuts. Both have the same loopback and token gate as every other endpoint.

A window focuses its own handle when it opens, so the first Tab lands on the first control. Tab from nowhere goes
nowhere: gpui-component binds `tab` in its `Root` context.
