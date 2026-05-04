# Spec-as-Obligation Plan

## Purpose

Today, a Lean theorem only becomes a tracked obligation if the author writes a `-- tama: obligation …` comment above it in `verity/proof/<Contract>Proof.lean`. Deleting the comment silently removes the audit's coverage requirement, so a contract can pass `tama audit` while the property has neither a proof nor a Foundry mirror. This plan reshapes the model so each definition in `verity/spec/<Contract>Spec.lean` *is* the obligation, and the spec file is pure Lean with no Tama tags. Two kinds of evidence discharge an obligation, declared symmetrically next to the evidence: a Lean theorem in the proof file carries `-- tama: discharges=<spec>`, and a Solidity fuzz/invariant test carries `// tama: mirrors=<spec>`. Specs with no executable mirror are listed in `tama.toml` under `[coverage.proof_only]` with a required reason. The spec file becomes the single source of truth for what the contract must do; proof and mirror declare what they witness.

## Decisions

1. **Decision 1 — Every top-level declaration in the spec module is an obligation.** `verity/spec/<Contract>Spec.lean` is an obligations-only file: every top-level `def <name> …` and every `#gen_spec <name> …` invocation in it becomes one entry in `manifest.obligations`. No naming convention and no `-- tama:` tag is required to register a spec — physical location is the registration. The build rejects the file if it contains any other top-level form besides `import`, `namespace`, `open`, `def`, `#gen_spec`, and the closing `end` of a `namespace` block. Helpers (auxiliary defs, lemmas, abbreviations) must live outside this file — typically in `Verity.Specs.Common` or a sibling module imported by the spec file.

2. **Decision 2 — The spec file is pure Lean: no Tama tags.** Spec files do not carry `-- tama:` comments at all. All Tama metadata (what discharges this spec, what mirrors it, whether it's exempt from mirroring) lives next to the evidence or in `tama.toml`, never on the spec itself.

3. **Decision 3 — Theorems declare what they discharge.** Each obligation theorem in `verity/proof/<Contract>Proof.lean` carries `-- tama: discharges=<spec_name>` on the line above (e.g. `discharges=transfer_total_supply_preserved`). Multiple theorems may discharge the same spec; a theorem may carry multiple `discharges=` entries (comma-separated) to discharge several specs. Theorems and `lemma`s without a `discharges=` tag are treated as helpers and are not tracked.

4. **Decision 4 — Foundry tests declare what they mirror.** Each fuzz/invariant test in the project's Foundry test tree carries `// tama: mirrors=<spec_name>` on the line above the function declaration. Multiple tests may mirror the same spec; a test may carry multiple `mirrors=` entries (comma-separated). Spec names in `mirrors=` are bare (e.g. `mirrors=transfer_total_supply_preserved`); the contract is inferred from the spec lookup at build time. A `mirrors=<X>` referencing an unknown spec is a build error. Solidity functions without a `mirrors=` tag are ordinary tests and are not tracked. Mirror functions must be named `testFuzz*` or `invariant_*`; named otherwise, the build rejects with "mirror tag on non-property-shaped test".

5. **Decision 5 — `proof_only` exemptions live in `tama.toml`.** A new section `[coverage.proof_only]` in `tama.toml` lists obligations that have no executable mirror, keyed by fully qualified obligation id (`<Contract>.<spec_name>`), with a non-empty reason string as value. An obligation listed here is not required to have any `mirrors=` tag in the test tree; an obligation NOT listed here must have at least one mirror. Empty reason is a config-load error; an entry whose key does not match any known obligation is a build error.

6. **Decision 6 — Manifest obligation entry shape.** Each entry has fields: `id` (`<Contract>.<spec_name>`), `name` (spec def name), `lean_decl` (`spec.<Contract>Spec.<spec_name>`), `contract`, `dischargers: Vec<String>` (fully qualified proof-theorem decls), `mirrors: Vec<String>` (each `<file>:<sol_contract>.<sol_function>`), and `proof_only_reason: Option<String>` (set iff this id appears in `[coverage.proof_only]`). `kind`, `function`/`functions`, and `coverage` are gone. The old `lean_decl` (proof-theorem name) becomes the spec decl; theorem decls move into `dischargers`; mirror paths move from a singleton `coverage.path` into the plural `mirrors`.

7. **Decision 7 — `theorems` CLI field is renamed to `specs`.** `Field::Theorems` and the `"theorems"` parse_field arm are deleted; a new `Field::Specs` and `"specs"` arm replace them. The CLI help, the `cli.mdx` reference, and the SPEC.md reference all update accordingly. No alias — clean break.

8. **Decision 8 — The trust-probe artifact JSON is internal and changes shape.** Old: `{"obligations": [{"lean_decl": <theorem>, "axioms": [...]}]}`. New: `{"obligations": [{"lean_decl": <spec_decl>, "dischargers": [{"lean_decl": <theorem>, "axioms": [...]}]}]}`. The artifact is regenerated by `tama build` on every run, lives only under `artifacts/trust-probe/`, and has no schema file or external consumer to update.

9. **Decision 9 — Build owns intra-input consistency; audit owns cross-artifact consistency.** Build catches errors a developer can fix by editing files under `verity/`, `test/verity/`, or `tama.toml`. Audit catches errors that require regenerating an elaboration or compilation artifact. There is no overlap; audit assumes the manifest is well-formed and asks whether the rest of the project agrees with it. Concretely:
   - **Build (`tama build`)** rejects: spec module containing forbidden top-level forms; `discharges=<X>` or `mirrors=<X>` referencing an unknown spec; obligation with empty `dischargers`; obligation with empty `mirrors` and not listed in `[coverage.proof_only]`; mirror tag on a function not named `testFuzz*` / `invariant_*`; `[coverage.proof_only]` entry whose id does not match any known obligation; `[coverage.proof_only]` entry with empty reason.
   - **Audit (`tama audit coverage`)** rejects: any `mirrors[]` path whose Solidity file or symbol cannot be resolved (file deleted, symbol renamed, parse failure). Logic adapts the existing path resolution at [crates/tama-audit/src/lib.rs:1163-1346](crates/tama-audit/src/lib.rs#L1163-L1346) to iterate over `obligation.mirrors` rather than a singleton `coverage.path`.
   - **Audit (`tama audit trust-boundary`)** rejects: any entry of `obligations[i].dischargers` missing from the trust-probe artifact (replaces the existing `lean_decl`-vs-probe check at [crates/tama-audit/src/lib.rs:1416-1438](crates/tama-audit/src/lib.rs#L1416-L1438)); any reachable axiom that is denied or unallowlisted; any reachable `sorry`. Walks axioms reachable from each discharger and aggregates them per spec.

## Steps

### Manifest + schema

1. In [crates/tama-manifest/src/lib.rs](crates/tama-manifest/src/lib.rs):
   - Delete the `ObligationKind` enum (lines 142–148) and all uses.
   - Delete the `Coverage` and `CoverageDisposition` types (lines 150–164) — no longer needed at the manifest layer.
   - Replace the `Obligation` struct (lines 130–140) with `{ id, name, lean_decl, contract, dischargers: Vec<String>, mirrors: Vec<String>, proof_only_reason: Option<String> }`.
   - In `validate_obligation` (lines 440–566): drop the kind/coverage branches; require `dischargers` to be non-empty and each entry to be a fully qualified Lean name (existing `is_qualified_lean_name`); require either `mirrors` non-empty OR `proof_only_reason` set with non-empty trimmed text (XOR: setting both is an error — a proof-only obligation should not also list mirrors); validate that each `mirrors[]` entry parses as `<path>:<sol_contract>.<sol_function>` with the function name matching `^(testFuzz|invariant_)[A-Za-z0-9_]*$` (reuse the existing `mirror_symbol_is_property` helper from audit, lifted into a shared location, or duplicate inline).
   - Update test fixtures in this file (lines 780–1010+) to the new shape.

2. In [schemas/tama.contract-manifest.v1.schema.json](schemas/tama.contract-manifest.v1.schema.json):
   - Remove `kind`, `function`, and `coverage` from the `obligation` definition (lines 305–344).
   - Add to `obligation.required`: `dischargers`, `mirrors`. (Optional: `proof_only_reason`.)
   - Add `dischargers`: array of `#/$defs/leanDecl`, `minItems: 1`.
   - Add `mirrors`: array of strings matching the existing mirror-path regex (move the regex out of the `mirrorPath` $def at lines 296-298 into a top-level `$defs/mirrorPath` and reference it).
   - Add `proof_only_reason`: nullable string with `minLength: 1` when non-null.
   - Add an `oneOf` constraint at the obligation level: either `mirrors` is non-empty AND `proof_only_reason` is null, OR `mirrors` is empty AND `proof_only_reason` is a non-empty string.
   - Delete the `coverage` $def entirely.

### Config

3. In [crates/tama-config/src/lib.rs](crates/tama-config/src/lib.rs):
   - Add a `CoverageConfig` struct: `pub struct CoverageConfig { pub proof_only: BTreeMap<String, String> }` with `#[serde(default)]`.
   - Add a `coverage: CoverageConfig` field to `TamaConfig` (alongside `trust` at line 52), `#[serde(default)]`.
   - On load (function `load_config` or equivalent), validate that every value is a non-empty trimmed string. Empty value → config-load error pointing at the offending key.
   - Add a unit test parsing a `tama.toml` with a `[coverage.proof_only]` table; assert the entries round-trip.

### Build pipeline

4. In [crates/tama-build/src/lib.rs](crates/tama-build/src/lib.rs):
   - Add `extract_specs(root, config, contract, spec_module)` that reads `verity/spec/<Contract>Spec.lean`, enumerates every top-level `def <name> …` and `#gen_spec <name> …` invocation, and returns `Vec<String>` (spec names). Block-comment stripping reuses `strip_lean_block_comments`. Reject the file with a build error if it contains any other top-level form besides `import`, `namespace`, `open`, `def`, `#gen_spec`, and the matching `end <namespace>` (whitespace-only and `--` comment lines are ignored). The spec file no longer carries `-- tama:` comments — any such comment in the spec file is rejected.
   - Add `extract_dischargers(root, config, contract, proof_module)` that reads `<Contract>Proof.lean`, recognises `theorem|lemma` declarations (existing regex at line 1399), parses `-- tama: discharges=<name>[,<name>]` above each, and returns `HashMap<spec_name, Vec<theorem_lean_decl>>`. Reject `discharges=<name>` where `<name>` does not appear in the spec list with a build error citing the proof file path and theorem name.
   - Add `extract_mirrors(root, foundry_test_dir, known_specs_by_contract)` that scans every `*.t.sol` file under the Foundry test directory, recognises `function (testFuzz[A-Za-z0-9_]*|invariant_[A-Za-z0-9_]*)\s*\(` declarations with a `// tama: mirrors=<name>[,<name>]` comment on a line directly above, and returns `HashMap<(contract, spec_name), Vec<MirrorPath>>` where `MirrorPath` is `<file>:<sol_contract>.<sol_function>` (the file is the test source path relative to project root; `sol_contract` is the enclosing `contract NAME { … }` block, found by scanning backwards from the function). The known-specs map is `HashMap<spec_name, contract>` (built from `extract_specs` outputs across all contracts in the project) used to resolve which obligation each mirror belongs to. Reject a `// tama: mirrors=<X>` whose `<X>` is not in the known-specs map with a build error citing the test file and function. Reject a `// tama: mirrors=` comment above a function whose name is not `testFuzz*` or `invariant_*` (it's a non-property-shaped test). Spec-name collisions across contracts (same spec name in two contracts) are a build error: mirror tags can't disambiguate.
   - Replace the call to `extract_obligations` at [line 569](crates/tama-build/src/lib.rs#L569) with `merge_obligations(specs, extract_dischargers(...)?, mirrors_for_this_contract, &config.coverage.proof_only)`. Each obligation entry: `id = <Contract>.<spec_name>`, `name = spec_name`, `lean_decl = spec.<Contract>Spec.<spec_name>`, `contract`, `dischargers = dischargers_map.get(spec_name).cloned().unwrap_or_default()`, `mirrors = mirrors_map.get(spec_name).cloned().unwrap_or_default()`, `proof_only_reason = config.coverage.proof_only.get(&id).cloned()`. Manifest validation rejects any obligation where both `mirrors` is empty and `proof_only_reason` is `None`.
   - Validate `[coverage.proof_only]` keys: every key must match an obligation id produced in this build run; otherwise build error citing the unknown key.
   - Delete `extract_obligations` (lines 1387–1431), `ObligationMeta` (lines 1433–1464), `parse_obligation_metadata` (lines 1466–1477), `apply_tama_metadata` (lines 1479–1550), and `tama_metadata_key_supported` (lines 1552–1565). Replace with a small `parse_lean_tama_metadata` (recognising only `discharges=` above proof theorems) and a `parse_solidity_tama_metadata` (recognising only `mirrors=` above test functions). Both reject unknown keys.
   - Update test fixtures at lines 2386, 2390, 2394, 2398, 2453, 2474, 2495 to the new shape, and add new tests for: spec module containing a forbidden top-level form (e.g. a `lemma`) errors; theorem with `discharges=<unknown>` errors; mirror tag with `mirrors=<unknown>` errors; mirror tag on a `test_*` (non-property) function errors; obligation with no mirror and no `proof_only` entry fails manifest validation; `[coverage.proof_only]` key that doesn't match any obligation errors; `[coverage.proof_only]` entry with empty reason errors at config load; spec module with mirrors/dischargers all wired correctly produces a passing manifest.

5. The `extract_mirrors` call needs the FULL set of specs across all contracts in the project (so spec-name collisions can be detected). Restructure the build entry point at [crates/tama-build/src/lib.rs:540-595](crates/tama-build/src/lib.rs#L540-L595) so spec extraction runs for ALL contracts before any per-contract manifest is constructed; pass the global spec map to each per-contract `merge_obligations` call. (This is a small refactor of the per-contract loop into two passes: first specs-only, then full manifest construction.)

### Audit pipeline

6. In [crates/tama-audit/src/lib.rs](crates/tama-audit/src/lib.rs):
   - In the coverage check loop starting at [line 1163](crates/tama-audit/src/lib.rs#L1163), iterate over `obligation.mirrors` (was: singleton `obligation.coverage.path`). For each mirror string, run the existing path-resolution / file-exists / symbol-found / property-shape checks (lines 1184–1300+). Drop the `Coverage::ProofOnly` branch (no longer in the manifest); instead, the audit ignores obligations whose `mirrors` is empty (build already guaranteed those have a `proof_only_reason`).
   - Drop the `match obligation.kind` branch entirely (`Helper => continue` is dead — helpers are not in `obligations` any more).
   - Replace the trust-probe corroboration check at [lines 1416–1438](crates/tama-audit/src/lib.rs#L1416-L1438): today this checks that `obligation.lean_decl` is present in the trust-probe artifact's `seen_decls`. Rewrite to iterate over `obligation.dischargers` and check each entry against `seen_decls`. Error code stays `TAMA_TRUST_DECL_MISSING`; message: "discharger `<decl>` declared in manifest but not reported by the trust probe", probe path as the location. Drop the fully-qualified-name guard (the manifest schema enforces it).
   - In the trust-probe codegen path at [crates/tama-build/src/lib.rs:1696-1740](crates/tama-build/src/lib.rs#L1696-L1740): generate one `tamaAxiomJson` call per discharger across all obligations. Emit JSON in the new shape (Decision 8): `{"obligations": [{"lean_decl": <spec_decl>, "dischargers": [{"lean_decl": <theorem>, "axioms": [...]}, ...]}]}`. The audit-side parser at [crates/tama-audit/src/lib.rs:1496-1532](crates/tama-audit/src/lib.rs#L1496-L1532) reads the nested shape: outer loop over obligations, inner loop over dischargers; `seen_decls` is the union of discharger `lean_decl`s; the per-spec axiom union is computed by flattening `dischargers[].axioms`.
   - Update fixtures around lines 2248 and 2267 to match the new trust-probe JSON shape.

### Inspect, CLI

7. In [crates/tama-inspect/src/lib.rs](crates/tama-inspect/src/lib.rs):
   - Rename `Field::Theorems` → `Field::Specs` ([line 46](crates/tama-inspect/src/lib.rs#L46)). Update `parse_field` ([line 61](crates/tama-inspect/src/lib.rs#L61)) to map `"specs"` (drop `"theorems"`).
   - Update the projection arms at [line 101](crates/tama-inspect/src/lib.rs#L101) and [line 156](crates/tama-inspect/src/lib.rs#L156): `Field::Specs | Field::Obligations` (still serialise `manifest.obligations`; human-friendly headers say "spec" not "theorem").
   - Update the `Field::Mirrors` projection at [lines 102–106](crates/tama-inspect/src/lib.rs#L102-L106): change from `obligation.coverage.path.as_ref()` (singleton) to `obligation.mirrors.iter()` (flattened across all obligations).
   - Update tests at lines 357–360 and 398 to use the new obligation shape (no `kind`, no `coverage`, populated `dischargers` and `mirrors`).

8. In [crates/tama-cli/src/main.rs](crates/tama-cli/src/main.rs):
   - Update the audit-help string at [line 44](crates/tama-cli/src/main.rs#L44) from "Public obligations have Foundry mirrors or proof-only reasons" to "Every spec has a discharger and either a Foundry mirror or a `[coverage.proof_only]` entry".
   - Update the inspect-fields help block at [line 83](crates/tama-cli/src/main.rs#L83) by replacing the `theorems` line with `specs`.

### Project scaffold + fixtures

9. In [crates/tama-project/src/lib.rs](crates/tama-project/src/lib.rs):
   - Update `spec_template` (lines 448–467): remove all `-- tama:` comments. The spec file is pure Lean.
   - Update `proof_template` (lines 468–490): each theorem keeps a `-- tama: discharges=<spec_name>` tag; drop `function`/`kind`/`coverage`/`path` from proof-side comments entirely.
   - Update `test_template` (the Solidity test scaffold near lines 492–520): each `testFuzz*` function gains a `// tama: mirrors=<spec_name>` comment on the line directly above.
   - Update `ERC20LITE_SPEC_LEAN` (lines 600–626): remove all comments mentioning Tama. The header comment about coverage tracking should be deleted; the spec file is pure.
   - Update `ERC20LITE_PROOF_LEAN` (lines 628–698): replace each `-- tama: obligation kind=postcondition function=<f> coverage=mirror path=<…>` block with `-- tama: discharges=<spec_name>`. Header comment becomes "Each `tama: discharges=` comment binds a proof theorem to a spec."
   - Update `ERC20LITE_TEST_SOL` (the Solidity test source — search for it; emitted as a constant near `ERC20LITE_PROOF_LEAN`): each `testFuzz*` and `invariant_*` function gains a `// tama: mirrors=<spec_name>` comment above it. Map: `testFuzzMintUpdatesBalanceAndSupply` → `mirrors=mint_owner_preserved` (and possibly `totalSupply_spec` if it covers two), `testFuzzTransferPreservesTotalSupply` → `mirrors=transfer_total_supply_preserved`, `testFuzzBalanceOfMirrorsGeneratedBytecode` → `mirrors=balanceOf_spec`, `testFuzzDeploymentSetsOwner` → `mirrors=owner_spec`. Confirm the exact mapping while editing by reading the existing path values in the legacy proof tags.
   - Update tests at lines 961–962 to match the new comment shapes.
   - The starter project does not need a `[coverage.proof_only]` table by default; ERC20Lite has mirrors for every spec.

10. In [fixtures/projects/counter/verity/spec/CounterSpec.lean](fixtures/projects/counter/verity/spec/CounterSpec.lean): remove all `-- tama:` comments (none today, but confirm). The file is already pure Lean per the new model.

11. In [fixtures/projects/counter/verity/proof/CounterProof.lean](fixtures/projects/counter/verity/proof/CounterProof.lean): replace each `-- tama: obligation kind=… function=… coverage=… path=…` (lines 11, 26, 41, 48) with `-- tama: discharges=<spec_name>` (`increment_spec`, `decrement_spec`, `getCount_spec`, `getCount_preserves_state_spec` respectively).

12. In [fixtures/projects/counter/test/verity/Counter.t.sol](fixtures/projects/counter/test/verity/Counter.t.sol): add `// tama: mirrors=<spec_name>` above each `testFuzz*` and `invariant_*` function. Map (read from existing legacy tags in the proof file before editing it): `testFuzzIncrementUpdatesCount` → `mirrors=increment_spec`, `testFuzzDecrementUpdatesCount` → `mirrors=decrement_spec`, `testFuzzGetterMirrorsGeneratedBytecodeState` → `mirrors=getCount_spec`, `testFuzzGetterPreservesCount` → `mirrors=getCount_preserves_state_spec`.

13. Regenerate the Counter fixture manifest by running `tama build` after the build-pipeline edits land; commit the regenerated `fixtures/projects/counter/artifacts/manifest/Counter.json` and any sibling fixtures that update.

### Site

14. In [site/src/pages/concepts.mdx](site/src/pages/concepts.mdx): rewrite the `### Obligation` and `### Mirror test` sections (lines 106–124). New text: an obligation is a top-level definition in the spec file; the spec file is pure Lean; proof theorems carry `-- tama: discharges=<spec>`; Foundry tests carry `// tama: mirrors=<spec>`; specs without an executable mirror live in `tama.toml` under `[coverage.proof_only]`. Update the manifest-features bullet at line 134.

15. In [site/src/pages/start.mdx](site/src/pages/start.mdx): rewrite step 3 (lines 120–149) so the spec file is pure Lean — no Tama tags shown. Rewrite step 4 (lines 151–190) so proof theorems carry only `-- tama: discharges=<spec>`. Rewrite step 5 (the Foundry test, lines 192–222) so each `testFuzz*` / `invariant_*` function carries `// tama: mirrors=<spec>`. Update the explanatory paragraphs to reflect the symmetric model. Update step 7's audit description at lines 279–283 to "trust-boundary walks the dependency graph of every theorem that discharges a spec".

16. In [site/src/pages/guides/proofs.mdx](site/src/pages/guides/proofs.mdx): rewrite end-to-end. New page structure:
    - **What counts as an obligation** — every top-level def in the spec file. Pure Lean, no tags.
    - **Discharging via proof** — `-- tama: discharges=<spec>` above each theorem; multiple theorems per spec OK; lemmas without the tag are helpers.
    - **Discharging via mirror** — `// tama: mirrors=<spec>` above each Foundry fuzz/invariant test; multiple tests per spec OK.
    - **Proof-only obligations** — list under `[coverage.proof_only]` in `tama.toml` with a non-empty reason. Show a tama.toml example.
    - **`sorry` is fine until release** — same as today, just retargeted.
    - **Allowlisted axioms** — unchanged.
    Replace the existing examples with the new tag shapes throughout.

17. In [site/src/pages/guides/starter.mdx](site/src/pages/guides/starter.mdx): rewrite the "specification" section (lines 98–135) — show the spec file as pure Lean. Rewrite the "proofs" section (lines 136–177) — show theorems with only `discharges=`. Add a new "mirror tests" section (or extend the existing one) showing Foundry tests with `// tama: mirrors=<spec>` comments. Drop the bullet at lines 165–170 ("The `-- tama: obligation` annotation has three pieces") and replace with a paragraph on the symmetric model.

18. In [site/src/pages/guides/audits.mdx](site/src/pages/guides/audits.mdx): update lines 115 and 143. The `coverage` audit step now reads "every spec has at least one discharger AND either at least one mirror or a `[coverage.proof_only]` entry". The `trust-boundary` step reads "every spec's dischargers walk to allowlisted axioms only".

19. In [site/src/pages/guides/foundry.mdx](site/src/pages/guides/foundry.mdx): update line 85 and surrounding text. The mirror tag is now `// tama: mirrors=<spec>` directly above the Foundry test function. Add a short example showing a `testFuzz_*` with the comment.

20. In [site/src/pages/limitations.mdx](site/src/pages/limitations.mdx): update line 71 from "`sorry` in any `-- tama: obligation` theorem" to "`sorry` in any theorem that discharges a spec".

21. In [site/src/pages/reference/manifest.mdx](site/src/pages/reference/manifest.mdx): rewrite the `### obligations` section (lines 169–186). New table columns: `id`, `name`, `lean_decl` (spec decl), `contract`, `dischargers`, `mirrors`, `proof_only_reason`. Drop the `kind`, `function`, and `coverage` rows. Replace line 171 with "One entry per top-level definition in the spec file."

22. In [site/src/pages/reference/artifacts.mdx](site/src/pages/reference/artifacts.mdx): update line 90 to "reachable from each spec's dischargers".

23. In [site/src/pages/reference/cli.mdx](site/src/pages/reference/cli.mdx): replace `theorems` with `specs` in the inspect-fields list at line 238.

24. In [site/src/pages/reference/config.mdx](site/src/pages/reference/config.mdx): add a `[coverage.proof_only]` subsection under the `tama.toml` reference. Show the example shape (`"<Contract>.<spec_name>" = "reason"`) and document validation rules (key must match a real obligation id, value must be non-empty).

25. In [docs/reference/SPEC.md](docs/reference/SPEC.md): update lines 88, 112, 606, 621, 625, 629 to reflect the new model — spec files pure Lean, proofs carry `discharges=`, Foundry tests carry `mirrors=`, proof-only exemptions live in `tama.toml`, no `kind` enum, no proof-side `obligation` tag. Update line 680 to replace `theorems` with `specs` in the `inspect` field list. Add a small subsection describing `[coverage.proof_only]` if the spec doc enumerates `tama.toml` sections.

### Build green

26. Run `cargo test -p tama-manifest -p tama-build -p tama-audit -p tama-inspect -p tama-project -p tama-cli -p tama-config` and confirm all tests pass after fixture regeneration.
27. Run `tama build && tama audit` against `fixtures/projects/counter` and confirm both succeed. Then:
    - Delete a `mirrors=` tag from one Foundry test in `Counter.t.sol`. Confirm `tama build` fails (not just audit) with "obligation `Counter.<spec>` has no mirror and no `[coverage.proof_only]` entry".
    - Restore the tag, then add a fake `// tama: mirrors=nonexistent_spec` to a test. Confirm build fails with "mirrors=`nonexistent_spec` is not a known spec".
    - Restore, then add `[coverage.proof_only]\n"Counter.bogus_spec" = "x"` to `tama.toml`. Confirm build fails with "`Counter.bogus_spec` does not match any known obligation".
    - Restore, then add a `lemma foo : True := trivial` to `CounterSpec.lean`. Confirm build fails with "spec module contains forbidden top-level form `lemma`".

## End state

- `crates/tama-manifest/src/lib.rs` has no `ObligationKind`, no `Coverage`, no `CoverageDisposition`. `Obligation` carries `dischargers: Vec<String>`, `mirrors: Vec<String>`, `proof_only_reason: Option<String>`; the old `kind`, `function`, `coverage` fields are gone.
- `schemas/tama.contract-manifest.v1.schema.json` requires `dischargers` (minItems 1) and `mirrors` per obligation, allows optional `proof_only_reason`, encodes the XOR (mirrors-non-empty XOR proof_only-set) at the schema level.
- `crates/tama-config/src/lib.rs` has a `CoverageConfig { proof_only: BTreeMap<String, String> }` field on `TamaConfig`. Empty reasons fail at config load.
- `tama inspect <Contract> specs` is the new field name (no `theorems` alias).
- `tama build` reads spec files as pure Lean (any `-- tama:` comment in a spec file is rejected). It registers every top-level `def` / `#gen_spec` as an obligation. It parses `-- tama: discharges=<spec>` from proof files and `// tama: mirrors=<spec>` from Foundry test files, populating `obligations[i].dischargers` and `obligations[i].mirrors`. It cross-references `[coverage.proof_only]` from `tama.toml` and populates `proof_only_reason`.
- An obligation with no discharger fails the build. An obligation with no mirror AND no proof_only entry fails the build. `[coverage.proof_only]` keys that don't match an obligation id fail the build.
- A proof theorem without `discharges=` is silently treated as a helper. A Foundry test without `mirrors=` is silently treated as a smoke test. Neither appears in the manifest.
- `tama audit coverage` resolves every `mirrors[]` path to a real Solidity symbol of property shape (`testFuzz*` or `invariant_*`); it does not touch the trust-probe artifact.
- `tama audit trust-boundary` validates every entry of `obligations[i].dischargers` against the trust-probe artifact, walks axioms reachable from each, and aggregates per spec.
- `tama init` emits a starter project where `ERC20LiteSpec.lean` is pure Lean, `ERC20LiteProof.lean` carries only `discharges=` tags, and `ERC20Lite.t.sol` carries `// tama: mirrors=<spec>` above each property-shaped test.
- The Counter fixture is migrated: spec file is pure Lean, proof file uses `discharges=`, test file uses `mirrors=`, and the regenerated `Counter.json` manifest matches the new shape.
- All site pages (`concepts.mdx`, `start.mdx`, `guides/proofs.mdx`, `guides/starter.mdx`, `guides/audits.mdx`, `guides/foundry.mdx`, `limitations.mdx`, `reference/manifest.mdx`, `reference/artifacts.mdx`, `reference/cli.mdx`, `reference/config.mdx`) and `docs/reference/SPEC.md` describe the symmetric model. No site page contains a `-- tama: obligation`, `kind=`, or `coverage=mirror path=` example.
- `cargo test` is green across `tama-manifest`, `tama-build`, `tama-audit`, `tama-inspect`, `tama-project`, `tama-cli`, `tama-config`.
