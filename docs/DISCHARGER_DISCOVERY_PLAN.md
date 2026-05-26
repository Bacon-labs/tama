# Discharger Discovery Plan

## Purpose

`tama build` resolves each obligation's dischargers by regex-scanning the single
`{Contract}Proof.lean` file for `-- tama: discharges=` comments. When a proof is
split across submodules, the tags live in the imported sub-files and the
top-level file is tag-less, so Tama records zero dischargers for the contract and
`manifest.validate()` fails with "requires at least one discharger". This plan
replaces the single-file comment scan with namespace + type-based discovery via
the elaborated Lean environment, so dischargers are found regardless of how the
proof is split across files.

## Steps

1. In `crates/tama-build/src/lib.rs`, extract the trust probe's target-shape
   helpers (`tamaZetaHead`, `tamaFinalTheoremTarget`, `tamaTargetContainsSpec`)
   from `collect_axioms_probe_source` into a single `const TAMA_TARGET_HELPERS:
   &str`, and reference that const from `collect_axioms_probe_source` so the
   discovery pass and the trust probe share one definition of the discharge
   criterion.
2. Add `discover_dischargers(root, config, manifests: &mut [ContractManifest])`
   in `crates/tama-build/src/lib.rs`. It writes a generated Lean file to
   `artifacts/.../dischargers/Discover.lean`, runs `lake env lean <file>` from
   `root` (same invocation shape as `generate_trust_probe`), parses the emitted
   markers, and fills `obligation.dischargers` (sorted, de-duplicated) on each
   manifest, then rewrites each manifest JSON.
3. Implement `discover_dischargers_source(manifests)` returning the generated
   Lean: `import Lean`, `import <each proof module>`, the shared
   `TAMA_TARGET_HELPERS`, and one `#eval` per contract that folds over
   `env.constants`, keeps `ConstantInfo.thmInfo` whose name has the proof
   module's namespace as a `Name`-prefix and is not `isInternal`, computes
   `tamaFinalTheoremTarget` once, and for each of that contract's spec decls
   emits `TAMA_DISCHARGER_JSON {"spec":"<spec lean_decl>","discharger":"<thm>"}`
   when `tamaTargetContainsSpec` holds.
4. Implement `parse_discharger_output(stdout, manifests)` that reads the
   `TAMA_DISCHARGER_JSON` markers into a `spec lean_decl -> BTreeSet<discharger>`
   map. Error (`Error::Adapter`) if any obligation resolves to zero dischargers,
   naming the spec `lean_decl` and the proof module, and stating that a
   discharger must conclude the spec with no extra hypotheses.
5. In `adapt_verity_outputs`, delete the `extract_dischargers` call and pass no
   discharger map to `merge_obligations`; obligations are built with empty
   `dischargers`. Remove `extract_dischargers` and drop the `dischargers`
   parameter from `merge_obligations`.
6. In `Pipeline::run` (`crates/tama-build/src/lib.rs`), add a `dischargers`
   progress step between `manifest` and `trust-probe` that calls
   `discover_dischargers(&self.root, &config, &mut manifests)`.
7. Remove the now-dead tag machinery and tests: the `extract_dischargers_*`
   unit tests and the direct `extract_dischargers` assertions; change
   `adapter_accepts_nested_verity_files_and_mirror_tests` to assert
   `dischargers` is empty (population is a separate Lean step). Add unit tests
   for `discover_dischargers_source` (generated Lean string) and
   `parse_discharger_output` (synthetic marker stdout, including the
   zero-discharger error).
8. In `fixtures/projects/counter/verity/proof/`, move two theorems into a new
   `CounterProofParts.lean` declaring `namespace proof.CounterProof`, have
   `CounterProof.lean` `import proof.CounterProofParts`, and remove every
   `-- tama: discharges=` comment from both files, so the counter e2e exercises
   split + tag-free discovery.
9. In `crates/tama-project/src/lib.rs`, remove the `-- tama: discharges=`
   comments and the comment teaching them from the starter proof templates;
   leave the theorems unchanged.
10. Update docs that teach the tag: `site/src/pages/{start,concepts}.mdx`,
    `site/src/pages/guides/{proofs,starter,audits}.mdx`,
    `site/src/pages/reference/{cli,manifest,artifacts}.mdx`, and
    `docs/reference/SPEC.md`. State that dischargers are auto-discovered by type
    from the proof namespace; keep the accepted/rejected target-shape criterion;
    drop instructions to author `discharges=` tags.
11. Bump the workspace `version` in `Cargo.toml` from `0.1.5` to `0.1.6`.

## End state

- A split-across-files proof builds: every obligation records at least one
  discharger discovered from the proof namespace, with no `-- tama: discharges=`
  tags present.
- `extract_dischargers` and all `-- tama: discharges=` parsing are gone from
  `crates/tama-build/src/lib.rs`; leftover tags in user proofs are inert.
- `discover_dischargers` runs one `lake env lean` pass over the proof
  namespaces, fills `obligation.dischargers` (sorted), and errors with a
  spec-and-namespace message when a spec has no qualifying theorem.
- The discharge criterion (theorem target is the spec application or a positive
  conjunction containing it, after stripping non-Prop binders) is defined once
  and shared by discovery and the trust probe.
- `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  and `cargo test --workspace` pass; the counter fixture passes `tama build`
  end to end with a real Lean toolchain.
- The workspace builds as `tama 0.1.6`.
