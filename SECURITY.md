# Security policy

## Supported versions

Security fixes go to the latest release on the `main` branch.

## Reporting a vulnerability

Please do not open a public issue for security problems. Report them
privately instead, through
[GitHub private vulnerability reporting](https://github.com/JCallico/photo-airt/security/advisories/new).

Include:

- the affected version or commit;
- steps to reproduce;
- the impact you observed.

You can expect an acknowledgement within a few days.

## Scope notes

Photo·AIrt runs the locally installed `claude` and `codex` CLIs and gives
them a job directory under `~/.cache/photo-airt/jobs`. These are especially
relevant reports:

- anything that lets a model read or write outside that directory;
- anything that sends user files other than the working photo or artwork to
  a model;
- anything that causes API keys or other credentials to be stored or
  transmitted.
