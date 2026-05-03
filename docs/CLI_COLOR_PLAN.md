# CLI color output

## Purpose

`tama` and `tamaup` currently emit plain text for every progress row, doctor
check, audit finding, clean entry, and error. Status tags such as `ok`, `run`,
`skip`, `fail`, `warn`, severity labels, and `error:` prefixes carry meaning that
should be visible at a glance. This plan introduces a single styling layer
shared across both CLIs that colors those meaning-bearing tokens, leaves the
surrounding text uncolored, and is suppressed automatically when output is not
a terminal, when `--json` is requested, when `--no-color` is passed, or when the
environment requests no color.

## Steps

1. Add `anstream` and `anstyle` to `[workspace.dependencies]` in
   `/home/user/tama/Cargo.toml` (both already appear transitively in
   `Cargo.lock` via clap, so no new third-party crates enter the dep tree).
   Pin to the lockfile versions: `anstream = "0.6"`, `anstyle = "1.0"`. Add
   them as direct dependencies of `tama-cli` and `tamaup-cli`. Do not add
   `colored`, `owo-colors`, `termcolor`, `nu-ansi-term`, or any other styling
   crate.

2. Create `/home/user/tama/crates/tama-cli/src/style.rs` (declared from
   `main.rs` via `mod style;`). It owns:
   - A `ColorChoice` enum (`Auto`, `Always`, `Never`) and a
     `pub fn resolve(choice: ColorChoice, json: bool, stream: Stream) -> bool`
     helper that returns whether the given stream should be styled. `Stream`
     is `Stdout` or `Stderr`. Logic, evaluated in order: `json` → `false`;
     `Always` → `true`; `Never` → `false`; `Auto` → `anstream::AutoStream`'s
     decision for the matching stream (which already honors `NO_COLOR`,
     `CLICOLOR`, `CLICOLOR_FORCE`, and TTY detection).
   - A `Palette` struct holding `anstyle::Style` values for every named role:
     `ok`, `run`, `skip`, `fail`, `warn`, `info`, `error_prefix`,
     `warning_prefix`, `header`, `dim`, `path`, `count`, `severity_error`,
     `severity_warning`, `severity_info`. Concrete styles:
     - `ok`, `severity_info` → green, bold
     - `run`, `info` → cyan, bold
     - `skip`, `dim` → dim
     - `fail`, `error_prefix`, `severity_error` → red, bold
     - `warn`, `warning_prefix`, `severity_warning` → yellow, bold
     - `header` → bold
     - `path` → cyan
     - `count` → bold
   - `Palette::new(enabled: bool) -> Self`: when `enabled` is false every field
     is `anstyle::Style::new()` (the empty style, which renders no escape
     codes). All call sites use the same paint API regardless of enablement.
   - `pub fn paint(style: anstyle::Style, value: impl fmt::Display) -> impl fmt::Display`
     wrapper that emits `{style}{value}{style:#}` so callers can drop styled
     fragments inline with `format!`/`write!`.

3. Replace the existing boolean `--no-color` flag on `Cli` in
   `/home/user/tama/crates/tama-cli/src/main.rs` (lines 150–151) with a
   `--color <when>` global argument (`auto` default, accepts `auto|always|never`)
   plus a hidden `--no-color` alias kept for backwards compatibility that maps
   to `--color=never`. Use clap's `ValueEnum` derive on `style::ColorChoice` so
   `--help` enumerates the values. Drop the `std::env::set_var("NO_COLOR", ...)`
   call at line 314–316; routing happens through the resolved `Palette`
   instead.

4. In `tama-cli` `main`, after parsing, build two palettes once:
   `let stdout_palette = Palette::new(style::resolve(cli.color, cli.json, Stdout));`
   and the same for `Stderr`. Thread `stdout_palette` into `run` (via a new
   field on a `RunContext` struct that already wraps `cli` plus the palette,
   or by passing `(cli, palette)` as a tuple). Pass `stderr_palette` to the
   error printer in `main` so `error: <msg>` uses
   `palette.error_prefix.paint("error:")`.

5. Replace every `println!` writing user-facing prose in
   `/home/user/tama/crates/tama-cli/src/main.rs` with `anstream::println!` (and
   the same for `eprintln!` → `anstream::eprintln!`). This keeps automatic
   stripping when the destination is not a TTY even if a caller forgets to
   use the palette. The 24 print sites listed by
   `grep -nE '(println!|eprintln!|print!|eprint!)' crates/tama-cli/src/main.rs`
   are the full set; convert all of them. JSON-emitting branches (lines 499,
   522, 562–565, 655–658) stay unchanged in content but still go through
   `anstream::println!` (which leaves their bytes unchanged because the
   palette already disabled colors when `cli.json` is true).

6. Style `CommandProgress` output in `/home/user/tama/crates/tama-cli/src/main.rs`
   (struct at lines 331–391, formatter at lines 393–395). Carry the
   `Palette` on the struct; `CommandProgress::new(enabled, palette)` already
   has the palette ready. Update the per-status mapping inside `line` so that
   the four-character status tag is painted from the palette
   (`run` → `palette.run`, `ok` → `palette.ok`, `skip` → `palette.skip`,
   `fail` → `palette.fail`). Leave the name and detail columns uncolored.
   Inside `scope`, paint the title (`{title}:`) with `palette.header` and the
   row labels (`label`) with `palette.dim`; values stay plain. The `Steps:`
   header at line 350 is also painted with `palette.header`.

7. Style the doctor formatter in
   `/home/user/tama/crates/tama-cli/src/main.rs` `format_doctor_report`
   (lines 857–919). Take `&Palette` as a parameter. Apply:
   - The `Checks:` and `Notes:` section headers → `palette.header`.
   - Per-tool status tags: `ok` (lines 866–871) → `palette.ok`; the two
     `fail` paths (lines 872–886) → `palette.fail`; the lock `ok`/`fail`
     pair (lines 891, 894) → `palette.ok`/`palette.fail`.
   - Tool names (the `{:<15}` column) → `palette.header` so the eye finds the
     row first; the detail column stays plain so paths and version strings
     stay legible.
   - The trailing summary (`Doctor passed: …` / `Doctor passed after repair:
     …` / `Doctor found issues: …`) takes the matching status color:
     passed → `palette.ok`, found issues → `palette.fail`. The count fragments
     produced by `format_count` (line 913–917) use `palette.count`.
   The function still returns a `String`; styling is embedded as ANSI
   sequences in the returned string, which `anstream::println!` strips when
   the destination cannot render them.

8. Style the audit formatter in
   `/home/user/tama/crates/tama-cli/src/main.rs` `format_audit_report`
   (lines 2152–2227) and the helpers it depends on:
   - Take `&Palette` as a parameter.
   - `Audit scope:` and `Checks:` and `Findings (...):` headers →
     `palette.header`.
   - Per-check status tag from `audit_check_status` (lines 2269–2284): `ok` →
     `palette.ok`, `warn` → `palette.warn`, `info` → `palette.info`, `fail`
     → `palette.fail`. To keep `audit_check_status` returning
     `&'static str`, paint at the call site (line 2174) using the palette
     and the returned tag.
   - Per-issue rows in the loop at lines 2186–2200: paint the severity label
     (`severity_label`, line 2189) with the matching severity style
     (`Error` → `palette.severity_error`, `Warning` →
     `palette.severity_warning`, `Info` → `palette.severity_info`). Paint the
     `path:` key on the indented detail row (line 2198) with `palette.dim`.
   - The trailing line: `Audit failed:` → `palette.fail`,
     `Audit passed with warnings:` → `palette.warn`, `Audit passed:` →
     `palette.ok`. The numeric counts use `palette.count`.

9. Style `format_clean_report` in `/home/user/tama/crates/tama-cli/src/main.rs`
   (lines 2426–2452). Take `&Palette`. Per entry: `ok` tag (line 2437)
   → `palette.ok`, `skip` tag (line 2438) → `palette.skip`. The trailing
   `Clean completed:` line uses `palette.header` for the prefix and
   `palette.count` for the numeric fragments.

10. Style the one-line completion messages emitted from `run` in
    `/home/user/tama/crates/tama-cli/src/main.rs` (lines 434, 466, 501,
    524–528, 689, 717, 760). Each follows the form
    `<verb> <object>: <detail>`. Wrap the leading verb-and-object
    (`Initialized Tama ERC20Lite starter`, `Created Verity contract scaffold`,
    `Check completed`, `Build completed`, `Installed Tama dependency`,
    `Removed Tama dependency`, `Updated Tama project lock state`) in
    `palette.ok`. Wrap the trailing path/name/count fragment in `palette.path`
    or `palette.count` as appropriate (path for init/new, count for build,
    name for install/remove). The `warning:` prefix at line 1636 uses
    `palette.warning_prefix`.

11. Style the `error:` prefix in `tama-cli` `main` at line 325 with
    `stderr_palette.error_prefix`. The error body stays plain so multi-line
    error messages from downstream crates render unchanged.

12. Mirror the layer in `tamaup-cli`:
    - Add a `mod style;` referencing
      `/home/user/tama/crates/tamaup-cli/src/style.rs`. The simplest path is to
      copy the `Palette` and `resolve` definitions verbatim — the layer is
      ~80 LOC and a shared crate is not justified for two CLIs. If a third
      consumer appears later, lift then.
    - Add the same `--color <when>` global to `Cli` in
      `/home/user/tama/crates/tamaup-cli/src/main.rs`.
    - Replace the five print sites (lines 135, 215, 400, 438, 440) with
      `anstream::eprintln!`/`anstream::println!`.
    - Paint the `error:` prefix at line 135 with `palette.error_prefix`. Paint
      `Installed Tama` (line 215) with `palette.ok` and the version with
      `palette.count`. Paint `Active Tama version:` (line 400) with
      `palette.header` and the version with `palette.count`. In the version
      list (lines 437–440), paint the `*` marker on the active version with
      `palette.ok` and the active version name with `palette.header`; inactive
      rows stay plain.

13. Update test fixtures in
    `/home/user/tama/crates/tama-cli/tests/doctor.rs` (and any other test
    asserting on stdout/stderr) to either run the binary with `--color=never`
    or to assert against the post-strip output. Use `anstream::strip_str` if a
    test needs to assert against painted output.

14. Update `/home/user/tama/README.md` and any reference docs under
    `/home/user/tama/docs/reference/` that document `--no-color` to also
    document `--color <auto|always|never>`, `NO_COLOR`, and `CLICOLOR_FORCE`.
    Note that `--json` always disables color.

## End state

- `--color <auto|always|never>` is accepted as a global option on both `tama`
  and `tamaup`; `--no-color` continues to work as a hidden alias for
  `--color=never`.
- With `--color=auto` (the default) and a TTY destination, status tags
  (`ok`, `run`, `skip`, `fail`, `warn`, `info`), severity labels (`error`,
  `warning`, `info`), section headers, path fragments, count fragments, and
  the `error:` / `warning:` prefixes render in color across `tama init`,
  `tama new`, `tama check`, `tama build`, `tama audit`, `tama inspect`,
  `tama clean`, `tama doctor`, `tama install`, `tama remove`, `tama update`,
  and all `tamaup` subcommands.
- With `--color=never`, `--json`, `NO_COLOR=1`, or a non-TTY destination, no
  ANSI escape sequences appear in either stream.
- With `CLICOLOR_FORCE=1` and `--color=auto`, color is emitted even when the
  destination is not a TTY.
- `cargo run -p tama -- audit --json | jq` and
  `cargo run -p tama -- inspect <C> abi --json | jq` both succeed without
  preprocessing, because JSON branches never receive escape sequences.
- `Cargo.lock` gains no new third-party packages; `anstream` and `anstyle`
  were already present transitively.
- `crates/tama-cli/src/style.rs` and `crates/tamaup-cli/src/style.rs` exist
  and centralize every named style; no other file uses raw ANSI escapes or
  imports `anstyle::AnsiColor` directly.
- `cargo test --workspace` is green; doctor and any other CLI tests run
  against `--color=never` output.
- `cargo build --workspace` and `cargo clippy --workspace --all-targets
  --all-features` are green.
