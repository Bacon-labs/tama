# Yul optimizer steps

## Purpose

Allow projects to select solc's Yul optimization sequence for generated contracts while preserving compiler defaults and reproducible lockfiles when the option is omitted.

## Steps

1. Decision 1: In `crates/tama-config/src/lib.rs`, add optional `yul_optimizer_steps: Option<String>` to `YulConfig`, defaulting to `None` and omitted from serialization when absent. Preserve supplied strings verbatim, including empty strings; solc owns sequence validation. Include the key in Yul lock entries only when present. Test absent, explicit, empty, wrong-type, round-trip, and lock drift/removal behavior.
2. Decision 2: In `crates/tama-build/src/lib.rs`, emit supplied strings at `settings.optimizer.details.yulDetails.optimizerSteps` in the existing direct standard-JSON path. Omission must preserve the exact current optimizer object. Test explicit sequences, empty strings, and absence. Add `None` only to existing test `YulConfig` literals in this file and `crates/tama-audit/src/lib.rs`, `crates/tama-cli/src/main.rs`, and `crates/tama-project/src/lib.rs`. Change no Foundry invocation, default optimizer, starter template, or dependency.
3. Decision 3: Document the optional key, verbatim pass-through, solc validation, compiler defaults, empty-string semantics, lock behavior, and separation from Foundry in `site/src/pages/reference/config.mdx` and `docs/reference/SPEC.md`. Bump the workspace version from `0.1.7` to `0.1.8` in `Cargo.toml` and regenerate only workspace package versions in `Cargo.lock`.
4. Decision 4: Run formatting, workspace tests, strict workspace Clippy, release build, version validation, documentation build, and a real pinned-solc smoke check of explicit/default sequences and invalid-sequence rejection. Review the complete diff, remove this executed plan from the final PR tree, and open a minimal signed-commit PR against `main` with Summary, Changes, and Verification sections. Preserve untracked `AGENTS.md` and unrelated work. Do not tag or publish a release.

## End state

- Explicit Yul sequences reach solc and are recorded in the lockfile.
- Omitted sequences preserve existing compiler input and lock entries.
- Documentation matches behavior; workspace packages report `0.1.8`.
- The PR contains only configuration support, regression tests, documentation, and version metadata.
