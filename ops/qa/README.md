# QA regression suite (`ops/qa`)

The walk runner, its params example and the cross-repo acceptance scenarios (setup, reopen, registry-views,
seq4-acceptance, update-path, launch-window) live in the private repo
[ducktape-industries/ducktape-qa](https://github.com/ducktape-industries/ducktape-qa): its README has the format, the
output, the cost and keyboard mode. They moved there from this directory at 8f74e92a. The app runs them at a pinned
rev.

What stays here:

- `scenarios/suite/*.json`: one file per surface (agents, boards, chat, explorer, files, forge, governance, huddle,
  inbox, members, node, pages, palette, settings). Each expectation reads names, roles, values and states from the
  tree, never pixels; a status note is not an error. `gaps` lists what the tree cannot address or show on that
  surface (findings; the scenario does not work around them): `jq -r '.name as $n | .gaps[] | "\($n): \(.)"'
  ops/qa/scenarios/suite/*.json`.
- `RUNNER_REV`: the full sha of ducktape-qa this app's suite runs on. A bump is a PR here.
- `run.sh`: checks out ducktape-qa at exactly `RUNNER_REV` under `${XDG_CACHE_HOME:-~/.cache}/ducktape-qa/<rev>`
  (a clone and a detached checkout; never a pull), refuses a checkout that is dirty or at another rev, then runs its
  `walk.py` with your arguments. The clone needs read access to the private repo.
- `FINDINGS-2026-09-19.md`: the first suite run's findings.
- The test door, the `ax` CLI and the tree contract (`ax_contract`) are app code: see the root README.

**Only the runner drives the app.** During a walk nobody clicks, types, runs `ducktape-app ax …` or starts an agent
shell against the rig. Workers write and check scenarios; the manager runs walks.

## Gate

```sh
ops/qa/run.sh --check    # every suite scenario loads through the pinned runner's walk.load; non-zero on a refusal
```

## Run the suite on a merge

```sh
JEV_API_KEY=… ops/qa/run.sh ops/qa/scenarios/suite/*.json --out ~/qa/<sha> --params ~/qa/params.json
ops/qa/run.sh ops/qa/scenarios/suite/*.json --keyboard \
    --out ~/qa/<sha>-keys --params ~/qa/params-keys.json   # keyboard only: a second rig, a second invite
```

Every suite scenario includes `setup.json` (install the release, join by invite, member node, wallet, console open).
It is not next to them, so the runner takes it from its own `scenarios/acceptance/`: the prelude is the one at the
pinned rev. setup runs once, then each surface in turn on the same rig; it spends the invite, so each run needs a
fresh invite and a fresh `--out`.
