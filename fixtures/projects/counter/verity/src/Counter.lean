import Contracts.Common

namespace src

open Verity hiding pure bind
open Contracts
open Verity.EVM.Uint256
open Verity.Stdlib.Math

verity_contract Counter where
  storage
    count : Uint256 := slot 0

  function increment () : Unit := do
    let current ← getStorage count
    setStorage count (add current 1)

  function decrement () : Unit := do
    let current ← getStorage count
    setStorage count (sub current 1)

  function view getCount () : Uint256 := do
    let current ← getStorage count
    return current

end src
