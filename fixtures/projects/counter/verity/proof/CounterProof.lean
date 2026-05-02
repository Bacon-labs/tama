import spec.CounterSpec
import Verity.Proofs.Stdlib.Automation

namespace proof.CounterProof

open Verity
open Verity.EVM.Uint256
open spec.CounterSpec
open src.Counter

-- tama: obligation kind=postcondition function=increment coverage=mirror path=test/verity/Counter.t.sol:CounterTest.testIncrementUpdatesCount
theorem increment_meets_spec (s : ContractState) :
  let s' := ((increment).run s).snd
  increment_spec s s' := by
  unfold increment_spec Specs.storageUpdateSpec Specs.sameAddrMapContext
  refine ⟨?_, ?_, ?_⟩
  · simp [increment, count, getStorage, setStorage, Contract.run, ContractResult.snd,
      Verity.bind, Bind.bind]
  · intro other h_neq
    simp [increment, count, getStorage, setStorage, Contract.run, ContractResult.snd,
      Verity.bind, Bind.bind, h_neq]
  · simp [Specs.sameStorageAddr, Specs.sameStorageMap, Specs.sameStorageArray,
      Specs.sameContext, increment, count, getStorage, setStorage, Contract.run,
      ContractResult.snd, Verity.bind, Bind.bind]

-- tama: obligation kind=postcondition function=decrement coverage=mirror path=test/verity/Counter.t.sol:CounterTest.testDecrementUpdatesCount
theorem decrement_meets_spec (s : ContractState) :
  let s' := ((decrement).run s).snd
  decrement_spec s s' := by
  unfold decrement_spec Specs.storageUpdateSpec Specs.sameAddrMapContext
  refine ⟨?_, ?_, ?_⟩
  · simp [decrement, count, getStorage, setStorage, Contract.run, ContractResult.snd,
      Verity.bind, Bind.bind]
  · intro other h_neq
    simp [decrement, count, getStorage, setStorage, Contract.run, ContractResult.snd,
      Verity.bind, Bind.bind, h_neq]
  · simp [Specs.sameStorageAddr, Specs.sameStorageMap, Specs.sameStorageArray,
      Specs.sameContext, decrement, count, getStorage, setStorage, Contract.run,
      ContractResult.snd, Verity.bind, Bind.bind]

-- tama: obligation kind=postcondition function=getCount coverage=mirror path=test/verity/Counter.t.sol:CounterTest.testGetterMirrorsGeneratedBytecodeState
theorem getCount_returns_count (s : ContractState) :
  let result := ((getCount).run s).fst
  getCount_spec result s := by
  simp [getCount_spec, getCount, count, getStorage, Contract.run, ContractResult.fst,
    Verity.bind, Bind.bind, Verity.pure, Pure.pure]

-- tama: obligation kind=invariant function=getCount coverage=proof_only reason="Read-only getter preservation is a symbolic frame fact; the Foundry mirror covers the observable return value."
theorem getCount_preserves_state (s : ContractState) :
  let s' := ((getCount).run s).snd
  getCount_preserves_state_spec s s' := by
  simp [getCount_preserves_state_spec, getCount, count, getStorage, Contract.run,
    ContractResult.snd, Verity.bind, Bind.bind, Verity.pure, Pure.pure]

end proof.CounterProof
