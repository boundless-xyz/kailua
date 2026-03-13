// Copyright 2025 RISC Zero, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
//
// SPDX-License-Identifier: Apache-2.0
pragma solidity 0.8.24;

/// @title Proxy
/// @notice EIP-1967 transparent proxy. ABI-compatible reimplementation of the OP Stack Proxy.sol
///         (packages/contracts-bedrock/src/universal/Proxy.sol) compiled under Solidity 0.8.24.
///         Passes through calls via delegatecall if the caller is not the admin.
contract Proxy {
    /// @notice Emitted when the implementation is changed (EIP-1967).
    event Upgraded(address indexed implementation);

    /// @notice Emitted when the admin is changed (EIP-1967).
    event AdminChanged(address previousAdmin, address newAdmin);

    /// @notice Reverts if not called by the admin; otherwise proxies the call.
    modifier proxyCallIfNotAdmin() {
        if (msg.sender == _getAdmin() || msg.sender == address(0)) {
            _;
        } else {
            _doProxyCall();
        }
    }

    /// @notice Sets the initial admin. Admin address is stored at the EIP-1967 admin slot.
    constructor(address _admin) {
        _changeAdmin(_admin);
    }

    receive() external payable {
        _doProxyCall();
    }

    fallback() external payable {
        _doProxyCall();
    }

    /// @notice Set the implementation contract address.
    function upgradeTo(address _implementation) public virtual proxyCallIfNotAdmin {
        _setImplementation(_implementation);
    }

    /// @notice Set the implementation and call a function in a single transaction.
    function upgradeToAndCall(address _implementation, bytes calldata _data)
        public
        payable
        virtual
        proxyCallIfNotAdmin
        returns (bytes memory)
    {
        _setImplementation(_implementation);
        (bool success, bytes memory returndata) = _implementation.delegatecall(_data);
        require(success, "Proxy: delegatecall to new implementation contract failed");
        return returndata;
    }

    /// @notice Changes the owner of the proxy contract.
    function changeAdmin(address _admin) public virtual proxyCallIfNotAdmin {
        _changeAdmin(_admin);
    }

    /// @notice Gets the owner of the proxy contract.
    function admin() public virtual proxyCallIfNotAdmin returns (address) {
        return _getAdmin();
    }

    /// @notice Queries the implementation address.
    function implementation() public virtual proxyCallIfNotAdmin returns (address) {
        return _getImplementation();
    }

    function _setImplementation(address _implementation) internal {
        /// @dev `bytes32(uint256(keccak256('eip1967.proxy.implementation')) - 1)`
        bytes32 proxyImplementation = 0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc;
        assembly {
            sstore(proxyImplementation, _implementation)
        }
        emit Upgraded(_implementation);
    }

    function _changeAdmin(address _admin) internal {
        address previous = _getAdmin();
        /// @dev `bytes32(uint256(keccak256('eip1967.proxy.admin')) - 1)`
        bytes32 proxyOwner = 0xb53127684a568b3173ae13b9f8a6016e243e63b6e8ee1178d6a717850b5d6103;
        assembly {
            sstore(proxyOwner, _admin)
        }
        emit AdminChanged(previous, _admin);
    }

    function _doProxyCall() internal {
        address impl = _getImplementation();
        require(impl != address(0), "Proxy: implementation not initialized");

        assembly {
            calldatacopy(0x0, 0x0, calldatasize())
            let success := delegatecall(gas(), impl, 0x0, calldatasize(), 0x0, 0x0)
            returndatacopy(0x0, 0x0, returndatasize())
            if iszero(success) { revert(0x0, returndatasize()) }
            return(0x0, returndatasize())
        }
    }

    function _getImplementation() internal view returns (address) {
        address impl;
        bytes32 proxyImplementation = 0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc;
        assembly {
            impl := sload(proxyImplementation)
        }
        return impl;
    }

    function _getAdmin() internal view returns (address) {
        address owner;
        bytes32 proxyOwner = 0xb53127684a568b3173ae13b9f8a6016e243e63b6e8ee1178d6a717850b5d6103;
        assembly {
            owner := sload(proxyOwner)
        }
        return owner;
    }
}
