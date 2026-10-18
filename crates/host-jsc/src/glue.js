// The page side of the iOS view runner (crates/host-jsc, wasm-mobile SPEC §6).
// Every seated view is a WebAssembly.Instance here, keyed by the runner's
// instance id; every describe module runs in a fresh instance of its own.
// The runner calls `dt.call(a)` through callAsyncJavaScript with one string,
// "<op> <id> <name> <arg>" (`arg` base64), and gets one string back
// (`answer.rs` reads it):
//
//   R <gas> <memory> <base64>   the export returned: gas_left after it, the
//                               memory's bytes, the answer's bytes
//   T <gas> <memory> <words>    the call trapped: gas_left after it (the
//                               runner reads Memory off it,
//                               view_meter::stop), the trap's words
//   P <memory> <words>          the view panicked: the words its panic hook
//                               (ducktape_view.panicked) handed over first
//   F <words>                   the view failed outside a call's trap (it
//                               does not instantiate, lacks an export, or
//                               answered outside its memory)
//   E <words>                   the page's own failure (no such seat, the
//                               module did not load)
//
// The page holds no meaning: answers are decoded in Rust, as the wasmtime
// runner decodes them. Nothing here imports beyond the view's one import.
"use strict";

const dt = (() => {
    let viewFuel = 0n;
    let describeFuel = 0n;
    // compiled modules by name, as the scheme handler serves them
    const modules = new Map();
    // seated views by instance id: { e: exports, panic: words or null }
    const seats = new Map();

    const toBase64 = Uint8Array.prototype.toBase64
        ? bytes => bytes.toBase64()
        : bytes => {
            let text = "";
            for (let at = 0; at < bytes.length; at += 0x8000) {
                text += String.fromCharCode.apply(null, bytes.subarray(at, at + 0x8000));
            }
            return btoa(text);
        };
    const fromBase64 = Uint8Array.fromBase64
        ? text => Uint8Array.fromBase64(text)
        : text => {
            const raw = atob(text);
            const bytes = new Uint8Array(raw.length);
            for (let at = 0; at < raw.length; at++) bytes[at] = raw.charCodeAt(at);
            return bytes;
        };

    function compiled(name) {
        let module = modules.get(name);
        if (!module) {
            module = fetch(`dt://app/${name}`)
                .then(response => {
                    if (!response.ok) throw new Error(`${name}: ${response.status}`);
                    return response.arrayBuffer();
                })
                .then(bytes => WebAssembly.compile(bytes));
            modules.set(name, module);
            // a failed load is not kept: the next seat asks again
            module.catch(() => modules.delete(name));
        }
        return module;
    }

    // `bytes` in a buffer the view allocated, as the wasmtime runner gives it
    function give(e, bytes) {
        const ptr = e.alloc(bytes.length) >>> 0;
        new Uint8Array(e.memory.buffer, ptr, bytes.length).set(bytes);
        return ptr;
    }

    // the bytes a packed (ptr << 32 | len) answer names, copied out; null
    // when they are outside the view's memory
    function answer(e, packed) {
        const bits = BigInt.asUintN(64, BigInt(packed));
        const at = Number(bits >> 32n), len = Number(bits & 0xffffffffn);
        if (at + len > e.memory.buffer.byteLength) return null;
        return new Uint8Array(e.memory.buffer, at, len).slice();
    }

    function returned(e, bytes) {
        return `R ${e.gas_left.value} ${e.memory.buffer.byteLength} ${toBase64(bytes)}`;
    }

    // one export call, armed with the budget first (alloc and the call share
    // it, as on wasmtime); `run` returns the answer's bytes
    function enter(seat, run) {
        const e = seat.e;
        e.gas_left.value = viewFuel;
        let bytes;
        try {
            bytes = run(e);
        } catch (error) {
            const words = seat.panic;
            seat.panic = null;
            if (words) return `P ${e.memory.buffer.byteLength} ${words}`;
            return `T ${e.gas_left.value} ${e.memory.buffer.byteLength} ${error && error.message || String(error)}`;
        }
        if (bytes === null) return "F the view answered outside its memory";
        return returned(e, bytes);
    }

    async function seatView(id, name) {
        let module;
        try {
            module = await compiled(name);
        } catch (error) {
            return `E the view's code did not load: ${error && error.message || error}`;
        }
        const seat = { e: null, panic: null };
        const imports = {};
        for (const wanted of WebAssembly.Module.imports(module)) {
            if (wanted.kind !== "function") continue;
            const from = imports[wanted.module] || (imports[wanted.module] = {});
            from[wanted.name] = wanted.module === "ducktape_view" && wanted.name === "panicked"
                ? (ptr, len) => {
                    // the first line of the words, as the wasmtime runner keeps them
                    const buffer = seat.e.memory.buffer, start = ptr >>> 0, end = start + (len >>> 0);
                    const text = end <= buffer.byteLength
                        ? new TextDecoder().decode(new Uint8Array(buffer, start, end - start))
                        : "";
                    seat.panic = text.split("\n")[0].replace(/\r$/, "");
                }
                : () => {
                    throw new WebAssembly.RuntimeError(`unknown import: ${wanted.module}::${wanted.name} has not been defined`);
                };
        }
        let instance;
        try {
            instance = new WebAssembly.Instance(module, imports);
        } catch (error) {
            return `F ${error && error.message || error}`;
        }
        const e = instance.exports;
        if (!(e.memory instanceof WebAssembly.Memory)) return "F the view exports no memory";
        // snapshot and restore are optional (`ViewInstance`): a program's
        // notifier hands no state over, and a call into one it lacks traps
        for (const name of ["alloc", "init", "tick"]) {
            if (typeof e[name] !== "function") return `F the view exports no ${name}`;
        }
        if (!(e.gas_left instanceof WebAssembly.Global)) return "F the view is not metered";
        seat.e = e;
        seats.set(id, seat);
        e.gas_left.value = viewFuel;
        return returned(e, new Uint8Array(0));
    }

    // a describe module on `op`, in an instance of its own
    async function describe(name, op) {
        const module = await compiled(name);
        const e = new WebAssembly.Instance(module, {}).exports;
        e.gas_left.value = describeFuel;
        const bytes = answer(e, e.describe(give(e, op), op.length));
        if (bytes === null) return "F the module answered outside its memory";
        return returned(e, bytes);
    }

    async function call(a) {
        const [op, idText, name, arg] = a.split(" ");
        const id = Number(idText);
        try {
            switch (op) {
                case "boot":
                    viewFuel = BigInt(name);
                    describeFuel = BigInt(arg);
                    return `R 0 0 ${btoa(typeof WebAssembly)}`;
                case "seat":
                    return await seatView(id, name);
                case "describe":
                    return await describe(name, fromBase64(arg));
                case "forget":
                    modules.delete(name);
                    return "R 0 0 ";
                case "drop":
                    seats.delete(id);
                    return "R 0 0 ";
                // the simulator gate's (`JscRunner::stall`): the page busy
                // until its process is killed, so a call begun after it is
                // still in flight when the page dies. Nothing else sends it.
                case "stall":
                    for (;;) {}
            }
            const seat = seats.get(id);
            if (!seat) return `E no view is seated as ${id}`;
            if ((op === "snapshot" || op === "restore") && typeof seat.e[op] !== "function") {
                return `F the view exports no ${op}`;
            }
            switch (op) {
                case "init":
                    return enter(seat, e => (e.init(), new Uint8Array(0)));
                case "tick": {
                    const events = fromBase64(arg);
                    return enter(seat, e => answer(e, e.tick(give(e, events), events.length)));
                }
                case "snapshot":
                    return enter(seat, e => answer(e, e.snapshot()));
                case "restore": {
                    const state = fromBase64(arg);
                    return enter(seat, e => answer(e, e.restore(give(e, state), state.length)));
                }
            }
            return `E no such call: ${op}`;
        } catch (error) {
            return `E ${error && error.message || error}`;
        }
    }

    return { call };
})();
