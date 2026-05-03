# Starter CI plan

## Purpose

`tama init` scaffolds a complete buildable project — Verity Lean source, spec,
proof, mirror Foundry tests, generated bridge, deploy script, lakefile,
foundry.toml, and a `tama.lock` — but it writes no CI configuration. A user
who runs `tama init my-protocol`, pushes to GitHub, and opens a pull request
gets zero automated verification: a stale `tama.lock`, a broken proof, a
mutated artifact, or a failing Foundry mirror surface only when someone runs
`tama` locally. This plan extends the `init` template so every freshly
scaffolded project ships a `.github/workflows/ci.yml` that installs the
toolchain, builds with `--locked`, runs the Foundry mirrors, and runs the
audit suite on every push and pull request.

## Steps

1. **Create the workflow template at
   `crates/tama-project/src/templates/starter-ci.yml`.** Single workflow file,
   not a matrix. Top-level fields:

   - `name: CI`
   - `on: { pull_request: {}, push: { branches: [main] } }`
   - `permissions: { contents: read }`
   - `concurrency: { group: ci-${{ github.ref }}, cancel-in-progress: true }`
   - `env: { TAMA_LAKE_PACKAGE_CACHE: ${{ github.workspace }}/.cache/tama/lake-packages }`

   One job `verify` on `ubuntu-22.04` with these steps, in order:

   1. `actions/checkout@v4`.
   2. `actions/cache@v4` for `.cache/tama` keyed on
      `${{ runner.os }}-${{ runner.arch }}-${{ hashFiles('lake-manifest.json', 'tama.lock') }}`
      with a restore-keys prefix that drops the hash suffix.
   3. `actions/cache@v4` for `~/.elan` and `~/.foundry` keyed on
      `${{ runner.os }}-${{ runner.arch }}-toolchain-${{ hashFiles('lean-toolchain', 'tama.lock') }}`.
   4. Install elan with the retry-tolerant block copied from
      `.github/actions/install-toolchain/action.yml:26-39` — four attempts,
      `--default-toolchain none`, append `$HOME/.elan/bin` to `$GITHUB_PATH`.
   5. `foundry-rs/foundry-toolchain@v1.6.0` with
      `env: { GITHUB_TOKEN: ${{ github.token }} }`.
   6. solc-select 0.8.33 in a venv, identical to
      `.github/actions/install-toolchain/action.yml:46-59`.
   7. Install Tama: `curl -fsSL --retry 3 --retry-delay 2 --retry-connrefused
      https://tama.tools/install.sh | sh`, then append `$HOME/.tama/bin` to
      `$GITHUB_PATH`. Pin the install via `TAMA_VERSION` env defaulting to
      empty (latest); leave a comment in the file showing how to set it.
   8. `tama doctor`.
   9. `tama check`.
   10. `tama build --locked`.
   11. `tama test`.
   12. `tama audit`.

   The file lives in `templates/` alongside `starter-lake-manifest.json` so it
   is shipped with the crate via `include_str!` and is discoverable by anyone
   reading the templates directory.

2. **Add the `STARTER_CI_WORKFLOW` constant in
   `crates/tama-project/src/lib.rs`.** Put it next to the existing
   `STARTER_LAKE_MANIFEST` constant at line 15:

   ```rust
   const STARTER_CI_WORKFLOW: &str = include_str!("templates/starter-ci.yml");
   ```

3. **Wire the template into `init()` in
   `crates/tama-project/src/lib.rs`.** In the directory list at lines 77-93,
   add `".github/workflows"` so the parent directory exists. After the
   `write_string(&path.join("docs/README.md"), STARTER_README)?` call at line
   140, append:

   ```rust
   write_string(
       &path.join(".github/workflows/ci.yml"),
       STARTER_CI_WORKFLOW,
   )?;
   ```

4. **Generate `.gitignore` from `init()`.** The starter currently produces no
   `.gitignore`, so the first `git add` after `tama init` stages every build
   output. After the workflow write in step 3, also write `path.join(".gitignore")`
   with the contents of `fixtures/projects/counter/.gitignore`
   (`/.lake/`, `/artifacts/`, `/cache/`, `/lib/`, `/out/`, `foundry.lock`).
   Define a `STARTER_GITIGNORE` constant alongside `FOUNDRY_TOML` at line 517.

5. **Add a unit test
   `init_creates_github_actions_workflow` in the `mod tests` block of
   `crates/tama-project/src/lib.rs` (after
   `init_creates_erc20lite_starter_without_foundry_counter` at line 934).**
   The test calls `init`, reads `.github/workflows/ci.yml`, and asserts the
   file contains the literal substrings `name: CI`, `tama doctor`,
   `tama check`, `tama build --locked`, `tama test`, and `tama audit`. Add a
   second test `init_creates_gitignore` that asserts the `.gitignore`
   contains `/.lake/` and `/artifacts/`.

6. **Cover the workflow YAML structure with a parse test in the same `mod
   tests` block.** Add `serde_yaml = "0.9"` as a `dev-dependencies` entry in
   `crates/tama-project/Cargo.toml`. Test
   `starter_ci_workflow_is_valid_yaml` runs
   `serde_yaml::from_str::<serde_yaml::Value>(STARTER_CI_WORKFLOW).unwrap()`
   and asserts the parsed root has `jobs.verify.steps` as a non-empty
   sequence. This catches indentation and quoting regressions on the embedded
   template before they ship.

7. **Update `STARTER_README` in `crates/tama-project/src/lib.rs:850-872` to
   document the workflow.** Add a final section:

   ```markdown
   ## Continuous integration

   `.github/workflows/ci.yml` runs `tama doctor`, `tama check`, `tama build
   --locked`, `tama test`, and `tama audit` on every push and pull request.
   The first run installs Lean (elan), Foundry, solc 0.8.33, and Tama; later
   runs reuse caches keyed on `lake-manifest.json` and `tama.lock`.
   ```

8. **Extend the `real-e2e` smoke check at
   `.github/workflows/ci.yml:69-99`.** After `tama init "$project"`, assert
   the generated workflow exists and parses:

   ```bash
   test -f "$project/.github/workflows/ci.yml"
   python3 -c "import sys, yaml; yaml.safe_load(open(sys.argv[1]))" \
     "$project/.github/workflows/ci.yml"
   ```

   Install `python3-yaml` (or `pip install pyyaml` in a temp venv) earlier in
   the job. This guards against the in-process YAML parse test (step 6)
   missing template substitutions a future template-parameterization step
   might introduce.

9. **Add an `actionlint` lint of the generated workflow in the same
   `real-e2e` job.** Run
   `curl -fsSL https://raw.githubusercontent.com/rhysd/actionlint/main/scripts/download-actionlint.bash | bash`
   then `./actionlint -color "$project/.github/workflows/ci.yml"`. Pin the
   actionlint version via the script's `INSTALL_VERSION` env to the latest
   tagged release at the time of plan execution.

10. **Update `crates/tama-project/src/lib.rs` doc-comment at the top of the
    file** so the list of files `init` produces names `.github/workflows/ci.yml`
    and `.gitignore`. The file currently has no module-level doc; add one
    that enumerates the generated tree so future contributors do not need
    to read `init()` line by line.

## End state

- `crates/tama-project/src/templates/starter-ci.yml` exists and is included
  via `include_str!`.
- Running `tama init <path>` on an empty directory creates
  `<path>/.github/workflows/ci.yml` and `<path>/.gitignore`.
- The generated workflow file declares the steps `tama doctor`,
  `tama check`, `tama build --locked`, `tama test`, and `tama audit` in that
  order, on a single `ubuntu-22.04` job.
- The generated workflow installs elan, Foundry, solc 0.8.33, and Tama, in
  that order, before the verification steps.
- `cargo test -p tama-project` exercises:
  - `init_creates_github_actions_workflow` (substring assertions)
  - `init_creates_gitignore`
  - `starter_ci_workflow_is_valid_yaml` (full YAML parse)
- `.github/workflows/ci.yml` `real-e2e` job runs `actionlint` and
  `yaml.safe_load` against the workflow generated by `tama init` in a fresh
  starter, and fails if either rejects it.
- `docs/README.md` inside a freshly initialized starter contains a
  "Continuous integration" section pointing at the workflow file.
- `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D
  warnings`, `cargo test --workspace`, and `cargo build --workspace
  --release` all pass on the branch.
