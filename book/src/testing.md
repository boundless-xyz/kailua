# Testing Tools

Beyond the agents described in the [migration guide](operate.md), the Kailua CLI ships commands for measuring proving
performance, testing fault proofs, and recovering delegated proofs.
None of these are required to operate Kailua, but they are useful when evaluating it, dimensioning provers, or
exercising a test deployment.

```admonish tip
All the parameters below can also be provided as environment variables.
```

## Validity Proving Demo

`kailua-cli demo` continuously computes validity proofs for a running OP Stack rollup **without requiring any Kailua
contract deployment**.
This is the quickest way to measure real proving costs and latency for your rollup before migrating.

```shell
kailua-cli demo \
      --eth-rpc-url [YOUR_ETH_RPC_URL] \
      --beacon-rpc-url [YOUR_BEACON_RPC_URL] \
      --op-geth-url [YOUR_OP_GETH_URL] \
      --op-node-url [YOUR_OP_NODE_URL] \
      --num-blocks-per-proof [BLOCKS_PER_PROOF]
```

* `num-blocks-per-proof`: The number of L2 blocks each proof must cover.
* `starting-block-height`: (Optional) The L2 block to start proving from. Defaults to `nth-proof-to-process` times `num-blocks-per-proof` blocks before the latest safe block.
* `nth-proof-to-process`: Compute only every n-th proof (Default 1), allowing multiple demo instances to split one workload.
* `num-concurrent-provers`: Number of provers to run simultaneously (Default 1).
* `kailua-cli`: The optional path of the external binary to call for custom proof generation.
* `data-dir`: Optional directory to save proving data to.

The [prover](validator.md#prover), [delegated proving](validator.md#delegated-proof-generation), and
[telemetry](validator.md#telemetry) parameters of the validator also apply.
The `just demo` recipe wraps this command.

## Benchmarking

`kailua-cli benchmark` measures proving cost and performance over past L2 blocks.
It scans the `bench-range` blocks starting at `bench-start`, selects the `bench-count` candidate blocks with the
highest transaction counts, and proves the `bench-length`-block sequence starting at each candidate.

* `bench-start`: The first L2 block number to scan from.
* `bench-range`: The number of L2 blocks to scan as benchmark candidates.
* `bench-count`: The number of top candidate blocks to benchmark.
* `bench-length`: The number of consecutive L2 blocks each benchmark proof must cover.
* `seq-window`: The sequencing window size to use for proving.
* `random-select`: Select candidates pseudorandomly instead of by highest transaction count (Default false).
* `export-bench-csv`: Whether to export a CSV file with the benchmark results (Default false).
* `num-concurrent-provers`: Number of provers to run simultaneously (Default 1).

The same endpoint, prover, and delegated proving parameters as `demo` apply.
The `just bench` recipe wraps this command.

## Fault Injection

`kailua-cli test-fault` publishes a deliberately faulty sequencing proposal, so that a running validator can be
observed disputing and defeating it.
It accepts all [proposer](proposer.md) parameters, plus:

* `fault-offset`: The offset of the faulty intermediate output commitment within the published proposal data.
* `fault-parent`: The index of the proposal to build the faulty proposal on top of.

The faulty proposal commits to a garbage output root at the chosen offset.
An offset within the proposal's output count yields a proposal refutable by an
[output fault proof](validating.md#output-fault-proofs), while an offset pointing into the zero-padded trail
yields one refutable by a [trail fault proof](validating.md#trail-fault-proofs).

```admonish danger
The faulty proposal is staked with the proposer wallet's real collateral, which is slashed once the fault is proven.
Only use this command against test deployments.
```

On the [local devnet](#local-devnet), `just devnet-fault [OFFSET] [PARENT]` wraps this command.

## Receipt Recovery

When proof generation is delegated to a remote service, the finished receipt can be lost if the requesting process
dies before retrieving it (or was run with `skip-await-proof`).
These two commands download the finished receipt, verify it, and save it to the same proof file a local proving run
would have produced:

* `kailua-cli bonsai --session-id [SESSION_ID]` retrieves a proving session's receipt from
  [Bonsai](https://risczero.com/bonsai), using the `BONSAI_API_KEY`/`BONSAI_API_URL` environment variables.
* `kailua-cli boundless --boundless-rpc-url [RPC_URL] --request-id [REQUEST_ID]` retrieves the fulfilled proof for a
  request from the [Boundless](https://docs.boundless.network/) market.

## Manual Proving

`kailua-cli prove` is the command the validator internally invokes to compute individual proofs, and it can also be
called manually (see the `prove` and `devnet-prove` justfile recipes for example invocations).
Beyond the shared [prover](validator.md#prover) and [delegated proving](validator.md#delegated-proof-generation)
parameters, three parameters bind the computed proof to a proposal's published data instead of a bare block range:

* `precondition-params`: Three comma-separated values: the proposal's starting L2 block number, its published
  intermediate commitment count, and the number of blocks covered per commitment.
* `precondition-block-hashes`: The comma-separated hashes of the L1 blocks at which each of the proposal's data blobs
  was published.
* `precondition-blob-hashes`: The comma-separated versioned hashes of the proposal's data blobs.

When set, the resulting proof can only settle disputes involving proposals that published that exact data.
The validator supplies these parameters automatically when generating proofs for on-chain disputes.

## Local Devnet

For end-to-end testing, the repository ships a [Kurtosis](https://docs.kurtosis.com/)-based local devnet with recipes
covering the full lifecycle:

```shell
just devnet-up            # fetch, patch, and launch the devnet (Docker + Kurtosis required)
just devnet-upgrade       # deploy the Kailua contracts (uses RISC0_DEV_MODE)
just devnet-propose       # run a proposer against the devnet
just devnet-validate      # run a validator against the devnet
just devnet-fault 1 0     # publish a faulty proposal for the validator to defeat
just devnet-down          # tear the devnet down
just devnet-clean         # tear down and delete all devnet artifacts
```

The recipes resolve endpoints and prefunded wallet keys automatically from the devnet descriptor produced by
`devnet-up`.
The underlying scripts — including the EigenDA devnet variant — are described in `scripts/README.md`.
