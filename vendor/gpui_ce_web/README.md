# gpui_ce_web (vendored patch)

Vendored copy of `gpui_ce_web 0.1.0` (from crates.io) with one fix: the
published package is missing the bundled font files that
`src/platform.rs` includes via `include_bytes!` (`assets/fonts/...` in the
upstream repo layout). Fonts are IBM Plex Sans and Lilex, both under the
SIL Open Font License (`fonts/*/license.txt|OFL.txt`), fetched from the
upstream repo [gpui-ce/gpui-ce](https://github.com/gpui-ce/gpui-ce).

Only two changes vs 0.1.0:
- `include_bytes!` paths point at the crate-local `fonts/` directory;
- `fonts/` added with the 8 TTF files + licenses.

Remove this patch once a fixed `gpui_ce_web` is published on crates.io.
