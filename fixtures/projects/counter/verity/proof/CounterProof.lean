import spec.CounterSpec
import proof.CounterProofParts
import Verity.Proofs.Stdlib.Automation

namespace proof.CounterProof

open Verity
open Verity.EVM.Uint256
open spec.CounterSpec
open src.Counter

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

end proof.CounterProof
