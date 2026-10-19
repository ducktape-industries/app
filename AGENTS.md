# Repository Instructions

Guidance for assistant sessions in this repository; `CLAUDE.md` links here.

## No Invented Restrictions (an invented limit is an invented cap)

- An operation is refused only for a reason the user asked for or that
  correctness forces: a signature that does not verify, a key or account that
  does not exist, acting for an account the signer does not control, or an
  account that is suspended or revoked.
- Never limit an operation to one kind of account (a user's, an agent, a
  program's own account) or to one role, unless the user asked for that limit
  by name. The same holds for a UI that hides an action from some kinds of
  account.
- A limit like this found in existing code is deleted in the same change. It
  is never carried forward, documented as a rule, or made configurable: no
  setting or flag that "lifts" it.
- A design that seems to need a new restriction is a question for the user
  before it is built.

## Delivery

- This repository lives on duckpond's forge as `app`, and forge's `dev`
  and `main` are its branches of record. GitHub's
  `ducktape-industries/app` is a read-only release mirror: forge-mirror
  fast-forwards its `dev` and `main` to forge's, its Actions run CI and build
  releases from those commits, and a ruleset refuses every other write to its
  `dev` and `main`. Nobody opens or merges a pull request there. Issues stay
  on GitHub.
- Forge checks every change too: forge actions runs
  `.forge/workflows/gates.toml` (fmt, `tools/dep-gate.sh` and clippy
  `-D warnings`, with and without `ax-door`; no tests, which wait for a
  release cut) in a pods pod on each change's Preview, the squash its merge
  would land on `dev`'s head, and on each push to `dev` and `main`. Its
  result is the commit's forge status `gates.toml#gates`; a red push is
  posted to #ci.
- Nobody pushes to forge's `dev` or `main`. Every piece of work, from anyone,
  lands through a forge change: push the work as a branch, open a change from
  that branch into `dev`, and merge the change. The merge's result is ONE
  squash commit on the current `dev`, with a single parent; its subject is the
  change's title followed by `(change N)`, and its body says what changed,
  why, and the gates run. No merge commits. Forge cannot force-update a
  branch and keeps every branch pushed to it, so when `dev` has moved and the
  branch no longer merges cleanly, rebase it onto `forge/dev`, push it as a
  new branch and open a new change.
- Duckpond's agents open changes with their harness tools. Operator
  workers use `forge-change.sh` in the operator workspace
  (`/mnt/data/code/qa/.local/duckpond/operator`), run from a worktree forked
  from `forge/dev`: `forge-change.sh open app SLUG TITLE BODY_FILE` pushes
  HEAD as `operator/SLUG` and opens its change into `dev`, the title and body
  being the squash commit's subject and body; `forge-change.sh merge app N`
  builds the squash commit on the current `dev`, publishes it as
  `operator/SLUG-merge`, merges change N and waits until forge confirms the
  merge.
- Who merges a change: a change from duckpond's agents is reviewed and
  merged only by chief (account 17), and tech workers never merge one.
  Operator workers merge their own changes. eddy (account 18) merges his own
  changes, and the changes brought over from his GitHub pull requests are his
  to land; chief merges one of them only if eddy agrees.
- `main` moves only on an explicit, user-requested release of `dev`: a forge
  change from `dev` into `main`.
- A checkout reaches forge through a remote `forge`, with git-remote-duck
  (ducktape release `git-remote-duck-7acf453`) on `PATH`:
  `git remote add forge duck://duckpond-725bcad1/forge/app` and
  `git config duck.duckpond-725bcad1.node https://duckpond.ducktape.industries`.
  `git fetch forge` needs nothing more. A push signs with a git key on a
  duckpond account: operator workers set
  `DUCKTAPE_HOME=/mnt/data/code/qa/.local/duckpond/operator DUCKTAPE_DEVICE_KEY_STORE=file`
  (`forge-change.sh` sets both itself), the workspace holding account 22's
  "operator git" key.
