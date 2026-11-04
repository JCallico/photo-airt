# CI and coverage

The CI workflow checks formatting, Clippy, documentation, dependency policy,
and tests on Linux, macOS and Windows. The separate Coverage workflow runs
the same workspace tests in release mode with `cargo-llvm-cov` on Linux.
It publishes `target/coverage/lcov.info` to Codecov and keeps the report as
the `coverage-lcov` workflow artifact for 14 days.

Tests use generated photos, local HTTP servers and a headless egui studio.
They do not require an AI subscription, a display or clipboard access.
Real Claude and Codex CLI execution is verified separately.

## Codecov setup

1. Sign in to [Codecov with GitHub](https://app.codecov.io/login) and enable
   `JCallico/photo-airt`. Install the Codecov GitHub App for this repository
   so it can publish coverage checks and pull-request comments.
2. Add the repository's Codecov upload token as the `CODECOV_TOKEN`
   GitHub Actions secret. Pushes to `main`, manual runs and same-repository
   pull requests authenticate with this secret. All jobs keep read-only
   repository permissions.
3. Public pull requests without access to the secret, including forks and
   Dependabot updates, upload using Codecov's tokenless
   `pr<number>:<branch>` convention.
4. After the first successful `main` upload, the README coverage badge shows
   the baseline. Pull requests then report overall and changed-line coverage.

[`codecov.yml`](../codecov.yml) allows one percentage point of overall
coverage decrease and expects 80% coverage on changed lines. It only posts a
PR comment when coverage changes. Existing branch-protection requirements
are managed separately.

Coverage generation and upload failures fail the Coverage job. The LCOV
artifact is saved before the upload, so it is still available if Codecov is
unavailable. To inspect a report locally:

```bash
cargo llvm-cov --release --workspace --locked --html
# Open target/llvm-cov/html/index.html.
```

## Dependency and tool updates

[Dependabot](../.github/dependabot.yml) checks Cargo and GitHub Actions every
Monday. Minor and patch version updates are grouped per ecosystem; major
versions are reviewed individually. Cargo security updates have their own
group. GitHub security updates and alerts are enabled for the repository.
Dependency review checks each PR and `cargo-deny` also runs weekly for newly
published RustSec advisories.

Actions are GitHub-owned and pinned to full commit SHAs. Dependabot updates
these pins. The `cargo-deny`, `cargo-llvm-cov` and Codecov CLI binaries are
installed from versioned releases with pinned SHA-256 checksums. These shell
installation pins need manual updates: update the version and checksum
together, verify them against the upstream release, and run the workflow.
This keeps CI-only tools out of the app's dependencies and release binary.
