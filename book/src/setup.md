# Setup

Make sure to first install the [prerequisites](quickstart.md#prerequisites) from the quickstart
section before proceeding.

## Installation

Before you can start migrating your rollup, you'll need to build and install Kailua's binaries by calling the following
commands from the root project directory:

```admonish tip
If you have modified the FPVM binary, you will need to build/install using `-F rebuild-fpvm`.
```

```admonish info
At the cost of longer compilation time, you can embed the RISC Zero zkvm prover logic into `kailua-cli` instead of 
having it utilize your locally installed RISC Zero `r0vm` for proving.
To do this, add `-F prove` to the install command below.
```

```admonish tip
For GPU-accelerated local proving, use one of the following feature flags:
* Apple: `-F metal`
* Nvidia: `-F cuda`
```

### CLI Binary
```shell
cargo install kailua-cli --path bin/cli --locked
```

## Configuration

Once your installation is successful, you should be able to run the following command to fetch the Kailua configuration
parameters for your rollup instance:

```shell
kailua-cli config --op-node-url [YOUR_OP_NODE_URL] --op-geth-url [YOUR_OP_GETH_URL] --eth-rpc-url [YOUR_ETH_RPC_URL]
```

Running the above command against the respective Base mainnet endpoints should produce the following output:
```
RISC0_VERSION: 3.0.3
KAILUA_FPVM_KONA_ID: 0x5F3809E0B8E9096AA4647C4B8B6B3652AE34FFB30692E046845D377F8AC6D06E
KAILUA_FPVM_KONA_ELF: 38.6 MiB
KAILUA_FPVM_HOKULEA_ID: 0x6B4F992476FC6C6BB22ABF2812C5B6A5ACD63F7A5273FC04B99725C0EDBD4D0F
KAILUA_FPVM_HOKULEA_ELF: 43.9 MiB
KAILUA_FPVM_HANA_ID: 0x78AEAD3752E413BAA56E8152747E14E5DFA6549D73751119265B2C76CD0DC120
KAILUA_FPVM_HANA_ELF: 42.2 MiB
CONTROL_ROOT: 0xA54DC85AC99F851C92D7C96D7318AF41DBE7C0194EDFCC37EB4D422A998C1F56
CONTROL_ID: 0x04446E66D300EB7FB45C9726BB53C793DDA407A62E9601618BB43C5C14657AC0
RISC_ZERO_VERIFIER: 0x8EAB2D97DFCE405A1692A21B3FF3A172D593D319
GENESIS_TIMESTAMP: 1686789347
BLOCK_TIME: 2
ROLLUP_CONFIG_HASH: 0x6948B0C582AE9EAE9604F8C266F6E34DAEF6FA352340A8CCD308D1EA7619DA1F
DISPUTE_GAME_FACTORY: 0x43EDB88C4B80FDD2ADFF2412A7BEBF9DF42CB40E
OPTIMISM_PORTAL: 0x49048044D57E1C92A77F79988D21FA8FAF74E97E
KAILUA_GAME_TYPE: 1337
```

```admonish warning
Make sure that your `FPVM_IMAGE_ID` matches the value above.
This value determines the exact program used to prove faults.
```

```admonish note
If your `RISC_ZERO_VERIFIER` value is blank, this means that your rollup might be deployed on a base layer that does
not have a deployed RISC Zero zkVM verifier contract.
This means you might have to deploy your own verifier.
Always revise the RISC Zero [documentation](https://dev.risczero.com/api/blockchain-integration/contracts/verifier)
to double-check verifier availability.
```

Once you have these values you'll need to save them for later use during migration.

## Telemetry

All Kailua binaries and commands support exporting telemetry data to an
[OTLP Collector](https://opentelemetry.io/docs/collector/).
The collector endpoint can be specified using the `--otlp-collector` parameter, or through specifying the
`OTLP_COLLECTOR` environment variable.
