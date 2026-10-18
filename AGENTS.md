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

- Accepted work merges into `dev`, on duckpond's forge (repository `app`)
  or on GitHub, and forge-sync keeps the two in step: a push to `dev` or
  `main` on one side moves the same branch on the other.
- When both sides moved at once and diverged, forge wins: GitHub's branch is
  reset to forge's, and the dropped commits are posted in duckpond's #sync.
