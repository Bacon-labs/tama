// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {CounterDeployer} from "../../src/generated/verity/CounterDeployer.sol";
import {CounterIface} from "../../src/generated/verity/CounterIface.sol";

abstract contract MinimalStdInvariant {
    struct FuzzSelector {
        address addr;
        bytes4[] selectors;
    }

    FuzzSelector[] private targetedSelectors;

    function targetSelector(FuzzSelector memory selector) internal {
        targetedSelectors.push(selector);
    }

    function targetSelectors() public view returns (FuzzSelector[] memory selectors) {
        selectors = targetedSelectors;
    }
}

contract CounterTest is MinimalStdInvariant {
    CounterIface internal invariantCounter;
    uint256 internal invariantModel;

    function setUp() public {
        invariantCounter = CounterDeployer.deploy();
        bytes4[] memory selectors = new bytes4[](2);
        selectors[0] = this.handlerIncrement.selector;
        selectors[1] = this.handlerDecrement.selector;
        targetSelector(FuzzSelector({addr: address(this), selectors: selectors}));
    }

    function testFuzzDeploymentStartsAtZero(uint8 preexistingSteps) public {
        CounterIface existing = CounterDeployer.deploy();
        for (uint256 i = 0; i < preexistingSteps; i++) {
            existing.increment();
        }
        CounterIface counter = CounterDeployer.deploy();
        require(counter.getCount() == 0, "initial count");
    }

    // tama: mirrors=increment_spec
    function testFuzzIncrementUpdatesCount(uint8 initialSteps, uint8 extraSteps) public {
        CounterIface counter = CounterDeployer.deploy();
        for (uint256 i = 0; i < initialSteps; i++) {
            counter.increment();
        }
        for (uint256 i = 0; i < extraSteps; i++) {
            counter.increment();
        }
        require(
            counter.getCount() == uint256(initialSteps) + uint256(extraSteps),
            "increment count"
        );
    }

    // tama: mirrors=decrement_spec
    function testFuzzDecrementUpdatesCount(uint8 initialSteps, uint8 decrementSteps) public {
        CounterIface counter = CounterDeployer.deploy();
        for (uint256 i = 0; i < initialSteps; i++) {
            counter.increment();
        }
        for (uint256 i = 0; i < decrementSteps; i++) {
            counter.decrement();
        }
        uint256 expected;
        unchecked {
            expected = uint256(initialSteps) - uint256(decrementSteps);
        }
        require(counter.getCount() == expected, "decrement count");
    }

    // tama: mirrors=getCount_spec
    function testFuzzGetterMirrorsGeneratedBytecodeState(uint8 incrementSteps, uint8 decrementSteps) public {
        CounterIface counter = CounterDeployer.deploy();
        uint256 expected;
        for (uint256 i = 0; i < incrementSteps; i++) {
            counter.increment();
            unchecked {
                expected++;
            }
        }
        for (uint256 i = 0; i < decrementSteps; i++) {
            counter.decrement();
            unchecked {
                expected--;
            }
        }
        require(counter.getCount() == expected, "getter count");
    }

    // tama: mirrors=getCount_preserves_state_spec
    function testFuzzGetterPreservesCount(uint8 incrementSteps, uint8 decrementSteps) public {
        CounterIface counter = CounterDeployer.deploy();
        for (uint256 i = 0; i < incrementSteps; i++) {
            counter.increment();
        }
        for (uint256 i = 0; i < decrementSteps; i++) {
            counter.decrement();
        }
        uint256 beforeCount = counter.getCount();
        counter.getCount();
        require(counter.getCount() == beforeCount, "getter preserves count");
    }

    function handlerIncrement() public {
        invariantCounter.increment();
        unchecked {
            invariantModel++;
        }
    }

    function handlerDecrement() public {
        invariantCounter.decrement();
        unchecked {
            invariantModel--;
        }
    }

    function invariant_countTracksModel() public view {
        require(invariantCounter.getCount() == invariantModel, "model count");
    }
}
