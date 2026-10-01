# Performance tracking: views and the shell

A design for continuous performance tracking of (a) the wasm view modules
the app seats and (b) the app shell itself. One mechanism for both, cheap
when off, gateable from qa through the AX door.

Code is referenced by file and symbol, never by line: the tree is about to
be split. Every claim about the code was checked against `origin/dev` at
d147bfab (#346) and re-checked against #347 (`193c9a21`, dev 38cecdee), the
window-switch fix whose measurements set the baselines in §4; a second,
adversarial pass re-read every symbol named here against d147bfab. Where a
claim comes from another session's measurement it says so.

---

## 1. Goals and non-goals

**Goals**

- One switch, `DUCKTAPE_PERF=1`. Off, every hook is one relaxed atomic load;
  no `Instant::now`, no lock, no allocation. Users and qa run the same
  binary.
- One registry in the process, keyed by `(view, stage)` and `(window,
  stage)`, holding counters, gauges and small histograms.
- One tracing target, `ducktape::perf`, for the per-event stream. It goes
  through the app's one `fmt` file layer (`src/main.rs` `install_log`), so
  there is no second log format. The former `ducktape::fuel` target was
  renamed into it, with no alias.
- One door endpoint, `GET /perf`, on the AX door (`src/ax/http.rs`), same
  token, same loopback bind. It returns the registry as JSON and must not
  draw a window.
- One summary line in app.log, so a user's log tells the story without the
  door.
- Deep dives through wasmtime's own profiler hooks, dev-only, switched on by
  environment, no rebuild.

**Non-goals, and the dependency question**

- **No new crate in the core mechanism.** The registry is stdlib: a
  `Mutex<BTreeMap>` of fixed-size sample rings. `serde_json` is already a
  dependency for the door's replies.
- **`hdrhistogram` via `gpui-kit/profiler` is the one dependency, and only
  behind the `perf-deep` feature** (owner, 2026-09-28; §7). The gpui-pre
  fork carries a frame profiler (`WindowProfiler`, the foreground journal,
  `HangDetector`) behind its `profiler` cargo feature; `gpui-kit` 0.6.1
  exposes it as `profiler = ["gpui/profiler"]`, and the app's
  `perf-deep = ["gpui-kit/profiler"]` turns it on for dev and qa builds.
  A default build compiles none of it: `hdrhistogram` is in `Cargo.lock`
  and absent from `cargo tree -e normal` without the feature. Turning it on
  is not zero-cost when off: the journal allocates its ring at `App` build,
  and the draw, input and task-poll paths take timestamps whenever the
  feature is compiled in. It is the only way to get present time and
  gpui-level long-frame causes.
- **Not measured from inside the guest.** A view has no clock: its only wasm
  import is the panic hook the host binds in `Guest::instantiate`
  (`src/runtime/guest/lifecycle.rs`), and `clock.ticks` items carry an empty
  payload (`kernel::ticked`, `src/runtime/kernel.rs`). Every timing here is
  host-side. Guest-side counters could travel through the `host.log` request
  at one request per tick; fuel, frame bytes and the `Frame` fields already
  cover the continuous signal, so this design does not ask for it.
- **Not a per-frame line in app.log.** The file writer is a `Mutex<File>`
  written by whichever thread logs; a line per tick per view would be a
  write syscall per frame on the window thread.
- **No tracy, puffin, `profiling` backend or `--cfg ztracing`.** The fork
  has `#[profiling::function]` on draw, present and dispatch, with no backend
  in the lock; `profile-with-tracing` would also turn wgpu's scopes into INFO
  spans the app's `info` default lets through.
- **Not gating on qa's own timings.** qa records a per-step `ms` and a
  per-run `wall_time_s`; both include judge time and door round-trips. They
  are not app performance.

---

## 2. Stages

The "held" column named the session that had each file in flight when
this was designed (`70` ducktape-70, `a3` ducktape-a3); both have merged,
and phases 0 and 1 landed over every row (§5). Metric types: **C**
counter, **H** histogram (µs unless said), **G** gauge. Class **D** is
deterministic for the same inputs and can be gated; **W** is wall clock
and is reported only under Xvfb.

### 2.1 One view's life

Threads: a load runs on one of the loader threads (`seat::queue`); the tick
loop runs on the window thread inside `Seat::turn`
(`src/runtime/seat/entity.rs`), off the draw path, which holds the seat mutex while it runs.

| Stage | Measured today | Hook point | Metric | Held |
|---|---|---|---|---|
| Roster read (per block) | nothing | `roster::spawn_roster_read` around `backend::views::programs` | H `roster` (W) | free |
| Fetch the blob | `LoadTiming.fetch` (`seat.rs`), logged by `LoadTiming::log` as `fetch_ms`; disk-cache hit and node read are not told apart | `Guest::load` (`guest/lifecycle.rs`) around `backend::views::view_of`; source from `backend::views::program_bytes` (cache path vs `client.blob`) | H `fetch` (W), C `fetch.source{disk,node}` (D), G `view_bytes` (D) | a3 for the wall time in `Guest::load`; free for source and bytes in `program_bytes` (`backend/views.rs`) |
| Compile: memory hit / disk-cache hit / cold | `LoadTiming.compile` as `compile_ms`, one number for all three | `Guest::compile` → `guest::compiled_view` (owns the static `ViewCodeCache`) → `guest::compile_view`: in-process cache hit before `Module::new`; disk vs cold from a `wasmtime::Cache::cache_hits`/`cache_misses` delta around `Module::new`. `guest::engine` moves its `Cache` into `config.cache(..)`; `Cache` is `Clone`, so keep a clone beside the `Engine` to read the counters. They are process-global and loaders compile concurrently on purpose, so the delta is approximate — `ponytail:` note | H `compile` (W), C `compile.source{memory,disk,cold}` (D) | a3 (`guest.rs`) |
| Instantiate | lumped into `init_ms` | `Guest::instantiate` (`guest/lifecycle.rs`): `Linker::instantiate` plus `Exports::bind`; fuel is armed there too, so `FUEL_PER_TICK - get_fuel()` after it is the start function's fuel | H `instantiate` (W), H `fuel.instantiate` (D) | a3 |
| Snapshot of the old view (swap only) | lumped into `init_ms`, and runs under the seat mutex the window thread needs | `Guest::load` where it calls `old.snapshot()` (`Guest::snapshot` → `Exports::snapshot`) | H `snapshot` (W), G `snapshot_bytes` (D), H `fuel.snapshot` (D) | a3 |
| Restore | lumped into `init_ms` | `Guest::restore` → `Exports::restore` | H `restore` (W), H `fuel.restore` (D) | a3 |
| Init | `init_ms` = instantiate + snapshot + restore + init together | `Guest::init` → `Exports::init` | H `init` (W), H `fuel.init` (D) | a3 |
| First frame of a swap | `first_frame_ms` | `Guest::first_frame` | H `first_frame` (W) | a3 |
| Install | nothing: `LoadTiming::log` runs before the loader takes the seat lock in `Load::land` | `seat::Load::land`, after `Guest::load` returns: lock wait, then the match on `Loaded` through `locked.wake.send_replace(())`; a swap also drops the old `Guest` (its `Store`, up to `MEMORY_LIMIT` of memory) under the lock | H `install.lock_wait`, H `install` (W) | free |
| First tree of a fresh view | nothing. A fresh load only calls `init`; the first `tick` runs on the window thread at the first redraw, after `view_load` was logged. A preloaded seat (`roster::read_roster`) is only compiled: the pane that claims it loads it again, and that load instantiates and `init`s it, so a first open pays those two; "from load start" is still not what a user feels | end of the first `Guest::tick` (`Guest.ticks == 0` in `Guest::redraw`), measured from the start of the `Seat::turn` that ticks it | H `first_tree` (W, ms) | a3 / 70 |
| Tick: fuel | `tracing::debug!(target: "ducktape::perf", used, limit)` in `Guest::tick` (`guest/requests.rs`) | same site | H `fuel.tick` (D) | a3 |
| Tick: wall | nothing | `Guest::tick`: the `arm` → `Exports::tick` → `shape` chain is one expression today and has to be split to time the call and the decode apart | H `tick.call`, H `tick.decode` (W) | a3 |
| Requests per tick | nothing (only the `MAX_REQUESTS_PER_TICK` refusal) | `Guest::redraw`, the loop over `frame.requests` and `frame.cancels` | H `requests`, H `cancels` (D) | a3 |
| Host-call latency and attempts | nothing (a warn when the 60 s retry budget runs out, `node::until_answered`) | `kernel::node::spawn_call`: `Instant` before the spawned future, record when `replies.item` is called; attempts from `until_answered`; the kind is the `(Capability, operation)` pair `kernel::answer` matched on. Also `node::spawn_device` | H `host_call.<kind>` (W), C `host_call.<kind>.attempts` (D) | free |
| Reply backlog at drain | nothing | `Replies::drain_into` (`kernel/replies.rs`): `events.len()` before the append | H `backlog` (D) | free |
| Frame bytes | nothing (the `MAX_FRAME_BYTES` refusal in `guest::shape`) | `Exports::tick` answer length; `wire::encode(&events)` length in `Guest::tick` | H `frame_bytes`, H `events_bytes` (D) | a3 |
| Frame kind, busy, node count | nothing | after `guest::shape` in `Guest::tick`: `frame.root.is_some()`, `frame.patches.len()`, `frame.unchanged`, `frame.busy`; the node count from a `root.for_each_mut` walk (view-wire has no `count`), only when `frame_rev` bumps (O(n), so only when on) | C `frame.{full,patch,unchanged}`, C `busy_ticks`, G `nodes` (D) | a3 |
| Sanitize truncations | a `display_text_truncated` warn (`Guest::report_display_truncation`) | `reports.local` after `guest::shape` | C `truncations` (D) | a3 |
| Tree merge and replace | nothing | `guest::merge` (patch frames clone the held tree before applying); `Seat::turn`, the `fresh` branch: root clone (`Guest::drawn`; picture bytes are shared, not cloned), `native_root`, `ViewTree::replace` (`src/render/commands.rs`) | H `merge`, H `replace` (W) | a3 / 70 |
| ViewTree render | `ViewTree.renders`, `#[cfg(test)]` only (`src/render.rs`) | `ViewTree::render` (`Render` impl); promote the counter out of `cfg(test)` | C `renders` (D), H `render` (W) | 70 |
| Cache hit / miss per view | nothing | On the cached path (`layers::PaneView` draws the seat's tree as a cached `AnyView`, with or without a11y) gpui reuses the last prepaint when bounds, content mask and text style match, the entity is not dirty and the window is not refreshing (`gpui:src/view.rs`, the `AnyView` `prepaint` reuse branch); otherwise it calls `ViewTree::render` again. So `draws` = `PaneView` draws of the seat, `misses` = `ViewTree` renders, `hits = draws − misses`. Before #347 every draw was a miss | C `draws` (D); `misses`, `hits` derived | 70 |
| Full redraws per interaction | nothing (#347 measured it with temporary spans: 57–67 per window switch before, 11–14 after) | the `misses` delta between two door reads around the interaction | derived (D) | 70 |
| Refresh causes | nothing | every `window.refresh()` caller in `src` is a cache-buster for all cached views in that window (`gpui:src/view.rs`, `!window.refreshing`). Today there are two, both in `src/render/text.rs`: the selection path (post-#347 `refresh_on_change`, only when the shown selection changes; before it, gpui-base's `refresh_window_on_change`) and the drag mouse-up handler (`MouseUpEvent` while `DRAG_CLIP` is set). `Window::activate_a11y` also refreshes, and so does gpui when the input turns from the pointer to the keys or back (the focus ring follows it). A focus move does not: the pinned fork (gpui-pre#9) draws the view that drew the old focus and the one that draws the new, with their ancestors, so a view that reads at its render a focus handle drawn outside its own subtree (neither by it nor by a view under it) stays stale unless it observes focus (`on_focus`/`on_blur`); no view in `src` draws from such a read (audited 2026-10-01). The two views' draw is held by `shell::layers::root_tests::a_focus_move_re_renders_two_layers_not_the_window`. Count per site | C `refresh.<site>` (D) | free (`text.rs`) |
| gpui layout / paint per view | nothing. gpui's own histograms are behind the fork's `profiler` feature (§1) | `input::Observe` (`src/runtime/input.rs`) wraps the guest element: `Observe::prepaint` runs render + layout + prepaint for a cached `AnyView`, `Observe::paint` the paint. Per view only on the cached path, which the tree takes with or without a11y | H `layout`, H `paint` (W) | a3 |
| Pictures | nothing | `Pictures::adopt` (`src/runtime/pictures.rs`) keeps a running byte total, held under `MAX_PICTURE_BYTES` (64 MiB) by eviction | G `picture_bytes` (D) | free |
| Linear memory | nothing (the `MEMORY_LIMIT` trap) | `Exports.memory.data_size(&store)` after `Guest::tick` | G `memory` max (D) | a3 |
| Snapshot on the way out | nothing | `Guest::snapshot` (covered above) | | a3 |
| Faults | warns `module_view_trapped` (`Guest::tick`), `module_view_unloadable` (`Mounted::load_failed`) | same sites | C `faults` (D) | a3 / free |

Two findings the table depends on:

- **There is no time budget.** `guest::engine` sets only
  `cranelift_opt_level`, `consume_fuel(true)` and the disk cache; no
  `epoch_interruption`, `set_epoch_deadline` or `increment_epoch` appears in
  `src`. The only bound is `FUEL_PER_TICK`, re-armed by `guest::arm` before
  every call. The owner kept it that way (§7), and the `src/runtime.rs`
  module doc now says so.
- **`LoadTiming` had already lost `status` and `check`** by the time phase
  0 landed; the split above is what it holds now (`seat.rs`).

### 2.2 The shell

Threads: the entities' methods and their timers, all rendering, view
ticks and the door's answers run on the gpui foreground thread
(`src/shell/entities/`; `ax::serve` in `src/ax.rs`). I/O is
driven by the `views-kernel` tokio thread (`kernel::handle`), but the
entities' `Task` futures are polled on the foreground under
`runtime.enter()` (`spawn_on_runtime`), so response bodies are decoded
there.

| Stage | Measured today | Hook point | Metric | Held |
|---|---|---|---|---|
| Startup milestones | nothing | t0 at the top of `main` (`src/main.rs`); `log` after `install_log`; `gpui` at the top of the `application.run` closure in `shell::launch::run`; `fonts` after `initialize_rendering`; `boot` once the app's notification centre (its log read off disk) and roster are in hand, before the entities are made; `window` at `Ok(handle)` in `Windows::open_window` (`shell/entities/windows.rs`); `connected` in `Session::connect_answered` (`shell/entities/session.rs`); `desk` where `Windows` sees `Screen` cross from the launcher to the desk (`shell/entities/windows.rs`); `first_seated.<module>` in `Seat::mount`. No first-present mark without the gpui profiler | G `startup.<mark>` ms since t0 (W) | free, except `frame` (70) |
| Frame time per window | nothing (`ZED_MEASUREMENTS=1` — `gpui_util::measure` in the fork's frame callback — prints `frame duration` to stderr for callback-driven frames only, no rebuild) | `WindowRoot::render` (`shell/layers/root.rs`) for the app's render phase; end-to-end draw+present needs the fork's `WindowProfiler` (§1, §7) | H `frame.render` per `WindowKey` (W) | free |
| Long frames > 16 ms with a cause | nothing | with only the app's spans: a ring of the last N spans over 1 ms, timestamped; a `frame.render` over budget is written to the ring with the spans that overlapped it (reducer domain, view module and stage, `turn`/`hide`/close). Draw/present/input causes need `HangDetector` over `cx.foreground_journal()` (fork, `profiler` feature) | C `long_frames`, ring `slow` (W) | free |
| Entity method per call | nothing | every entity method (`shell/entities/`) keyed by the domain the reducer's arms had (the reducer is gone, with its `dispatch` timer; the names stay, since qa's hang rule reads them, §4.2 rule 5): `Desk` as `reducer.pane`, `Windows`, `Toast` and `Prefs` as `reducer.desk`, `Notifications` as `reducer.notify`, `Overlays` as `reducer.overlay`, `Session` as `reducer.connect`, `Account` as `reducer.sign_in`. **Never key by the argument**: the entities' methods take passwords, codes and phrases. Calls arrive three ways — synchronously from input listeners (`pane_message`, keys, screens), from a frame's callback (`PaneLayer::shown`: the desk's size and its seed, measured in the pane layer's prepaint and called from `window.on_next_frame`, never from the draw), and from the entities' own tasks, observers and subscriptions | H `reducer.<domain>` (W) | free |
| Pane / window switch | nothing (ducktape-70's temporary `pane_message` mark and `pane_stage` span) | start at `WindowRoot::pane_message` / `open_view` (`shell/panes.rs`), `Windows::raise` (`shell/entities/windows.rs`) or `WindowRoot::new`'s activation observer (`shell/layers/root.rs`), each through `WindowRoot::start_switch`; end with `window.on_next_frame` scheduled from the `WindowRoot::render` that shows the switch. Next-frame callbacks run at the start of the frame after the switched one was presented, so this is an upper bound high by one frame interval | H `switch` (W, ms) | 70 (`panes.rs`), free (`entities/windows.rs`, `layers/root.rs`) |
| Animation frames | `a_frame_is_cheap` test in `shell/figure.rs` (under 15 ms at test opt-level 1) | `Spin::render` (`shell/spin.rs`): time `Figure::frame`, and the interval between renders. `Spin::run` paces with a background timer plus `cx.notify()`, never `request_animation_frame`, so gpui's present-interval histogram and its inactive-window throttle both miss it | H `figure.frame`, H `figure.interval` (W) | free |
| Idle frames per second | nothing (#347's PR body: idle drawing over 5 s at 979 ms before, 334 ms after; the ~20 idle frames/s per window and the pulse as its driver are ducktape-70's session notes, not in the PR) | count `WindowRoot::render` per window over a quiet interval (`renders`), and each layer's own render beside it (`renders.chrome`, `renders.launcher`, `renders.panes`, `renders.pane.<n>`, `renders.strip.<n>`, `renders.overlays`, `renders.toast`, `renders.dot`, `renders.empty`, `renders.help`: §5 names the view each counts); door draws (`ax::actions::current` forces one per read) counted separately. Known idle drivers: the status-dot pulse (`status_bar::pulse`, `shell/status_bar.rs`, an `Animation::repeat().with_max_fps(30.)` drawn by `layers::StatusDot`, an uncached root sibling over the bar's empty well, so a pulse draws the dot and the root and leaves the cached bar alone: `renders.dot` ≈ `renders`, `renders.chrome` flat), the node menu and the reconnecting bar; capped, gpui's animation element paces itself with a background timer and `cx.notify` on the view drawing it) — cheap since #347 but still ~20 frames/s while motion is on, each a `WindowRoot::render` whose cached layers hit; the Spin figure's 33 ms timer on an empty desk. The clocks no longer draw an idle window (next row). Why the root is not cached: a view skips its render only when it is itself `.cached()`, clean, and no cached ancestor is re-rendering; a cached view that renders again renders every cached view inside it again (`gpui:src/view.rs`, the cached miss path sets `window.refreshing`); and a program's tick dirties the root, its ancestor. So a cached root spared the pulse's frames would redraw every program's tree on every program's tick, the #347 class (tried 2026-09-29: with the window cached, three model notifies took a seated tree from 2 renders to 5). The root is therefore thin and uncached, and the caches are its siblings' (P5, P6) | C `renders.<window>`, C `renders.<layer>` per window, C `door_draws` (D) | free / a3 |
| The clocks draw only what they move | `a_still_chain_draws_no_frame` (`shell/layers/root_tests.rs`): the node's 2 s status poll lands the same height six times (once with no answer), and neither the root, nor any layer's `renders.*`, nor the pane's tree draws; no `shell.dispatch` key is recorded. Idle probe (2026-09-29, the brief's no-read recipe: Forge opened from Spotlight, cache on, a seeded stage at 1 s blocks, release builds, two runs each), window renders over 5 s: motion off 25 / 23 before, 3 / 3 after; motion on 121 / 122 before, 107 / 109 after (the pulse); dispatches 30 / 30 before, 14 / 14 after (the reducer, its beat and its `dispatch` timer are gone since) | No clock beats for the whole app: each timer belongs to the entity whose value it moves, and a layer draws only for the entities it observes. The node's 2 s status poll is `Session`'s own (`shell/entities/session.rs`, on the executor's timer): its answer writes `Chain` compare-before-notify, so a still chain draws nothing, a moved height draws only the layers reading `Chain`, and `heard` restamps without a notify (`an_unchanged_status_poll_notifies_nothing`, `a_moved_height_notifies_the_chain_and_not_the_session`, `heard_moves_without_a_notify`, `entities/session_tests.rs`). A toast takes itself down on its own 3.6 s timer (`Toast::show`, `shell/entities/toast.rs`): one notify up, one down, no tick between. An open node or bell menu ages its rows on its own 1 s timer (`layers::Chrome`), alive only while it is open. A roster read or a load that moved the rail's rows wakes `Rail` on `runtime::changes_channel`, and a seat wakes on its own `wake`: neither waits for a clock. Before, an idle connected app re-rendered each window 1 + 1000/300 + 0.5 ≈ 4.8 times a second from the clocks alone; now once or twice per moved height on a live chain (the poll and the account read it starts), and not at all on a still one. The `renders.<window>` counter above is the measure | | free |
| `rail()` per roster change | nothing | `Roster::rail` locks the roster, the registry and every seat mutex. `entities::Rail::refresh` calls it once per burst on `runtime::changes_channel` (a roster read that found the list changed, a load installed, a seat claimed, retried or dropped); the bar (`layers::Chrome`), the pane strip (`layers::Strip`, through `panes::label`), the empty panes (`layers::EmptyPane`) and the dialogs (`layers::OverlayLayer`) read the `Rail`'s rows, so an idle app calls it 0 times | H `rail` (W), C `rail.calls` (D) | free (`roster.rs`) |
| Synchronous I/O on the window thread | nothing | `backend::session::read_prefs`/`write_prefs`; `store::answer` and `notify::Center::flush`, timed where they run, on the blocking pool and off the window thread (`io.store.*` per request, one queue per view file; `io.notify_save` once per view redraw or person's action, not per post); `backend::RpcClient::new` (`backend/noded.rs`) builds a new `reqwest::Client` per call: once per connection (`Session::connect_answered`, shared by the views, the roster and the sign-in calls), and the status poll one per connect (`Session::status_source`) that it asks through | H `io.<site>` (W) | free |
| RSS | nothing | `libc::getrusage(RUSAGE_SELF).ru_maxrss` (libc is already a dependency): KiB on Linux, bytes on macOS; current RSS from `/proc/self/statm` on Linux only, skipped on macOS. Sampled by the same 1 s task that writes the summary | G `rss.peak`, G `rss.current` (bytes) | free |

Where the shell's biggest window-thread costs sit, for the record, all
verified: wasm ticks and frame decode (`Guest::tick`, `guest::shape`);
deep tree clones on every patch (`guest::merge`), change
(`Seat::turn`) and render (`ViewTree::render` clones `root`);
`rail()` once per roster change; `read_prefs` in render paths through
`notify::Settings::load` (the permission bar in `layers::PaneView`, Settings ›
Notifications); file I/O in
the entities' methods (`Prefs::set_*`) and guest requests. The `reducer.<domain>` and I/O histograms above
are what will show which of these matter.

---

## 3. The mechanism

### 3.1 The registry (`src/perf.rs`, stdlib only)

```rust
//! Performance counters, on with `DUCKTAPE_PERF=1`, read at `GET /perf`.
pub(crate) fn start()                            // first thing in `main`: t0, the switch, the sampler thread
pub(crate) fn on() -> bool                       // one relaxed atomic load

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Key {
    View { module: &'static str, instance: u64 }, // the seat key (`seat::mounted`)
    Window(crate::runtime::WindowKey),
    Shell,                                       // startup, reducer, io, rss
}

/// `None` when off: the caller never takes an `Instant`.
pub(crate) fn time(key: Key, stage: &'static str) -> Option<Timer>  // `Timer::started()` for intervals
pub(crate) fn count(key: Key, stage: &'static str, n: u64)
pub(crate) fn gauge(key: Key, stage: &'static str, value: u64) // keeps max and last
pub(crate) fn record(key: Key, stage: &'static str, value: u64) // a sample that is not a time (fuel, bytes)
pub(crate) fn mark(stage: &'static str)          // ms since t0, once
pub(crate) fn suffixed(stage: &str, suffix: &str) -> Option<&'static str> // an interned name, only when on

pub(crate) fn snapshot(by_instance: bool) -> serde_json::Value
pub(crate) fn reset()                            // counters and samples; the marks stay
pub(crate) fn summary()                          // one `perf_summary` line if anything changed
pub(crate) fn retire(module: &'static str, instance: u64) // one `view_perf` line
```

- `Timer` records `elapsed()` on drop into the histogram named by its key.
- The store is one `Mutex<BTreeMap<(Key, &'static str), Metric>>`, a metric
  being a counter, a gauge `{max, last}` or `Samples { n, sum, max, last,
  ring: [u64; 256] }`. `n`, `sum` and `max` cover everything since the last
  reset; p50/p95 come from the ring and cover only the last 256 samples.
  `ponytail:` one global lock, one fixed ring; fine at fewer than a few
  thousand samples a second.
- Stage names are flat and carry their group as a prefix (`tick.call`,
  `fuel.tick`, `frame_bytes`, `host_call.module.query`,
  `host_call.module.query.attempts`, `io.read_prefs`, `reducer.pane`), so
  a key's JSON is one object of stage → value, the same in `/perf`,
  `perf_summary` and `view_perf`.
- Keys are `&'static str`: module names are interned already
  (`runtime::intern`), stage names are literals.
- Keyed by **(module, instance)**, not module: the seat registry is
  `(module, instance)` (`seat::mounted`, `Seat::new`), and two
  windows can show the same view. The seat carries its instance
  (`Mounted.instance`, `Guest.instance`), so the loader thread and the
  window thread write under one key; a preloaded seat's load stages land
  under instance 0 until a tab claims it. `/perf` aggregates by module
  unless `?by=instance`.
- Why not on `Guest`: stats must survive a swap and cover load stages that
  run before the `Guest` exists. The registry is a leaf lock; reading it
  never takes a seat lock.
- Windows are keyed by `WindowKey` (`src/runtime.rs`). The door names
  windows `console`, `console2`, … (`Windows::served`: the console
  `console`, any other `console<key>`), each for its life, and the
  snapshot carries both.

**Cost when off.** `on()` is an atomic load. `time` returns `None` before
touching the clock; `count`/`gauge`/`record` return before the lock. ducktape-70 reports that
its temporary helper's `span()` took `Instant::now()` unconditionally and
tested the switch on drop (never committed, so not checked here); the
permanent version must not copy that.

**Cost when on.** Two `Instant::now()`, one lock, one map lookup and an
O(1) ring write per sample. The node-count walk and picture-byte sums run
only when on and only when the tree changed.

### 3.2 One tracing target

- `ducktape::perf` carries the per-event debug stream: the per-tick fuel
  event in `Guest::tick`, plus one debug event per registry sample.
  Callsite-filtered, so it costs nothing unless
  `RUST_LOG=ducktape::perf=debug` (the filter is `info` plus `RUST_LOG`,
  `install_log`). There is no `ducktape::fuel` target and no alias; the
  `FUEL_PER_TICK` doc in `src/runtime.rs` names the new one.
- `view_load` (`LoadTiming::log`) is on this target as well, with `blob`,
  `code`, `instantiate_ms`, `restore_ms`, `snapshot_ms`, `snapshot_bytes`,
  `lock_wait_ms` and `install_ms`, and is logged after install rather than
  before it. Nothing in qa reads the old field names.

### 3.3 The door: `GET /perf`

- Route: `("GET", "perf")` and `("POST", "perf/reset")` in
  `ax::http::route`, listed in its 404 endpoint list; `Request::Perf` and
  `Request::PerfReset` in `src/ax.rs`; the CLI verb `ducktape-app ax perf
  [--by instance | --reset]` in `ax::http::request` (`--reset` sends the
  POST). `POST /perf/reset` clears the
  counters and samples (the startup marks stay); `GET` never mutates.
- `POST /key` with `"delta": false` is the one press that reads no tree
  (§4.3), so it leaves `cache_on` as it found it.
- **It must not draw.** `ax::actions::current` turns a11y on at the first
  read and calls `window.draw(cx)` on every read; each such draw renders
  the root and the pane views once (`door_draws`), while every cached
  layer and tree that did not move hits. `/perf` is answered in
  `ax::answer` from `perf::snapshot()`; its one window access reads
  `is_a11y_active` (and, with `perf-deep`, the profiler's histograms) and
  never draws.
- Reply shape: one flat object of stage → value per key. A counter is a
  number, a gauge `{max, last}`, a histogram `{n, mean, p50, p95, max,
  last}` (µs for times). Views by module, or by `module/instance` with
  `?by=instance`; windows by `WindowKey` number with the door's `name`
  beside it; `startup` the marks in ms since `t0`:

```json
{ "on": true, "since_ms": 123456, "cache_on": true,
  "startup": { "log": 3, "gpui": 41, "fonts": 60, "boot": 70, "window": 190, "connected": 800, "desk": 1500, "first_seated.chat": 1700 },
  "views": { "chat": { "module": "chat",
      "ticks": 412, "busy_ticks": 3, "draws": 900, "renders": 415, "faults": 0,
      "fuel.tick": { "n": 412, "p50": 4100000, "p95": 31000000, "max": 71800000 },
      "tick.call": {...}, "tick.decode": {...}, "merge": {...}, "replace": {...}, "render": {...},
      "frame_bytes": {...}, "events_bytes": {...}, "snapshot_bytes": { "max": 20480 }, "memory": { "max": 12582912 },
      "picture_bytes": {...}, "view_bytes": {...}, "nodes": { "max": 1900 }, "truncations": 0,
      "frame.full": 3, "frame.patch": 90, "frame.unchanged": 319, "requests": {...}, "cancels": {...}, "backlog": {...},
      "host_call.module.query": { "n": 20, "p50": 9000, "p95": 40000, "max": 90000 }, "host_call.module.query.attempts": 20,
      "fetch": {...}, "fetch.disk": 1, "compile": {...}, "compile.memory": 1, "instantiate": {...}, "fuel.instantiate": {...},
      "snapshot": {...}, "restore": {...}, "init": {...}, "first_frame": {...}, "install.lock_wait": {...}, "install": {...}, "first_tree": {...} } },
  "windows": { "1": { "name": "console", "renders": 240, "renders.chrome": 3, "renders.panes": 240, "renders.pane.0": 240, "renders.dot": 230,
                      "frame.render": {...}, "switch": {...},
                      "gpui": { "us": { "dirty_to_present": {...}, "draw": {...}, "present_interval": {...}, "input_latency": {...} } } } },
  "shell": { "reducer.connect": {...}, "reducer.pane": {...}, "io.read_prefs": {...}, "io.store.get": {...},
             "rail": {...}, "rail.calls": 900, "figure.frame": {...}, "figure.interval": {...}, "roster": {...},
             "door_draws": 0, "rss.peak": { "max": 400000000 }, "rss.current": { "last": 350000000 } } }
```

- `"cache_on"`: true when no served window has a11y active (`Window::is_a11y_active`),
  so no door read has drawn in any. The layers and trees draw cached with
  a11y on too (gpui-pre#8): it says the window's counts are free of door
  draws, not that the cache was off.
  A gate refuses to judge cache metrics (`misses`, `hits`, full redraws per
  switch, idle drawing) from a reply with `cache_on: false` (§4.3).
- `"gpui"` per window only with `perf-deep` (§3.5), µs from nanosecond
  histograms kept since the window opened, not since the last reset;
  `door_draws` counts the door's own draws for the process,
  since `ax::actions::current` has no window key.
- `misses` and `hits` are derived by the reader: `renders` and `draws`
  per view.
- Off: `{"error": "perf is off: launch with DUCKTAPE_PERF=1"}` with status
  409, so a gate fails loudly instead of passing on empty data.
- Access as for every door call: loopback only, bearer token from the door
  file (`ax::http::open`).

### 3.4 app.log summaries

- One `perf_summary` info line every 10 minutes if anything changed (the
  `perf` sampler thread, which also reads RSS each second), and once from
  `cx.on_app_quit` in `shell::launch::run` — the one hook every quit path
  reaches (`Windows::quit` runs only for the tray's and the menu's Quit).
- One `view_perf` info line per instance when it retires — swap, retry and
  roster removal all drop the old `Guest`, so it is `impl Drop for Guest`,
  skipping instances whose `installed_generation` is `None`.
- Same stable field names as the `/perf` JSON, so a log line and a door
  reply are the same numbers.

### 3.5 Deep dives (dev only)

- **`perf` on Linux.** `DUCKTAPE_WASM_PROFILER=perfmap|jitdump` sets
  `Config::profiler(ProfilingStrategy::PerfMap | JitDump)` in
  `guest::engine`. The strategy is not part of the compile-cache key
  (`HashedEngineCompileEnv` in wasmtime's `compile/code_builder.rs` hashes
  the compiler triple, flags, isa flags, tunables, features, `wmemcheck` and
  `module_version`), so the disk cache stays warm and every module registers
  with the agent, cache hits included. wasmtime's `profiling` feature is in
  its defaults.
- **`GuestProfiler` (Firefox-profiler JSON).** `DUCKTAPE_WASM_PROFILE=<module>`.
  It needs `Config::epoch_interruption(true)`, a ticker calling
  `Engine::increment_epoch`, `Store::set_epoch_deadline` in `guest::arm`
  and `Store::epoch_deadline_callback` to call `GuestProfiler::sample` and
  continue; `GuestProfiler::finish` writes the file. `epoch_interruption` is
  a **tunable**, so it is in the cache key: the one process-wide `Engine`
  compiles every view cold while this is on, and every `arm` needs the
  deadline. Keep it a dev flag. If the owner adopts a real time budget (§7),
  epoch interruption is always on and this cost disappears.
- **Names.** Shipped views are stripped and `wasm-opt`'d
  (`modules/tools/view-gate.sh` → `optimize-view.sh`); `make wasm-why
  V=<view>` in modules keeps names and skips `wasm-opt`. Use it for *where*
  time goes, not *how much*. It can only be seated through
  `DUCKTAPE_VIEWS_DIR`, and override loads are always `Fresh` and log no
  `view_load`, so the swap/snapshot/restore path cannot be profiled that way.
- **Not `Store::call_hook`.** It needs wasmtime's non-default `call-hook`
  feature.
- **gpui histograms.** The app feature `perf-deep = ["gpui-kit/profiler"]`
  (owner, §7) reads `Window::frame_duration_snapshot` and
  `Window::input_latency_snapshot` into each window's `"gpui"` object in
  `/perf` (`ax::gpui_perf`), and polls a `HangDetector` over
  `App::foreground_journal` once a second (`shell::launch::first_present`)
  until it latches the `first_present` startup mark. gpui's histograms
  cover the window's whole life: `POST /perf/reset` does not clear them.
  `cargo build --features perf-deep`; a default build has none of it.

---

## 4. Budgets and the qa gate

### 4.1 Ceilings the app enforces today

| Ceiling | Value | Effect |
|---|---|---|
| `FUEL_PER_TICK` | 250M | trap → fault, "This view stopped" |
| `MEMORY_LIMIT` | 64 MiB | trap on grow |
| `MAX_FRAME_BYTES` | 8 MiB | refused in `guest::shape` → fault |
| `MAX_REQUESTS_PER_TICK` | 256 | refused in `guest::shape` → fault |
| `MAX_NODES` (view-wire) | 8192 | truncation, not a kill |
| `MAX_PATCHES` (view-wire) | 1024 | patch refused, `Resync` asked |
| `MAX_SNAPSHOT_BYTES` (view-wire) | 8 MiB | checked by `wire::Snapshot::decode` in `Guest::load` |

### 4.2 Initial budgets

Measured so far (fold in, attribute, do not over-trust):

- Fuel, heaviest ticks, #291 (`3a2d63c3`, 2026-09-24, the commit that set
  `FUEL_PER_TICK` to 250M): chat ~72M, forge ~45M, explorer ~26M. A manual
  per-tick fuel trace, screen and stage not recorded, before the qa gate
  existed.
- Explorer, qa's `a11y-census` on a freshly seeded stage (`kit up`,
  `kit seed`, the seed inside the 1,000-block window). The heaviest tick is
  one render of the Transactions page, 50 rows and 1,074 wire nodes
  (`nodes.max`). `fuel.tick.max` read 111.95–114.43M in all 89 such runs on
  record before modules#232 (2026-09-29 to 10-01). modules#232 (`62e52242`)
  made the SDK's diff move a replaced subtree instead of copying it:
  96,763,257 in the census at the `dev` heads (2026-10-01: app `cf8384cd`,
  modules `62e52242`, qa `24a1e33a`), and 96.7–96.8M in every seeded
  census on record since.
  Without the seed the explorer reads less: 18.8M at 218 nodes when every
  block in the window was empty (2026-09-29), 57–59M at 394 nodes once the
  seed had aged out of the window, and 53–61M at 467–468 nodes in
  `perf-drive`, which never opens Transactions. #291's 26M fits a
  near-empty window.
- #347 (`193c9a21`), seeded stage, three windows, Xvfb, measured with
  ducktape-70's temporary spans before and after the fix. These replace the
  earlier `view_tree.render` ~2.9 ms figure, which was taken with the cache
  broken:

  | | before #347 | after #347 |
  |---|---|---|
  | full view redraws per window switch | 57–67 | 11–14 |
  | drawing time per frame | 11–12 ms | 2.5–3.5 ms |
  | idle drawing over 5 s | 979 ms | 334 ms |
  | idle frames/s (status-dot pulse, motion on) | ~20 | ~20, now cheap |

  Root causes, both "cached guest trees never stayed cached": an id-less
  container measured on its named ancestor's path and flagged a change
  ~2,900 times per 2 s (`ViewTree::measure`, `src/render/layout.rs`);
  gpui-base's selection sweep sent `SelectionChanged(None)` every frame and
  `refresh_window_on_change` refreshed the window on each (now
  `refresh_on_change`, `src/render/text.rs`). The 57–67 / 11–14, 11–12 /
  2.5–3.5 ms and 979 / 334 ms figures and the ~2,900 count are the PR's;
  the rest of the idle story is ducktape-70's session. Wall numbers are one
  rig, one run: **report-only until three baseline runs on the reference
  rig.**
- members-view sits at 98.5% of its wasm size limit (owner's number; the
  modules tooling enforces no limit — `tools/view-gate.sh` prints
  `name  bytes` and nothing else). Its top
  guest-side cost is `view_wire::Node` serialize/clone/eq. The host cannot
  time guest encode, but it sees the result: track `bytes.view` (the
  `view_section` length per load) and `bytes.frame` per tick as D metrics,
  and `tick.call` as the encode time's upper bound — the guest settles,
  diffs and encodes inside the one `Exports::tick` call, so `bytes.frame`
  and `tick.call` together are the tracked pair.

Rules, in `qa`'s `perf-budgets.json`, keyed by module and by window:

1. **Hard ceiling**: 50% of each kill ceiling, every view. Fuel max ≤ 125M,
   frame bytes max ≤ 4 MiB, requests max ≤ 128, memory max ≤ 32 MiB.
2. **Regression budget**: baseline × 1.25 on the D metrics — fuel max and
   p95, frame bytes max, nodes max, requests max, truncations, snapshot
   bytes, memory max, picture bytes, view bytes — refreshed from a green
   run on `dev`. For `fuel.tick.max` a run is green when the steps that
   produce the metric pass, whatever AX audits fail elsewhere in it.
   Starting points: chat 72M → 90M, forge 45M → 56M, explorer 26M → 33M;
   the explorer's was refreshed to 96.8M → 121M (below). Gate on `max` and
   counts since the last reset, never on equality: inputs vary with when
   replies and blocks land.

   The explorer's refresh, and the procedure for the next one. Bring up a
   fresh stage and seed it (`kit up`, `kit seed`), run qa's `a11y-census`
   with the app and the views at the `dev` heads, and read
   `views.explorer.fuel.tick.max` from the end snapshot in the run's
   `perf.jsonl`. The run is green for this metric when "open the
   Transactions page" and "audit Explorer: transactions" pass, and it is a
   baseline only when it drew the full Transactions page: explorer
   `nodes.max` 1,074 today, and a run far under that had a seed that had
   aged out of the window. The budget is the reading × 1.25, rounded to
   the million. At or over rule 1's ceiling it would be no regression
   budget: the ceiling reports the same tick without it.

   Refreshed 2026-10-01: 96,763,257 × 1.25 = 120,954,071, so 121M. The run
   is the census at the `dev` heads (app `cf8384cd`, modules `62e52242`, qa
   `24a1e33a`): 93 of its 94 steps passed, both Transactions steps among
   them, and the one that failed is the Chat reaction-picker audit. The 33M
   it replaces was never reachable on a seeded stage (113M before
   modules#232), and 113M × 1.25 is over rule 1.

   What holds the explorer's fuel, in order: this budget (a tick over 121M
   is reported, 25% over the baseline), rule 1 at 125M (4M above it), and
   the 250M kill (§4.1). Nothing in `perf-budgets.json` reports a
   regression under 25%, and of this rule's D metrics the file holds only
   the fuel max for the explorer. `--perf-baseline <run-dir>` (§4.4) is the
   check that compares every D metric with a recorded run (fuel p95 and
   nodes max among them), and no census run passes it today. Every line
   here is a report: `breach_fails_the_scenario` is false.
3. **Idle budget**: after a scenario settles, `POST /perf/reset`, wait 2 s
   with **no door calls**, then `GET /perf`:
   - `busy_ticks == 0`;
   - `ticks ≤ clock items + block-subscription items + 1`. Clocks have a
     16 ms floor (`MIN_TICK_MS`, `src/runtime/kernel.rs`); `module.changes`
     delivers one item per block that wrote to the program, `chain.heads` polls
     at block_time/2 clamped to 100 ms–2 s and delivers one item per block
     (`kernel::node::live`, `heads`);
   - `renders ≤ ticks + 2` per view, because every tick notifies the
     ViewTree in `Seat::turn`. Renders far above ticks is a
     notify loop: the #347 class of bug, and this is its regression gate
     (only meaningful with `cache_on: true`, §4.3);
   - per window, the root and its layers (§5 names the view each counts):
     root `renders.<window>` ≤ seconds / 2 + 2 with motion off (nothing
     beats: a window draws only what moved, at most a new height per 2 s
     status poll and the account read it starts); ≤ 25 × seconds with
     motion on (each pulse of `layers::StatusDot` draws the dot and the
     root; the pulse is capped at 30 fps, `with_max_fps`, and the budget
     moves to 30 × seconds only if a measured root count passes 125 per
     5 s, which none below does); `renders.chrome` ≤ seconds / 2 + 2 in
     both modes (the dot is the bar's sibling, so a pulse leaves the bar
     cached);
     `renders.pane.<n>` ≤ root `renders` (a `PaneView` is uncached and
     draws with the root, never more); `renders.overlays` and
     `renders.toast` ≤ 2; a view's `draws` ≤ root `renders`;
     `door_draws == 0`. In-process, on the same counters:
     `a_pulse_re_renders_the_dot_and_not_the_chrome`,
     `a_still_chain_draws_no_frame`, `a_pane_wake_leaves_the_chrome_alone`
     and `a_toast_re_renders_its_layer_only`
     (`src/shell/layers/root_tests.rs`). Preconditions, or a healthy app
     fails: no text field focused (a focused field's caret
     blinks every 500 ms, gpui-base 0.7, and each blink draws the field's
     cached ancestor, its layer or its view's tree, and the root: the idle
     recipe Tabs the keys onto a button first); no menu open (the node
     menu and the bell tick their ages every second, and the node menu's
     pulse draws the bar); the node answering (the reconnecting bar pulses
     in the footer, `renders.toast`); and no empty pane in a served window
     (each `layers::EmptyPane` draws the Spin figure, paced at
     33 ms by `Spin::run` whatever the window's activity; `renders.empty`
     counts its frames). A rail row saying `Loading` is no precondition: no
     frame loop waits on it (the seat wakes on its own `wake`, the rail on
     `runtime::changes_channel`). Measured, release, Xvfb 1280×800, the
     stage seeded at 1 s blocks, Forge open with the keys on a button,
     5 s windows (the idle probe, 2026-09-30), on the s12 build (#403,
     `cab059c6`), three runs per setting: motion off root 2, 2, 3 (a frame
     per status poll, each moving the height at 1 s blocks; the third
     window caught three polls); motion on 107, 107, 105 (about 21 a
     second, under 125 per 5 s: the budget stays 25 × seconds);
     `renders.chrome` 2, 2, 3 motion off and 2 in every motion-on run;
     `renders.pane.0` and forge `draws` equal to the root in every run;
     forge ViewTree `renders` 0 on 0 ticks; `renders.overlays`,
     `renders.toast` and `door_draws` 0.
   - idle drawing time per 5 s ≤ 500 ms (334 measured), report-only.
4. **Switch budget** (the keys-only scenario, §4.3): full view redraws per
   window switch (`misses` delta) ≤ 20 per window (11–14 measured; 57–67 was
   the bug); drawing time per frame p95 ≤ 6 ms, report-only (2.5–3.5
   measured).
5. **Wall clock**: report only under Xvfb (qa's rig is Xvfb: `qa/src/rig.rs`,
   `walk.py`), apart from an absolute hang ceiling: no single
   `frame.render`, `reducer.*` or `tick.call` sample ≥ 250 ms in the steady
   phase. Proposed, to be set from recorded runs: reducer p99 ≤ 2 ms,
   `io.*` p99 ≤ 2 ms, `figure.frame` p95 ≤ 2 ms, `switch_ms` p95 ≤ 50 ms,
   `startup.desk` ≤ 3 s with a local node.
6. **RSS peak** ≤ baseline × 1.2.

**Caveat on every qa number.** The door turns a11y on at its first read and
draws on every read (`ax::actions::current`), and a `wait` polls every
50 ms (`ax::answer`, `POLL`). With a11y on the window draws cached as it
does for a person: the pinned fork carries a cached view's nodes, focus
ids, bounds and action listeners through reuse (gpui-pre#8), so a read's
draw renders the root, the pane layer and the pane views (each view's
`draws`), and the bar, the dialogs, the footer, the strips and every
view's tree only if they moved. So qa's cache metrics (`misses`, `hits`)
mean the same with the door on as off, and a door-driven run's `misses`
are real; its root, `renders.panes`, `renders.pane.<n>` and `draws`
counts carry one render per read. The idle window must still contain no
door reads, and `door_draws` is reported so a reviewer can see how much of
a window's render count is the door's. `perf-switch` keeps its no-read
press: it measures what a person sees.

### 4.3 The a11y trap, and measuring with the cache on

Until the fork carried a cached view's nodes through reuse (gpui-pre#8),
the app drew every layer and guest view uncached while a11y was active, or
the tree would have lost their nodes: every draw of a guest view was a full
`ViewTree::render`, `misses == draws` by construction, and the whole #347
class of bug (a cached tree that never stays cached) was invisible to a
gate driven through `/tree`, `/act` or `/wait`. Now the layers and trees
draw cached with a11y on as well, so cache metrics can be judged with the
door on too: under the door a view's `misses` fall short of its `draws` by
at least the door's reads minus the tree's own changes, and a #347
regression shows in a door-driven run (in-process,
`a_cached_layer_keeps_its_nodes_under_a11y`: ten dot pulses and ten reads
with a11y on draw the pane view at least 20 times and its tree none, and the tree
keeps every node under the same parent). What a read still costs is one
render of the root and the pane views, which the idle window and the
switch budget must not count; there is no app-callable way to turn a11y
back off (`gpui:src/window.rs` has `activate_a11y`, and deactivation
belongs to the platform adapter), so those two keep a door-free window:
the no-read press below and `cache_on`.

Design:

- `/perf` never draws and never activates a11y (§3.3), and reports
  `cache_on`.
- Cache-independent D metrics — fuel, bytes, nodes, requests, ticks,
  truncations, memory, faults — gate in the same launch as any scenario.
- Cache-dependent metrics — `misses`, `hits`, full redraws per switch, idle
  drawing — gate only from a reply with `cache_on: true`. They get their own
  keys-only scenario, `perf-switch`: launch, connect, open three heavy views
  (chat on a busy channel, forge, members) by keyboard alone, switch windows
  twice by keyboard, idle 5 s, `GET /perf`. Its budgets are rule 4 and the
  idle drawing line above.
- **Today's `/key` is not keys-only.** `Request::Key` in `ax::answer` reads
  every window's tree before the press (to pick the window and compute the
  delta) and `settle`s on tree reads after it, so it activates a11y like
  `/tree`, `/act` and `/wait`; so do `/keys` and `/drag`. The scenario needs
  a press that reads nothing: a `POST /key` body flag (`"delta": false`, say)
  that skips both reads and answers `{}` — in `src/ax.rs`, which is free —
  or OS-level key injection on the rig's Xvfb. This document picks the flag.
  With `window` named, `keyboard_window` already resolves the handle from the
  door's window list (`Windows::served`) without a tree; unnamed, it
  finds the focused node in the tree it just read, so the flag makes
  `window` required (falling back to the first window, as it does today).

  **Landed (`perf/key-no-read`).** `Key` in `src/ax.rs` has `delta`, true
  unless the body says `"delta": false`. With it false, `answer` presses
  through `press_keys` and answers `{}`: it calls neither `read` nor
  `settle`, so a11y stays off and the door draws nothing (`door_draws`
  stays put; `deadline_ms` goes unused). `keyboard_window` gets no nodes,
  so `window` names the window, and an unnamed press goes to the first
  window served (no focused node to look for); `window` is not made
  required, since the first window is what a rig with one console wants.
  `ducktape-app ax key <keys> --no-read` sends it. Tests:
  `ax::tests::a_press_without_a_read_keeps_the_cache_on` (the key reaches
  the window, a11y off after the press, `/perf` `cache_on: true`; a tree
  read flips both), `ax::http::tests::key_route_parses_the_no_delta_flag`
  and `key_cli_words`. `/keys` and `/drag` still read.
- The gate refuses to judge a cache metric from a `cache_on: false` reply
  and says so, instead of passing.
- Recommended default for the owner (§7): the extra scenario plus the
  no-read key, not a door launch mode.

### 4.4 The gate

- **Where**: at the end of each scenario **and** of each mission, not as a
  new step kind. The two runners have different step-kind lists and an
  unknown kind fails at load. Rust: end of `run_scenario` (`qa/src/steps.rs`)
  and `run_mission` (`qa/src/mission.rs`). Python: after the step loop and
  at the end of `Walk.run_mission` in `walk.py`.
- **How**: `POST /perf/reset` at scenario start; at its end `GET /perf`
  through the door client (`qa/src/door.rs` `call`), then the idle window
  (§4.2 rule 3), then a second `GET /perf`. Write both as a `perf.jsonl`
  line beside `transcript.jsonl`; put the breaches in the scenario's
  result.
- **Failure**: an over-budget D metric or a steady-phase hang fails the
  scenario as `perf-over-budget <module|window> <metric> <value> > <budget>`.
  Oracles cannot do this: they see trees and context, not the door.
- **Baseline**: `--perf-baseline <run-dir>` fails any D `max` above 1.25 ×
  baseline; a green run on `dev` refreshes it.
- **Rig**: `DUCKTAPE_PERF=1` beside `DUCKTAPE_AX_DOOR=0` in `qa/src/rig.rs`
  and `walk.py`.
- **A mission that drives the gate**: connect to the local node, open every
  program in the rail, switch panes and windows across all of them, type
  into chat, open forge with a large file, settle, then the idle window.
  This covers load (fetch/compile/init), swap (a view redeployed mid-run;
  `kit seed` fills a stage with data and deploys no view), tick, switch and
  idle in one pass.
- **`cargo test` counts**, already in CI. An idle ViewTree renders zero
  times across N frames (#347's `an_unchanged_guest_tree_is_not_drawn_again`
  in `src/render/tests/layout.rs`, on the `cfg(test)` field
  `ViewTree.renders`). A seated view drawing #347's tree shape renders no
  more than it ticks, read off the registry's `renders` and `ticks` as
  `/perf` serves them (`an_idle_view_renders_no_more_than_it_ticks`,
  `src/runtime/seat/entity_tests.rs`): undoing #347's selection fix fails it.
  Undoing its layout fix does not — under the test scheduler a notify from
  inside a draw is cleared with that frame — so the bounds assertion in
  #347's own test guards that one. Also counts: a redraw of the desk with
  nothing in the pane changed renders no ViewTree
  (`a_desk_redraw_with_nothing_changed_renders_no_view_tree`,
  `src/shell/panes_tests.rs`, on the registry's `renders` for a seated view,
  the cached path); a status poll that finds the chain where it was draws
  no window (`a_still_chain_draws_no_frame`, `src/shell/layers/root_tests.rs`,
  on the window's `renders` and every layer's); a still figure asks for no frames
  (`a_still_figure_asks_for_no_frames`, `src/shell/spin.rs`, older than
  this document). Times are flaky under the test scheduler; counts are not.

---

## 5. Rollout

**Phase 0 and phase 1 — landed 2026-09-28 (`perf/registry`), the owner's
GO for both together.** The sessions that held the view files
(ducktape-70, ducktape-a3) had merged, so both phases went in one branch.

Phase 0, as listed:
- `src/perf.rs`: switch, registry, `snapshot`, `reset`, `mark`, the
  sampler thread (RSS each second, `perf_summary` every 10 minutes),
  `perf_summary` on `on_app_quit`.
- `GET /perf`, `POST /perf/reset`, `Request::Perf`, `Request::PerfReset`,
  `ax perf [--by instance]` (`src/ax.rs`, `src/ax/http.rs`), answered
  without a draw.
- Shell hooks: startup marks (`main.rs`, `shell/launch.rs`,
  `shell/windows.rs`, `shell/entities/session.rs`, `first_seated` in
  `seat/entity.rs`); the entities' `reducer.<domain>` timers (the
  reducer's own `dispatch` timer went with it); `WindowRoot::render` `frame.render` and `renders` per window
  (the root is uncached: every layer's miss draws it once);
  the bar, `renders.chrome` (`layers::Chrome`, cached: a count is a miss);
  the launcher screens, `renders.launcher` (`layers::LauncherLayer`,
  uncached: it draws with the root, which draws for `Screen` and `Prefs`,
  and for `Session` and `Account` through the launcher); what opens over
  the desk, `renders.overlays`: `layers::OverlayLayer` (Spotlight,
  Settings, "Add a device…"; cached over `Overlays`, `Spotlight` and the
  slices they read, a miss per keystroke in Spotlight's field, which is its
  child); the footer, `renders.toast`
  (`layers::ToastView`, cached: a toast or the node's answering moving is a
  miss); the node's breath, `renders.dot` (`layers::StatusDot`, uncached:
  it draws with the root, and each of its pulses draws the root);
  the pane layer, `renders.panes` (`layers::PaneLayer`),
  `renders.pane.<n>` (the `layers::PaneView` at layout index `n`) and
  `renders.strip.<n>` (its cached `layers::Strip`), and
  the pane bodies the app draws, `renders.empty` (`layers::EmptyPane`: the
  bare desk or an empty window) and `renders.help` (`layers::HelpPane`) per
  window;
  the `switch` timer from `pane_message`, `Windows::raise` and the activation
  observer to the next frame; `Spin::render` `figure.frame` and
  `figure.interval`; `rail` and `rail.calls`; `io.read_prefs`,
  `io.write_prefs`, `io.store.get`, `io.store.set`, `io.notify_save`,
  `io.rpc_client`.
- View hooks: blob source and view bytes (`backend/views.rs` hands the
  source back with the bytes), `host_call.<capability>.<operation>` and its
  `.attempts` (`kernel/node.rs`, one literal stage per `kernel::answer`
  arm), `backlog` (`Replies::drain_into` answers the count), `install` and
  `install.lock_wait`, the `view_perf` retire line (`impl Drop for Guest`)
  and `view_load` after install (`seat.rs`), `roster`, `picture_bytes`
  (`Pictures::bytes`).
- Same PR: the `FUEL_PER_TICK` doc names `ducktape::perf`; the runtime
  module doc no longer claims a time budget; ARCHITECTURE.md follows.

Phase 1, as listed:
- `Guest::tick` split (`tick.call`, `tick.decode`, `merge`), the fuel
  event on `ducktape::perf`, `frame.{full,patch,unchanged}`,
  `frame_bytes`, `events_bytes`, `nodes`, `requests`, `cancels`,
  `truncations`, `memory`, `faults`, `ticks`, `busy_ticks`
  (`guest/requests.rs`); the load-stage split with fuel per call and
  `snapshot_bytes` (`guest/lifecycle.rs`); `compile.{memory,disk,cold}`
  from the in-process cache and wasmtime's hit counter (`guest.rs`,
  approximate under concurrent loads, said so in code).
- `Seat`: `replace`, `first_tree`, `first_seated.<module>` (`seat/entity.rs`);
  `draws` (`shell/layers/panes.rs`, `PaneView`); `ViewTree::render` `renders`
  and `render` under the seat's key (`render.rs`, `ViewTree::with_perf_key`).
- `door_draws` in `ax::actions::current`, for the process.
- The idle rule as an app test:
  `runtime::seat::entity_tests::an_idle_view_renders_no_more_than_it_ticks`
  (`renders ≤ ticks + 2` on the registry's counters over eight cached
  frames of #347's tree shape; a notify loop on the tree or #347's
  selection refresh fails it).

Where the landed code differs from §2: `input::Observe` no longer exists,
so there is no per-view `layout`/`paint` histogram (gpui's own, under
`perf-deep`, cover the window); `first_tree` runs from the start of the
frame that found the view seated to its tree mounted; `door_draws` is
process-wide; the registry's JSON is flat (§3.3). `kernel::node` has no
`spawn_device`.

**Phase 2 — qa.** `DUCKTAPE_PERF=1` in both rigs, `perf.jsonl`,
`perf-budgets.json`, the end-of-scenario and end-of-mission gate with the
idle window in both runners, the driving mission (§4.4). Report-only until
the owner says budgets block (§7).

**Phase 3 — dev deep dives.** `DUCKTAPE_WASM_PROFILER`,
`DUCKTAPE_WASM_PROFILE`.

---

## 6. The temporary `DUCKTAPE_PERF` helper

ducktape-70's never-committed helper (a `Span` that `eprintln!`ed on drop,
a stderr `mark()`, spans on the window-switch path) was folded into this
design, not merged. What still informs:

- `on()` is the permanent switch, same shape.
- Its `span()` took `Instant::now()` unconditionally and checked the switch
  only on drop; the permanent `time()` returns `None` before touching the
  clock, and every argument that allocates or walks a tree sits inside an
  `if perf::on()`.
- Its first suspect — `pane_stage` calling `set_focused`/`set_props` every
  render — was not the cause of the window-switch cost. The root causes
  were the two cache-busting loops in `render/layout.rs` and
  `render/text.rs` (§4.2), merged as #347. Its early numbers were taken
  with the cache broken and are superseded by the #347 before/after. The
  `renders ≤ ticks + 2` idle gate and the `misses` delta per switch are
  what would have caught both loops, and what catches the next one — with
  the cache on (§4.3).

---

## 7. Open questions for the owner

1. **The gpui `profiler` feature.** It is the only route to end-to-end frame
   time (draw + present), input→present latency and gpui-level long-frame
   causes, and to a first-present startup mark. It costs one lock entry
   (`hdrhistogram`), a journal ring at `App` build and timestamps on draws,
   inputs and task polls whenever compiled in. Options: (a) never; (b) an
   app feature `perf-deep = ["gpui-kit/profiler"]` for dev and qa builds
   only, accepting that users and qa then run different binaries; (c)
   always on, accepting the cost.

   **Answered (owner, 2026-09-28): (b).** `perf-deep` is declared; under it
   `/perf` carries each window's frame and input histograms and the
   `first_present` mark (§3.5). A default build compiles none of it.
2. **A time budget.** The `src/runtime.rs` doc claimed one; none exists.
   Either add epoch interruption (always on; then the guest profiler no
   longer splits the compile cache) or delete the claim.

   **Answered (owner, 2026-09-28): delete the claim.** Fuel
   (`FUEL_PER_TICK`) is the only ceiling; no epoch interruption. The
   module doc says so.
3. **Do budgets block merges?** Fuel, bytes, nodes and idle counts are
   deterministic enough to gate.

   **Answered (owner, 2026-09-28): report only.** Nothing in this branch
   fails on a budget; the qa gate is phase 2.
4. **Measuring with the cache on.** §4.3 proposes one extra keys-only
   scenario per qa run for the cache-dependent metrics, because a11y cannot
   be turned off once the door has read a tree — and every door verb today,
   `/key` included, reads one, so the scenario also needs a `/key` that
   skips its reads. The alternative is a door launch mode that never
   activates a11y and refuses `/tree`, `/act` and `/wait` for the whole run.

   **Answered (owner, 2026-09-28): the document's pick** — the scenario and
   the no-read key, both phase 2.
   The no-read key has landed (§4.3); the scenario has not.
5. **Instance vs module keys on the door.** The registry is per
   `(module, instance)`; should `/perf` default to aggregating by module
   for qa's budgets, with instances behind `?by=instance`?

   **Answered (owner, 2026-09-28): yes.** `/perf` aggregates by module;
   `?by=instance` lists instances.
6. **`POST /perf/reset` vs `GET /perf?reset=1`.**

   **Answered (owner, 2026-09-28): the POST.** `GET` never mutates.
7. **`store.get`/`store.set` and the notification log on the window
   thread.** The I/O histograms (`io.store.get`, `io.store.set`,
   `io.notify_save`) will show whether they matter; if they do, moving them
   to the `views-kernel` thread is a runtime change, not a perf one —
   flagging it now so it is not a surprise later.

   **Moved (2026-10-01).** Both run on the blocking pool now
   (`kernel::in_order`, one queue per file); the three histograms time the
   job there, so they no longer measure the window thread.
