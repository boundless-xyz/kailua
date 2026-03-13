# FPVM Upgrade

This section describes how to upgrade the `KailuaVerifier` contract behind a proxy to a new implementation, for example when the FPVM image ID changes.

## Architecture

When Kailua is deployed using `Deploy.s.sol`, the `KailuaVerifier` contract is placed behind an OP Stack [Proxy](https://github.com/ethereum-optimism/optimism/blob/develop/packages/contracts-bedrock/src/universal/Proxy.sol) (EIP-1967).

The proxy address is what `KailuaTreasury` and `KailuaGame` reference. Upgrading the implementation behind the proxy changes the FPVM image ID and other configuration values without redeploying any downstream contracts.

```admonish info
The `faultProofPermits` mapping lives in proxy storage and is preserved across upgrades.
Configuration values (`FPVM_IMAGE_ID`, `RISC_ZERO_VERIFIER`, `ROLLUP_CONFIG_HASH`, `PERMIT_DURATION`, `PERMIT_DELAY`) are Solidity immutables embedded in each implementation's bytecode and change when the implementation changes.
```

## Prerequisites

1. Access to the proxy admin private key. By default, the proxy admin is the `DisputeGameFactory` owner.
2. The address of the deployed `KailuaVerifier` proxy (`KAILUA_VERIFIER_PROXY`).
3. The new parameter values to change (e.g. `FPVM_IMAGE_ID`). Any parameters not specified will be read from the current implementation.

```admonish warning
The `upgradeTo` call must be sent by the proxy admin. If the admin is a multisig or Safe, you will need to route the transaction through the appropriate signing workflow.
```

## Running the Upgrade

Change your working directory to `crates/contracts/foundry`:
```shell
cd crates/contracts/foundry
```

Set the required environment variables:
```shell
export PRIVATE_KEY=[PROXY_ADMIN_PRIVATE_KEY]
export KAILUA_VERIFIER_PROXY=[DEPLOYED_PROXY_ADDRESS]
```

Set only the parameters you want to change. Any omitted parameters will be read from the current implementation:
```shell
# Example: upgrading only the FPVM image ID
export FPVM_IMAGE_ID=[NEW_FPVM_IMAGE_ID]
```

The full set of optional parameters is:
* `FPVM_IMAGE_ID`: The RISC Zero image ID of the fault proof program.
* `RISC_ZERO_VERIFIER`: The address of the RISC Zero verifier contract.
* `ROLLUP_CONFIG_HASH`: The hash of the rollup configuration.
* `PERMIT_DURATION`: The duration (in seconds) after which a fault proof permit expires.
* `PERMIT_DELAY`: The duration (in seconds) after which a fault proof permit becomes active.

Run the upgrade script:
```shell
forge script UpgradeVerifierScript --rpc-url [YOUR_ETH_RPC_URL] --broadcast
```

## Verification

After the upgrade, verify the new implementation's values through the proxy:

```shell
# Check the new FPVM image ID
cast call [KAILUA_VERIFIER_PROXY] "FPVM_IMAGE_ID() returns (bytes32)" --rpc-url [YOUR_ETH_RPC_URL]

# Check the RISC Zero verifier address
cast call [KAILUA_VERIFIER_PROXY] "RISC_ZERO_VERIFIER() returns (address)" --rpc-url [YOUR_ETH_RPC_URL]

# Check the rollup config hash
cast call [KAILUA_VERIFIER_PROXY] "ROLLUP_CONFIG_HASH() returns (bytes32)" --rpc-url [YOUR_ETH_RPC_URL]

# Check the version
cast call [KAILUA_VERIFIER_PROXY] "version() returns (string)" --rpc-url [YOUR_ETH_RPC_URL]
```
