# Releases

The [Release workflow](../.github/workflows/release.yml) builds portable
executable archives when a version tag is pushed. It waits for every build
and test to pass, then creates a **draft** GitHub release with generated
release notes and a `SHA256SUMS.txt` file. Publication is a separate manual
step in GitHub, so you can review the downloads and notes first.

| Archive suffix | Platform | Executable |
|---|---|---|
| `linux-x86_64.tar.gz` | Linux x64, glibc 2.35 or newer (Ubuntu 22.04 baseline) | `photo-airt` |
| `macos-aarch64.tar.gz` | macOS, Apple Silicon | `photo-airt` |
| `macos-x86_64.tar.gz` | macOS, Intel | `photo-airt` |
| `windows-x86_64.zip` | Windows x64 | `photo-airt.exe` |

Each archive includes the application and font licenses. Fonts are embedded
in the executable; no Rust toolchain or files beside the executable are
needed. Linux still needs its desktop graphics libraries and XDG desktop
portal. These are executable archives, with no installer, macOS app bundle,
code signing or notarization. The macOS builds currently use macOS 14
(Apple Silicon) and macOS 15 (Intel); test older systems before claiming
compatibility. AI features continue to use separately installed, logged-in
Claude Code or Codex CLIs.

## Preparing a release

1. Update the package version in `Cargo.toml` and regenerate `Cargo.lock`.
   Merge the change through a pull request after the required CI checks pass.
2. Fetch `main` and tag the intended commit on `origin/main`. The tag must
   exactly match the package version, prefixed with `v` (for example,
   `v0.1.0`). For a prerelease, use a package version such as `0.2.0-beta.1`
   and a matching `v0.2.0-beta.1` tag.

   ```bash
   git fetch origin main
   git tag -a v0.1.0 origin/main -m "Photo·AIrt v0.1.0"
   git push origin v0.1.0
   ```

3. Watch **Actions → Release**. The workflow rejects branch runs, version
   mismatches and commits outside `main`'s history. It builds and tests with
   `Cargo.lock` enforced on all four native runners.
4. Open the draft under **Releases**, review the notes, download and try each
   platform's executable, and verify the checksums. On Linux:

   ```bash
   sha256sum --check --ignore-missing SHA256SUMS.txt
   tar -xzf photo-airt-v0.1.0-linux-x86_64.tar.gz
   ./photo-airt-v0.1.0-linux-x86_64/photo-airt
   ```

5. Publish the draft in GitHub. Tags containing a hyphen automatically mark
   the draft as a prerelease.

Failed runs can be rerun from Actions. To rebuild an existing tag manually,
use the GitHub CLI:

```bash
gh workflow run release.yml --ref v0.1.0
```

A successful rerun replaces assets only while the release is still a draft.
Published releases are never overwritten; prepare a new version instead.
Only the final release job receives `contents: write`; the other jobs use
read-only tokens, and all actions are GitHub-owned and pinned by commit SHA.
