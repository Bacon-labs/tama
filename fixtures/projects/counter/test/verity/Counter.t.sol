// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {CounterDeployer} from "../../src/generated/verity/CounterDeployer.sol";
import {CounterIface} from "../../src/generated/verity/CounterIface.sol";

contract CounterTest {
    function testDeploymentStartsAtZero() public {
        CounterIface counter = CounterDeployer.deploy();
        require(counter.getCount() == 0, "initial count");
    }

    function testIncrementUpdatesCount() public {
        CounterIface counter = CounterDeployer.deploy();
        counter.increment();
        require(counter.getCount() == 1, "increment count");
    }

    function testDecrementUpdatesCount() public {
        CounterIface counter = CounterDeployer.deploy();
        counter.increment();
        counter.increment();
        counter.decrement();
        require(counter.getCount() == 1, "decrement count");
    }

    function testGetterMirrorsGeneratedBytecodeState() public {
        CounterIface counter = CounterDeployer.deploy();
        counter.increment();
        counter.increment();
        require(counter.getCount() == 2, "getter count");
    }
}
