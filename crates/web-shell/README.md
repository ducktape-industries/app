# ducktape-web-shell

ducktape in a browser: the host (`ducktape-host`, without its default features) on gpui's web platform, views run by the page's own WebAssembly (`ducktape-host-web`), one program's view on the page. The crate is a cdylib for `wasm32-unknown-unknown`; natively it is only the path reading below and its test.

**A read-only preview.** The page reads the network through a throwaway key made per tab and signs nothing: `device_key` is refused, so no account is seated, and a banner says "Read-only preview: sign-in coming". The browser's key store, its kept files in IndexedDB and its file picks come with the browser's own `OsServices` (J-405); until then kept files live in memory for the tab's life.

- **Which node.** `/config.json`'s `node` where the server gives one (the web image's deployment mounts it), else the page's own origin where it answers `/v1/status`, else the node the build names (`DUCKTAPE_WEB_NODE` at build time, else `https://duckpond.ducktape.industries`). The node answers any origin (core noded's CORS), so the page may live on a hostname of its own.
- **Which view.** The page's path is a duck link: `/<chain>/<program>/<route…>` opens `<program>`'s view with `<route…>` handed to it (`host.route`), and `/` opens chat. A link the view opens loads its path when it is a duck link, opens in a new tab when it is an http or https link (the scheme in any case), and is refused otherwise (`link::opening`, tested natively) (a view is untrusted, and a `javascript:` link would run as the page).
- **Renderer.** gpui's `Auto` (WebGPU, else WebGL2); `?renderer=webgl` or `?renderer=webgpu` picks one.

Build: `ops/web/build.sh [<dir>]` (cargo with the `web` profile, wasm-bindgen `--target web`, `wasm-opt -Oz`, then a brotli and a gzip copy of each file). The image: `ops/web/image.sh [<commit>]` builds `ops/web/Containerfile`, Caddy serving that directory on 8080 from `/srv/web`, sending the precompressed copies and `index.html` for every path it has no file for, except `/config.json` and the node's `/v1/*`, which answer 404 there: on a host that serves the node too, those go to the node ahead of the page's server.

Not in the browser yet: signing in and every signed write, the desk and launcher (the desktop's shell, `src/shell`), the live lane (`live.*` answers `live_unavailable`), view sockets, calls, CEF artifacts, banners, and a blob cache that outlives the tab.
