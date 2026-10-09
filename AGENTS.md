# Photo·AIrt Contributor Guide

## Project layout

- `src/main.rs` contains the entry point: the GUI, plus the `--render` (batch
  styles) and `--ai` (single AI job) command-line modes.
- `src/imaging.rs` holds the shared raster primitives. These cover the float
  `Img` buffer, blurs, structure-tensor flow fields, LIC, distance
  transforms, relief lighting, noise and the deterministic `Rng`. Reuse them
  rather than re-implementing per style.
- `src/styles/` is the algorithm catalogue. `mod.rs` holds the registry
  (`STYLES`), the parameter schema, `Params` and the render `Ctx`.
  `painterly.rs`, `drawing.rs` and `graphic.rs` hold the implementations.
- `src/finish.rs` is the non-destructive finishing layer applied to every
  artwork.
- `src/ai.rs` integrates the CLIs. It covers CLI and model discovery, roles
  (Art Director and Master Painter), prefs and model health, jobs, prompts,
  and the `claude` and `codex` process runners.
- `src/app.rs` holds application state, background workers and the message
  pump.
- `src/ui_canvas.rs` (the stage) and `src/ui_panels.rs` (chrome) draw the UI.
  `src/theme.rs` holds colours, fonts and custom widgets.
- `src/photo_io.rs` handles loading (EXIF orientation, plus a CLI fallback for
  HEIC and similar formats) and saving.
- `assets/fonts/` holds the bundled OFL fonts. `docs/screenshots/` holds the
  README images.
- `.github/` holds the CI workflow, Dependabot configuration and
  `CODEOWNERS`. The CI uses only GitHub-owned actions pinned by commit SHA,
  with a read-only token. `deny.toml` is the `cargo-deny` policy for
  advisories, licenses, bans and sources.
- Unit tests live next to the code in `#[cfg(test)]` modules.

## Development workflow

### Branch isolation

- Treat every new work session that will modify repository files as separate
  work. Before making edits:
  1. Inspect the worktree.
  2. Fetch the latest default branch.
  3. Create a dedicated, descriptively named branch from `origin/main`.
- Never make a new session's changes on an existing branch from an unrelated
  task, and never base the new branch on that task branch. If edits were
  started on the wrong branch, preserve them, then move them onto a branch
  based on the latest `origin/main` before continuing.
- `main` is protected by a repository ruleset. Changes reach it only through
  pull requests that pass the required CI checks (`Lint`,
  `Test (ubuntu-latest)`, `Test (macos-latest)`, `Test (windows-latest)`,
  `Dependency policy` and `Dependency review`). Pull requests are
  squash-merged and keep a linear history. Force-pushes and deletion of
  `main` are blocked. Never try to bypass or weaken these rules.
- Creating the branch does not authorize staging, committing or pushing.
  Those still require the explicit authorization described under
  Verification before handoff.

### Libraries before code

Before implementing a new feature, investigate whether a maintained crate
already provides the complete feature or important building blocks. Present
the viable options to the user before implementation:

- add a dependency and use the existing crate; or
- implement the behaviour from scratch.

Include the relevant maintenance, security, portability and binary-size
tradeoffs. Hand-written image algorithms are a deliberate part of this
project. Prefer established crates for decoding and encoding, SVG,
triangulation, file dialogs and anything security-sensitive.

### Toolchain and commands

The Rust toolchain is pinned in `mise.toml`. Run commands from the repository
root (prefix them with `mise exec --` if Rust is not on `PATH`):

```bash
cargo run --release -- path/to/photo.jpg
cargo test --release
cargo clippy --release --all-targets -- -D warnings
cargo fmt --check
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
cargo deny check   # needs cargo-deny, e.g. `mise exec cargo-deny@latest -- cargo-deny check`
```

Use `--release` when testing. Image algorithms are very slow unoptimised, and
the dev profile already optimises dependencies.

## Code conventions

- Follow `rustfmt.toml`: 140 columns and `use_small_heuristics = "Max"`.
  Keep clippy clean with `-D warnings`. If a lint is genuinely not
  applicable, use a narrowly scoped `#[allow(...)]` at the item.
- Use concise module and function doc comments. Put reusable behaviour in the
  existing module that owns it rather than duplicating it in UI code.
- Keep the UI thread responsive. Do rendering, file I/O and CLI calls on
  background threads, and report back through the existing channels with
  `ctx.request_repaint()`.
- **Styles**:
  - Each style is a pure `fn(&Img, &Params, &Ctx) -> Option<Img>`.
  - Scale size-like parameters with `ctx.px()` so thumbnails, previews and
    full-resolution exports look alike.
  - Report progress through `ctx.progress()`.
  - Honour `ctx.cancelled()` in long loops.
  - Stay deterministic for a given seed.
- **Public interfaces** must stay compatible:
  - command-line modes and flags;
  - `PHOTO_AIRT_*` environment variables;
  - style ids and parameter keys, which AI recipes reference by name;
  - the `prefs.json` format;
  - the output locations (`~/Pictures/Photo-AIrt`, `~/.cache/photo-airt`).
- When renaming shared symbols, search the whole source tree, including
  tests, before building.
- Keep sensitive values out of version control. The app needs no API keys;
  never add any.

## AI CLI integration

- All AI goes through the locally installed, logged-in `claude` and `codex`
  CLIs, using the user's subscriptions. Never call vendor APIs directly. Never
  pass API-key variables to the CLIs, and never switch a feature to paid API
  billing. Reaching a subscription limit should surface as an error, not a
  fallback.
- Discover capabilities at run time instead of hard-coding assumptions:
  - CLI presence and version;
  - Codex `image_generation`;
  - model catalogues;
  - availability learned from failures.
- Model choices are configuration (the Roles UI and `prefs.json`), not code.
  Role logic belongs in `Role`, `RoleCfg` and `Prefs`.
- Pass prompts on stdin. `claude --allowedTools/--add-dir` and
  `codex --image` are variadic and swallow trailing positional prompts.
- Keep the CLIs sandboxed to the job directory:
  - restrict Claude with `--tools Read`;
  - run Codex text tasks with `-s read-only --disable image_generation`;
  - use `-s workspace-write --enable image_generation` only for Codex
    painting.
- Only the downscaled working photo or the artwork may be sent to a model.
  Never send other user files.
- Tests must not spawn the real CLIs. Use the pure functions (role
  resolution, error parsing, JSON and SVG extraction) and verify CLI flows
  manually with `photo-airt --ai …`.

## Verification before handoff

Never stage, commit or push changes automatically. Perform each of these
operations only when the user explicitly asks for that specific operation. A
request to edit, fix, test or otherwise prepare changes does not authorize
staging, committing or pushing them.

Never credit an AI agent as an author or co-author. This rules out
`Co-Authored-By:` trailers, "Generated with …" footers and any similar
attribution in commits, pull requests, tags or release notes. It applies even
when a tool adds such lines by default.

Run focused tests while iterating. After the final edit, and before every
commit or push, always run the full test, lint, format and whitespace
sequence:

```bash
cargo test --release
cargo clippy --release --all-targets -- -D warnings
cargo fmt --check
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
cargo deny check
git diff --check
```

- Any code, test, configuration or documentation edit after this sequence
  invalidates it. Rerun the full sequence before committing or pushing.
- If a new dependency introduces a license that is not allowed, never
  loosen `deny.toml` silently. Present the crate and its license to the user
  first.
- If the format check reports files, run `cargo fmt`, then rerun the complete
  sequence. Do not rely on clippy alone, because it does not enforce
  formatting.
- For algorithm changes, also render the affected styles with
  `photo-airt --render <id|all> photo.jpg out/` and inspect the images.
  Unit tests only prove that rendering succeeds, not that it looks right.
- For user-facing changes, verify that the README text, examples and
  screenshots in `docs/screenshots/` still match the implementation.
