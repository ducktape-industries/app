#!/usr/bin/env bash
# The dependency-direction gate (wasm-mobile SPEC §3.4): the host core names
# no shell, OS, or runtime crate other than wasmtime; the phone shell names
# no desktop; no wasmtime on iOS; one fork rev; one programs rev; bans and
# licenses. Every row prints PASS or FAIL with what it found; a FAIL fails
# the script. Rows whose subject does not exist yet (the phone crate, the
# views lock) say SKIP.
#
# Run from the repository root before every push (CI is off; the app has
# no Makefile, so this is the gate itself):
#   tools/dep-gate.sh
set -uo pipefail
cd "$(git rev-parse --show-toplevel)"

status=0
pass() { printf 'PASS  %s\n' "$1"; }
fail() { printf 'FAIL  %s\n' "$1"; status=1; }
skip() { printf 'SKIP  %s\n' "$1"; }

# `cargo tree -p <crate> -e normal --target <t>`: the crate's normal (not
# dev, not build) dependency graph for one target. Resolution only, no
# compile: a target the toolchain lacks still answers.
# A cargo-tree failure (a crate name that is not one, a stale lock under
# `--locked`) returns 1 with cargo's last line on stderr; every caller turns
# that into a FAIL, never an empty, passing tree. (`tree` runs inside
# `$(...)`, a subshell: a `fail` in it would not reach `status`.)
tree() {
    local out
    if ! out=$(cargo tree --locked -p "$1" -e normal --target "$2" --prefix none ${3:-} 2>&1); then
        printf '      %s\n' "$(printf '%s' "$out" | tail -1)" >&2
        return 1
    fi
    printf '%s\n' "$out" | awk '{print $1}' | sort -u
}

# Row 1: host names no shell, OS or runtime crate. Transitively for the
# crates nothing under gpui pulls; at depth 1 for gpui's own platform
# backends (`gpui-pre-linux`, `gpui-pre-macos`) and its a11y stack's bus
# (`zbus`, under accesskit on Linux), which gpui-kit carries on those
# targets and host must not name itself.
host_banned='ducktape-app ducktape-phone ducktape-host-jsc ducktape-updater updater tray-icon keyring commonware-cryptography keystore keyscheme ducktape-home cef ducktape-gpui-ios gpui-pre-mobile jni ndk objc2-web-kit'
host_direct_banned='gpui-pre-linux gpui-pre-macos zbus'
for target in x86_64-unknown-linux-gnu aarch64-apple-darwin aarch64-apple-ios aarch64-linux-android; do
    all=$(tree ducktape-host "$target") || { fail "cargo tree -p ducktape-host --target $target failed"; continue; }
    near=$(tree ducktape-host "$target" "--depth 1") || { fail "cargo tree -p ducktape-host --target $target --depth 1 failed"; continue; }
    found=$(grep -xF -f <(tr ' ' '\n' <<<"$host_banned") <<<"$all" | tr '\n' ' ')
    direct=$(grep -xF -f <(tr ' ' '\n' <<<"$host_direct_banned") <<<"$near" | tr '\n' ' ')
    if [ -z "$found$direct" ]; then
        pass "host tree on $target (default features) names no shell, OS or runtime crate"
    else
        fail "host tree on $target names: $found$direct"
    fi
done

# Row 2: host is OS-blind in source (test modules excluded). Each
# `cfg(...)`, `cfg!(...)` and `cfg_attr(...)` is read whole, to its closing
# parenthesis, so a predicate rustfmt split over lines is read too.
cfgs=$(find crates/host/src -name '*.rs' ! -name tests.rs ! -name '*_tests.rs' ! -path '*/tests/*' -print0 |
    xargs -0 perl -0777 -ne '
        while (/\b(?:cfg!?|cfg_attr)\s*(\((?:[^()]++|(?1))*\))/g) {
            my ($whole, $at, $args) = ($&, $-[0], $1);
            next unless $args =~ /\b(?:target_os|target_family|target_vendor|unix|windows)\b/;
            my $line = 1 + (substr($_, 0, $at) =~ tr/\n//);
            $whole =~ s/\s+/ /g;
            print "$ARGV:$line: $whole\n";
        }')
if [ -z "$cfgs" ]; then
    pass "host source carries no OS cfg (target_os, target_family, target_vendor, unix, windows) outside test modules"
else
    fail "host source carries an OS cfg:"
    printf '%s\n' "$cfgs" | sed 's/^/      /'
fi

# Row 3: phone shell names no desktop. Row 4: no wasmtime on iOS.
if [ -d crates/phone ]; then
    phone_banned='ducktape-app ducktape-updater updater tray-icon keyring zbus cef commonware-cryptography'
    for target in aarch64-apple-ios aarch64-linux-android; do
        all=$(tree ducktape-phone "$target") || { fail "cargo tree -p ducktape-phone --target $target failed"; continue; }
        found=$(grep -xF -f <(tr ' ' '\n' <<<"$phone_banned") <<<"$all" | tr '\n' ' ')
        if [ -z "$found" ]; then
            pass "phone tree on $target names no desktop crate"
        else
            fail "phone tree on $target names: $found"
        fi
    done
    if ! ios=$(tree ducktape-phone aarch64-apple-ios); then
        fail "cargo tree -p ducktape-phone --target aarch64-apple-ios failed"
    elif grep -qx wasmtime <<<"$ios"; then
        fail "phone tree on aarch64-apple-ios links wasmtime"
    else
        pass "no wasmtime on aarch64-apple-ios"
    fi
else
    skip "crates/phone does not exist yet (rows 3 and 4)"
fi

# Row 4b: the workspace `describe` entry enables no `host` feature for
# every member; each crate that runs describe modules enables it itself.
if grep -E '^describe = .*features = \[.*"host".*\]' Cargo.toml >/dev/null; then
    fail "the workspace describe entry enables features = [\"host\"] for every member"
else
    pass "the workspace describe entry enables no host feature"
fi

# Row 5: one fork rev in every [patch.crates-io] entry.
revs=$(grep -E 'github.com/ducktape-industries/gpui-pre' Cargo.toml | grep -oE 'rev = "[0-9a-f]+"' | sort -u | wc -l)
if [ "$revs" -eq 1 ]; then
    pass "one gpui-pre fork rev in Cargo.toml"
else
    fail "$revs gpui-pre fork revs in Cargo.toml"
fi

# Row 6: one programs rev in every programs entry (and the views lock).
revs=$(grep -E 'github.com/ducktape-industries/programs' Cargo.toml crates/*/Cargo.toml | grep -oE 'rev = "[0-9a-f]+"' | sort -u)
count=$(printf '%s\n' "$revs" | grep -c .)
if [ "$count" -eq 1 ]; then
    pass "one programs rev across every manifest ($(printf '%s' "$revs" | grep -oE '[0-9a-f]{8}' | head -1))"
else
    fail "$count programs revs across the manifests: $(printf '%s ' $revs)"
fi
if [ -f phone/views.lock ]; then
    lock_rev=$(grep -oE '[0-9a-f]{40}' phone/views.lock | sort -u | tr '\n' ' ')
    if printf '%s' "$revs" | grep -q "$lock_rev"; then
        pass "phone/views.lock is at the manifests' programs rev"
    else
        fail "phone/views.lock rev ($lock_rev) differs from the manifests'"
    fi
else
    skip "phone/views.lock does not exist yet (row 6, lock half)"
fi

# Row 7: bans and licenses.
if command -v cargo-deny >/dev/null; then
    if [ -f deny.toml ]; then
        if cargo deny check bans licenses >/dev/null 2>&1; then
            pass "cargo deny check bans licenses"
        else
            fail "cargo deny check bans licenses (run it for the list)"
        fi
    else
        skip "no deny.toml yet (row 7)"
    fi
else
    skip "cargo-deny is not installed (row 7)"
fi

drawn=$(git grep -n -E '\.render_frame\(' -- '*.rs' | grep -vE '^[^:]+:[0-9]+:[[:space:]]*//' || true)
if [ -z "$drawn" ]; then
    pass "no Rust code calls the kit's render_frame: tests draw with render::draw_frame or refresh_and_draw, which keep a frame's cache replay"
else
    fail "the kit's render_frame refreshes before it draws, which hides a cache replay that a real frame takes; use render::draw_frame or refresh_and_draw:"
    printf '%s\n' "$drawn" | sed 's/^/      /'
fi

exit $status
