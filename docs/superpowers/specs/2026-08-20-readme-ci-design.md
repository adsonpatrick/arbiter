# README and CI Design

**Date:** 2026-08-20
**Status:** Approved design

## Goal

Make the repository understandable to a new contributor from its root and
continuously validate the supported Windows and Unix code paths without
requiring Codex credentials in GitHub Actions.

## README

Add an English `README.md` at the repository root. It will:

- explain Arbiter M0 as a local transparent governor for Codex;
- state that Codex owns ChatGPT authentication and Arbiter accepts no API key;
- summarize the Rust workspace and the responsibility of each crate;
- document prerequisites, release build, installation, validation, normal use,
  status inspection, and safe uninstall;
- provide the standard local quality and test commands;
- state the M0 security/privacy boundary and deferred scope;
- link to the detailed operations, contract, performance, release, design, and
  license documents rather than duplicating them.

Commands will be concise and usable from PowerShell. Paths and prose will not
assume an API key, a globally installed Arbiter binary, or a changed default
Codex profile.

## Continuous integration

Add `.github/workflows/ci.yml`, triggered for pull requests and pushes to
`main`. The workflow will use least-privilege read-only repository permissions
and cancel superseded runs on the same ref.

The workflow has two responsibilities:

1. `quality` runs once on Ubuntu and executes formatting, workspace check,
   Clippy with warnings denied, and `cargo deny`.
2. `test` runs as a Windows/Ubuntu matrix and executes the complete workspace
   suite in debug and release modes.

The stable Rust toolchain declared by the repository remains authoritative.
Cargo registry, Git database, and build artifacts will be cached per operating
system. `cargo-deny` will be installed only in the quality job. Live Codex
contract tests remain ignored because CI has no interactive ChatGPT login and
must not receive credentials.

## Failure behavior

Each command is a separate named step so GitHub identifies the failing gate.
No step may weaken or bypass failures. The workflow will not upload logs,
databases, credentials, or other runtime artifacts.

## Acceptance criteria

- The root README gives a new contributor a correct path from clone to a local
  M0 run and safe uninstall.
- README commands and claims match the current CLI and operations docs.
- CI YAML parses successfully and declares both Windows and Ubuntu test jobs.
- CI runs format, check, Clippy, dependency policy, debug tests, and release
  tests.
- The local workspace remains formatted and all default tests pass.
- No API key, Codex credential, or live-test secret is introduced.

## Out of scope

- Release packaging or binary publication;
- code coverage services;
- macOS CI;
- live Codex tests in GitHub Actions;
- changes to M0 runtime behavior.
