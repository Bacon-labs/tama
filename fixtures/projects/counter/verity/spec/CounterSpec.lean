import Verity.Specs.Common
import Verity.Macro
import src.Counter

namespace spec.CounterSpec

open Verity
open Verity.Specs
open Verity.EVM.Uint256

#gen_spec increment_spec (0, (fun st => add (st.storage 0) 1), sameAddrMapContext)

#gen_spec decrement_spec (0, (fun st => sub (st.storage 0) 1), sameAddrMapContext)

def getCount_spec (result : Uint256) (s : ContractState) : Prop :=
  result = s.storage 0

def getCount_preserves_state_spec (s s' : ContractState) : Prop :=
  s' = s

end spec.CounterSpec
