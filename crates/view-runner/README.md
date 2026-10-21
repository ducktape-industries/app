# view-runner

The contract between the host and whatever runs a view's code: wasmtime (the desktop and Android, `crates/host/src/runner/wasmtime.rs`), JavaScriptCore (iOS, `crates/host-jsc`) and the page's own WebAssembly (the browser, `crates/host-web`). It names no runtime and no window, so a runner depends on this crate and not on the host. `ducktape_host::contract` re-exports every item, and the shell hands its runner to the host in `Boot::runner`.

## The runner (`ViewRunner`)

One per process; the shell picks it.

- `prepare(hash, code, view_bytes)` compiles (wasmtime) or validates, meters and keeps (JSC, the page) `code`, whose sha256 is `hash`, for a view whose linear memory may reach `view_bytes` (the `memory.view_bytes` setting). Nothing runs. The code stays prepared while the answer's hold (`Prepared::hold`, opaque to the host) is held: whatever may still seat it holds the hold, and a running instance holds its own code. A runner may free code nothing holds. `CodeSource` says where the code came from: still held (`Memory`), the runner's disk cache (`Disk`), or done now (`Cold`).
- `instantiate(hash, seat, land)` seats the prepared code `hash` names. It lands `Landed::Seated` (or a `Fault`) on `land`, which the instance keeps for every later answer. Code nothing holds any more lands `Fault::Runner`.
- `describe(hash, module, op)` is `program.describe`: it runs the import-free describe module `module` on `op`, once `view_meter::accepts` takes it as it would a view, and answers `None` on any failure. It answers a future (`rt::Fut`) that borrows nothing: a runner copies what it needs of `module` before it returns, does its blocking part (wasmtime's compile, JSC's meter pass) on `rt::task::blocking`, and awaits a page's answer rather than block a thread on it.
- `code_memory()` is the compiled code the runner holds (`HeldCode`): by hash, the bytes mapped and how many of them are resident.
- `Code` is code a runner prepares: its bytes, and where they lie in a file when they are a file's (a section of a program's blob the cache keeps). A runner that compiles apart reads them there itself, so they never pass through this process's heap.
- `prepare` refuses with `Failure`: `Unsupported` names the operator or feature the runner does not run, the same answer on every runner; `Runner` is the runner's own failure. A view built against another wire is not refused: it runs, and whatever fails names both wires.

Threads: `prepare` runs on a loader and may block it (wasmtime compiles; JSC and the page meter; none of them touches a page). `describe` is called on the kernel's I/O loop and never blocks it; its future is awaited there. `instantiate` and every `begin_*` are called only on the seat's thread (the window thread), which awaits each answer as it lands on the seat's `Landing`, at once or later. That covers a first load, a pane claiming a compiled seat, the developer's override and `Preload::Seat`'s instance (a loader prepares the code and parks it; the window thread seats and inits it), a swap's replacement, a sibling after a restore trap, and a revive.

## An instance (`ViewInstance`)

One running view: the five exports (`alloc` and `memory` stay inside the runner). It is `Send` so the seat registry may hold it. Every entry is called on the seat's thread and is completion-shaped: it returns at once, and the answer lands on the seat's `Landing`.

- `begin_init` lands `Inited`; `begin_tick(events)` lands `Frame` (the bytes as the module wrote them: a view's frame, or a notifier's turn, which the host reads, never the runner); `begin_snapshot` lands `Snapshot` or `SnapshotRefused`; `begin_restore` lands `Restored` or `RestoreRefused`. Any of them may land a `Fault` instead.
- `Fault::Trap` is the trap's words, `Fault::Panicked` the words the view's panic hook (`ducktape_view.panicked`) handed over before the trap, `Fault::Memory` a `memory.grow` or `table.grow` past the `memory.view_bytes` setting, and `Fault::Runner` the runner's own failure (a refused script, a dead page before the answer). Every runner produces every variant but `Runner`.
- `fuel_used` is the fuel the last landed call used, `memory_bytes` the view's linear memory, and `memory_resident` how much of it is resident where the runner can say.
- A program's notifier is seated the same way, on the same code path and budget: a module that exports `alloc`, `init` and `tick` only. Every runner seats a module without `snapshot` and `restore`, and a `begin_snapshot` or `begin_restore` on it lands `Fault::Trap("the view exports no …")`. Its seat's thread is the notifier's own, never the window thread.
- One call is in flight per instance: the host begins a call only once the answer to the one before it landed, so each answer is the answer to the last call begun. Each call lands exactly one `Landed`, once.

Dead pages (SPEC §6.5; the host relies on every rule here):

- Every call into a dead page lands `Fault::Runner`, never nothing: the call in flight when the page died, and every call begun on the instance after, until it is dropped. A call that lands nothing leaves its seat waiting for good.
- `instantiate` right after a death seats the view on a new page: the host's revive asks at once, and a refusal there (a page still reloading) stops the view. A runner that needs time takes it before it lands `Seated`.
- The host revives each seat itself (`View::revive`: a new instance, restored from the kept snapshot, the kept batches replayed). A runner never restores or replays a seat on its own.
- A dead instance keeps holding its code until it is dropped: a revive seats the same code by hash while the dead instance is still the view's.

Ending: the host calls `stop` on an instance it replaces while it lives (a revive's dead one). Every other instance is dropped, and dropping one stops it. Drop runs on whichever thread drops it and must not wait on the seat's thread, which may be waiting on a lock the dropping thread holds: the seat's thread (a swap's install), a loader's (an install replacing the view, a `Loaded::Empty` install, both under the seat's lock), or the roster thread's (a program's `retire`, under the registry's and the seat's locks). So a JSC page handle is posted to the main thread to be dropped there, never dropped through it.

`SeatId` names which seat an instance is for: the program, and the pane's layout instance (0 for a seat no pane held when it was seated). During a swap or a revive two live instances share one id, so it is for a runner's own logs and bookkeeping, never a key two instances may not share.

`Landing` is a seat's answer channel, `Send + Clone`: the seat makes it when it asks for an instance and the instance keeps it, and a runner `send`s each answer into it from wherever the answer appears. An answer after the seat is gone is dropped.

`test-support` (on for the host's own tests) adds `memory_for_test` and `memory_mut_for_test`, which only the wasmtime instance lends.
