# klams-viewport

Tauri 2 + Svelte 5 desktop app that reads from a running klams
service.

## Build (Linux → Windows cross-compile)

```bash
# from this directory
pnpm install
pnpm build                                                # SvelteKit static SPA → ./build
cd src-tauri
cargo xwin build --release --target x86_64-pc-windows-msvc
# output: src-tauri/target/x86_64-pc-windows-msvc/release/klams-viewport.exe
```

Prerequisites on Linux: `cargo install cargo-xwin` plus
`apt install clang lld llvm` (clang-cl, lld-link, llvm-lib).

`cargo-xwin` downloads the Microsoft xwin SDK once into `viewport/xwin/`
(gitignored). No MSI is produced; only the raw `.exe` per FR-022
(`bundle.active = false` in `tauri.conf.json`).

See [quickstart.md §6](../sprints/001-initial-mvp/quickstart.md) for the
full walkthrough and [docs/memory.md](docs/memory.md) for the in-app
usage guide.

## Native dev (optional)

```bash
pnpm install
pnpm tauri dev
```
