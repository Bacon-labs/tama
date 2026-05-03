# Audit

`tama audit` validates the project after a real build. Human output shows the project root, manifest directory, loaded contracts, each check that ran, and any findings. The command fails on error-severity issues and reports all findings it can collect.

Checks:

- `structure`: required files, aggregate imports, generated bridge headers, manifest artifact paths, deployer bytecode drift, and bytecode hash drift.
- `selectors`: function selectors, error selectors, event topic0, generated interface signatures, and generated Yul dispatcher cases.
- `storage-layout`: manifest storage declarations, duplicate slots, fixed-slot overlap, valid encodings, and compiler layout report drift for contracts with storage.
- `coverage`: every public invariant and postcondition has mirror or proof-only coverage; proof-only entries require a reason.
- Mirror coverage must point to a contract-qualified, property-shaped Foundry symbol such as `ERC20LiteTest.testFuzzTransferPreservesTotalSupply` or `ERC20LiteTest.invariant_totalSupplyTracksMinted`.
- Mirror coverage files must live under Foundry's configured test directory.
- `trust-boundary`: Lean axiom dependencies, `sorryAx`, unallowlisted axioms, unresolved declarations, and Verity trust-surface reports from `artifacts/trust-report.json` and `artifacts/assumption-report.json`.
- Trust allowlist entries in `[trust.allow_axioms]` must include a non-empty reason. `sorryAx` is hard-denied and cannot be allowlisted.
- Trust probes are generated with Lean's `collectAxioms` API and must emit `tama.trust-probe.v1` JSON.

Negative fixture coverage must include deleted files, corrupt selectors/topics, duplicate storage slots, missing mirrors, example-shaped mirror tests, empty proof-only reasons, unresolved Lean declarations, inserted `sorry`, inserted custom axioms, and hand-edited generated bridge files.

`--json` emits stable issue records for CI consumers.

Run one check with `tama audit <check>`. `storage` is accepted as an alias for
`storage-layout`, and `trust` is accepted as an alias for `trust-boundary`.
`--deny-warnings` treats warning-severity findings as failures.
