# Kailua Validator

The Kailua validator watches your rollup for sequencing proposals that contradict each other and generates a ZK fault
proof to settle the dispute between them.

```admonish note
The Kailua validator agent requires access to an archive `op-geth` rollup node to retrieve data during proof generation.
Node software other than `op-geth` is not as reliable for the necessary `debug` namespace rpc calls.
```

## Usage

Starting the Kailua validator is straightforward:
```shell
kailua-cli validate [OPTIONS] --op-node-url <OP_NODE_URL> --op-geth-url <OP_GETH_URL> --eth-rpc-url <ETH_RPC_URL> --beacon-rpc-url <BEACON_RPC_URL>
```

```admonish tip
All the parameters above can be provided as environment variables.
```

### Remote Endpoints
The mandatory arguments specify the endpoints that the validator should use to resolve disputes:
* `eth-rpc-url`: The parent chain (ethereum) endpoint for reading proposals.
* `beacon-rpc-url`: The DA layer (eth-beacon chain) endpoint for retrieving rollup data.
* `op-geth-url`: The rollup `op-geth` endpoint to read configuration data from.
* `op-node-url`: The rollup `op-node` endpoint to read sequencing proposals from.

### RPC Calls
To fine-tune the interaction with the above endpoints, the following additional parameters can be specified:
* `op-rpc-concurrency`: Number of concurrent RPC requests to allow (default: 64).
* `op-rpc-delay`: Number of L2 blocks to delay observation by (default: 0).
* `rpc-poll-interval`: Time (in seconds) between successive RPC polls (default: 6).
* `op-node-timeout`: Timeout (seconds) for an OP-NODE RPC request (default: 5).
* `op-geth-timeout`: Timeout (seconds) for an OP-GETH RPC request (default: 2).
* `eth-rpc-timeout`: Timeout (seconds) for an ETH RPC request (default: 2).
* `beacon-rpc-timeout`: Timeout (seconds) for a BEACON RPC request (default: 20).

### Cache Directory
The validator saves data to disk as it tracks on-chain proposals.
This allows it to restart quickly.
* `data-dir`: Optional directory to save data to.
    * If unspecified, a tmp directory is created.

### Kailua Deployment
These arguments manually determine the Kailua contract deployment to use and the termination condition.
* `kailua-game-implementation`: The `KailuaGame` contract address.
* `kailua-anchor-address`: Address of the first proposal to synchronize from.
* `final-l2-block`: The last L2 block number to reach and then stop.

### Telemetry
Telemetry data can be exported to an [OTLP Collector](https://opentelemetry.io/docs/collector/).
* `otlp-collector`: The OTLP collector endpoint.

### Rollup Config
These arguments tell Kailua how to read the rollup configuration.
* `bypass-chain-registry`: This flag forces the rollup configuration to be fetched from `op-node` and `op-geth`.

### Prover
The validator proving behavior can be customized through the following arguments:
* `kailua-cli`: The optional path of the external binary to call for custom proof generation.
* `num-concurrent-provers`: Number of provers to run simultaneously (Default: 1).
* `num-concurrent-preflights`: Number of threads per prover to use for fetching preflight data (Default: 4).
* `num-concurrent-proofs`: Number of threads per prover to use for computing sub-proofs (Default: 1).
* `num-concurrent-witgens`: How many threads to use for witness generation per prover.
* `num-concurrent-r0vm`: How many threads to use for zkvm executors per prover.
* `segment-limit`: ZKVM Proving Segment Limit (Default 21).
* `max-witness-size`: Maximum input data byte size per single proof (Default 2.5 GB).
* `max-proof-stitches`: Maximum number of derivation proofs to aggregate per stitching proof.
* `max-derivation-length`: Maximum number of blocks in a continuous derivation proof sequence
* `max-block-derivations`: Maximum number of blocks to derive per single proof.
* `max-block-executions`: Maximum number of blocks to execute per single proof.
* `num-tail-blocks`: Rate of growth of tail proofs in L1 blocks (Default 10).
* `enable-experimental-witness-endpoint`: Enables the use of `debug_executePayload` to collect the execution witness from the execution layer.
* `max-fault-proving-delay`: The maximum amount of seconds to wait before starting to compute a fault proof (Default 900).
* `max-validity-proving-delay`: The maximum amount of seconds to wait before starting to compute a validity proof (Default 0).
* `clear-cache-data`: Whether to clear cache data after successful completion (Default false).

### Fault Proving Permits
The validator can optionally acquire [fault proving permits](design.md#fault-proving-permits) before generating
fault proofs.
* `fault-proving-permit`: Whether acquisition of permits before proving faults is skipped / optional / mandatory (Default `optional`).
* `fault-proving-permit-expiry`: Minimum amount of time (seconds) left on a permit to consider it unexpired (Default 600).
* `min-validity-proving-timestamp`: The minimum UNIX timestamp after which computed validity proofs can be submitted (Default 0).

| Value | Behavior |
|-------|----------|
| `SKIPPED` | Never acquire permits |
| `OPTIONAL` (default) | Acquire if available; proceed without if acquisition fails |
| `MANDATORY` | Halt proving if permit cannot be acquired |

```admonish tip
For most operators, the default `OPTIONAL` policy is recommended.
Use `MANDATORY` if you are the sole validator and want guaranteed exclusive rewards.
Use `SKIPPED` if the permit system is deactivated for your deployment or you do not care about rewards being frontrun.
```

### Alt DA
The following additional parameters are required if an alternative DA method is used:
* `eigenda-proxy-address`: URL of the EigenDA RPC endpoint.
* `celestia-connection`: Connection to celestia network.
* `celestia-auth-token`: Token for the Celestia node connection.
* `celestia-namespace`: Celestia Namespace to fetch data from.

### Wallet
The validator requires a funded wallet to be able to publish fault proofs on chain, and an (optional) alternative address
to direct fault proof submission payouts towards.
This wallet can be specified directly as a private key or as an external AWS/GCP signer.
* `validator-key`: The private key for the validator wallet.
* `payout-recipient-address`: The ethereum address to use as the recipient of fault proof payouts.
* `validator-aws-key-id`: AWS KMS Key ID
* `validator-google-project-id`: GCP KMS Project ID
* `validator-google-location`: GCP KMS Location
* `validator-google-keyring`: GCP KMS Keyring Name
* `validator-google-key-name`: GCP KMS Key name

```admonish tip
`validator-key` can be replaced with the corresponding AWS/GCP parameters as described [here](upgrade.md#kms-support).
```

```admonish warning
You must keep your validator's wallet well funded to guarantee the liveness of your rollup and prevent faulty proposals
from delaying the finality of honest sequencing proposals.
```

```admonish success
Running `kailua-cli validate` should monitor your rollup for any disputes and generate the required proofs!
```

### Transactions
You can control transaction publication through the two following parameters:
* `txn-timeout`: A timeout in seconds for transaction broadcast (default 120)
* `exec-gas-premium`: An added premium percentage to estimated execution gas fees (Default 25)

The premium parameter increases the internally estimated fees by the specified percentage.

### Upgrades
If you re-deploy the KailuaTreasury/KailuaGame contracts to upgrade your fault proof system, you will need to restart
your validator (and proposer).
By default, the validator (and proposer) will use the latest contract deployment available upon start up, and ignore any
proposals not made using them.
If you wish to start a validator for a past deployment, you can explicitly specify the deployed KailuaGame contract
address using the optional `kailua-game-implementation` parameter.
```admonish note
The validator will not generate any proofs for proposals made using a different deployment than the one used at start up.
```

## Validity Proof Generation
Instead of only generating fault proofs, the validator can be instructed to generate a validity proof for every correct
canonical proposal it encounters to fast-forward finality until a specified block height.
This is configured using the below parameters:
* `fast-forward-target`: The L2 block height until which validity proofs should be computed.
* `fast-forward-start`: Block height to start fast-forwarding finality.

```admonish note
To indefinitely power a validity-proof only rollup, this value can be specified to the maximum 64-bit value of
`18446744073709551615`.
```

```admonish success
Running `kailua-cli validate` with the above parameter should generate a validity proof as soon as a correct proposal
is made by an honest proposer!
```

## Delegated Proof Generation
Extra parameters and environment variables can be specified to determine exactly where the RISC Zero proof
generation takes place.
Running using only the parameters above will generate proofs using the local RISC Zero prover available to the validator.
Alternatively, proof generation can be delegated to an external service such as [Bonsai](https://risczero.com/bonsai),
or to the decentralized [Boundless proving network](https://docs.boundless.network/).

```admonish note
All data required to generate the proof can be publicly derived from the public chain data available for your rollup,
making this process safe to delegate.
```

### Bonsai
Enabling proving using [Bonsai](https://risczero.com/bonsai) requires you to set the following two environment variables before running the validator:
* `BONSAI_API_KEY`: Your Bonsai API key.
* `BONSAI_API_URL`: Your Bonsai API url.

```admonish success
Running `kailua-cli validate` with these two environment variables should now delegate all validator proving to [Bonsai](https://risczero.com/bonsai)!
```

### Boundless
When delegating generation of Kailua Fault proofs to the decentralized [Boundless proving network](https://docs.boundless.network/),
for every fault proof, a proof request is submitted to the network, where it goes through the standard
[proof life-cycle](https://docs.boundless.network/developers/proof-lifecycle) on Boundless, before being published by
your validator to settle a dispute.

Pricing, timing, and collateral for proof requests are handled automatically by the
[Boundless SDK](https://docs.boundless.network/developers/tutorials/request). The SDK determines
appropriate prices from market data and gas costs, sets cycle-aware timeouts, and uses chain-specific
collateral defaults. See the [auction parameter guide](https://docs.boundless.network/developers/tutorials/auction)
for details on how the reverse Dutch auction works.

This functionality requires some additional parameters when starting the validator.
These parameters can be passed in as CLI arguments or set as environment variables.

#### Connection
* `boundless-rpc-url`: The RPC endpoint of the L1 chain where the Boundless network is deployed.
* `boundless-wallet-key`: The wallet private key to use to send proof request transactions.
* `boundless-order-stream-url`: (Optional) The URL to use for off-chain order submission.
* `boundless-chain-id`: EIP-155 chain ID of the network hosting Boundless.
* `boundless-verifier-router-address`: Address of the RiscZeroVerifierRouter contract.
* `boundless-set-verifier-address`: The address of the RISC Zero verifier supporting aggregated proofs for order validation.
* `boundless-market-address`: The address of the Boundless market contract.
* `boundless-collateral-token-address`: Address of the stake collateral ERC-20 contract.

#### Execution Estimation
* `boundless-look-back`: (Defaults to `true`) Whether to inspect for duplicates before making a new proof request.
* `boundless-assume-cycle-count`: Skip preflighting execution and assume the given cycle count.
* `boundless-assume-cycles-per-gas`: Skip preflighting and assume a fixed cycle count per gas.
* `boundless-assume-cycles-per-byte`: Skip preflighting and assume a fixed cycle count per input byte.
* `boundless-assume-cycles-per-snark`: Skip preflighting and assume a fixed cycle count per recursive snark.

#### Pricing
By default, the Boundless SDK sets pricing automatically based on market conditions and gas costs.
The following optional parameters allow you to override the SDK defaults:
* `boundless-min-price-per-cycle`: Minimum price per cycle, e.g. `"0.00001 USD"` or `"0.0000001 ETH"`. Requires a unit. If unset, the SDK uses market pricing from the price provider.
* `boundless-max-price-per-cycle`: Maximum price per cycle, same format. If unset, the SDK uses a market-calibrated default plus a gas cost buffer.
* `boundless-max-price-cap`: Hard cap on total order price (e.g. `"0.5 ETH"`, `"100 USD"`). Safety mechanism to prevent excessive spending.

#### Retry Escalation
When a proof request expires without being fulfilled, it is automatically resubmitted with increased pricing and timeouts:
* `boundless-expired-price-inc-perc`: Percentage to increase the price by per retry attempt (Default 10).
* `boundless-expired-time-inc-perc`: Percentage to increase timeouts by per retry attempt (Default 4).

#### Order Submission
* `boundless-order-submission-cooldown`: Time in seconds between attempts to submit new orders (Default 12).
* `boundless-order-check-interval`: (Defaults to `12`) Time in seconds between attempts to check order status.
* `boundless-enable-upload-caching`: Whether to enable image/input upload caching (Default `true`).

#### Funding
* `boundless-order-funding-mode`: Funding mode for order submission. One of `never`, `always`, `available-balance`, or `below-threshold` (Default `never`).
* `boundless-order-funding-threshold`: Threshold (wei) for `below-threshold` funding mode.

#### Legacy Pricing
For backward compatibility, static wei-based pricing can be enabled with `--boundless-legacy-pricing`.
When this flag is set, the SDK's dynamic pricing is bypassed and the following parameters are used instead.
These flags are hidden from `--help` and require `--boundless-legacy-pricing` to be set.

* `boundless-cycle-min-wei`: Starting price (wei) per cycle (Default `200000000`).
* `boundless-cycle-max-wei`: Maximum price (wei) per cycle (Default `600000000`).
* `boundless-mega-cycle-min`: Minimum megacycles per proving order (Default 250).
* `boundless-mega-cycle-collateral`: Collateral (ZKC) per megacycle (Default `2500000000000000`).
* `boundless-order-min-collateral`: Minimum collateral (ZKC) per order (Default `5000000000000000000`).
* `boundless-order-bid-delay-factor`: Multiplier for delay before price ramp-up starts (Default 0.5).
* `boundless-order-min-bid-delay`: Minimum bid delay in seconds (Default 120).
* `boundless-order-ramp-up-factor`: Multiplier for price ramp-up duration (Default 1.0).
* `boundless-order-min-ramp-up`: Minimum ramp-up time in seconds (Default 600).
* `boundless-order-lock-timeout-factor`: Multiplier for lock timeout (Default 3.0).
* `boundless-order-min-lock-timeout`: Minimum lock timeout in seconds (Default 1200).
* `boundless-order-expiry-factor`: Multiplier for order expiry (Default 1.0).
* `boundless-order-min-expiry`: Minimum expiry time in seconds (Default 900).

#### Storage Uploader
The below second set of parameters determine where the proven executable and its input are stored:
* `storage-uploader`: One of `s3`, `gcs`, `pinata`, or `file`.
* `aws-access-key-id`: The `s3` access key.
* `aws-secret-access-key`: The `s3` secret key.
* `s3-bucket`: The `s3` bucket.
* `s3-url`: The `s3` url.
* `s3-use-presigned`: Use presigned URLs for S3.
* `aws-region`: The `s3` region.
* `gcs-bucket`: The GCS bucket name.
* `gcs-url`: The GCS endpoint URL (optional, for emulators).
* `gcs-credentials-json`: GCS service account credentials JSON (optional, uses ADC if not set).
* `pinata-jwt`: The private `pinata` jwt.
* `pinata-api-url`: The `pinata` api URL.
* `ipfs-gateway-url`: The `pinata` gateway URL.
* `file-path`: The file storage provider path.
* `r2-domain`: Custom domain for file retrieval. Currently used to upload with a custom prefix and replace the download URL with this domain.

```admonish success
Running `kailua-cli validate` with the above extra arguments should now delegate all validator proving to the [Boundless proving network](https://docs.boundless.network/)!
```


## Advanced Settings
```admonish warning
The below settings should not be normally used in production.
```

When manually computing individual proofs, the following parameters (or equiv. env. vars) take effect:
* `SKIP_AWAIT_PROOF`: Skips waiting for the proving process to complete on Bonsai/Boundless.
* `SKIP_DERIVATION_PROOF`: Skips provably deriving L2 transactions using L1 data.
* `L1_HEAD_JUMP_BACK`: The number of l1 heads to jump back when initially proving.
