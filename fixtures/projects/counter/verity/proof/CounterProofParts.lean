import spec.CounterSpec
import Verity.Proofs.Stdlib.Automation

namespace proof.CounterProof

open Verity
open Verity.EVM.Uint256
open spec.CounterSpec
open src.Counter

theorem getCount_returns_count (s : ContractState) :
  let result := ((getCount).run s).fst
  getCount_spec result s := by
  simp [getCount_spec, getCount, count, getStorage, Contract.run, ContractResult.fst,
    Verity.bind, Bind.bind, Verity.pure, Pure.pure]

theorem getCount_preserves_state (s : ContractState) :
  let s' := ((getCount).run s).snd
  getCount_preserves_state_spec s s' := by
  simp [getCount_preserves_state_spec, getCount, count, getStorage, Contract.run,
    ContractResult.snd, Verity.bind, Bind.bind, Verity.pure, Pure.pure]

end proof.CounterProof
