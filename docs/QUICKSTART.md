# Quickstart

Install Tama, create the ERC20Lite starter, and run the full pipeline:

```sh
curl -L https://tama.tools/install.sh | sh
tama init my-protocol
cd my-protocol
tama doctor
tama check
tama build
tama test
tama audit
tama inspect ERC20Lite selectors
```

Use `tamaup install <version>` later to switch to a specific signed release.

The starter project is a Foundry project with Verity source, spec, proof, generated Solidity bridge files, Yul, bytecode, and manifests. It should pass without hand-editing files on a supported machine with the declared toolchain.

Create another contract scaffold:

```sh
tama new TipJar
tama check
```

New contract scaffolds may require the user to fill in proofs before `tama audit` passes. They must not break the existing ERC20Lite starter.
