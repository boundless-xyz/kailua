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
KAILUA_FPVM_KONA_ID: 0x3A06234E13575C46350E33F91921F045E07000AD0A59E52503190BF0F588B565
KAILUA_FPVM_KONA_ELF: 37.4 MiB
KAILUA_FPVM_HOKULEA_ID: 0x23AC56A6AE6420C7A487C4DA31488A4BDB40EE4DEB0C98DB02E16B92407EFF47
KAILUA_FPVM_HOKULEA_ELF: 40.6 MiB
KAILUA_DA_HOKULEA_ID: 0xE6AE1F0EE0FEE9E253DB02250FAD8C0C8DC65141A0042A879FBACBDAE50EA2CB
KAILUA_DA_HOKULEA_ELF: 2.9 MiB
KAILUA_FPVM_HANA_ID: 0xD72C647405745D83A3AE4E9837BBA2122E8DDEC221E555F5028E7FE386CADFBA
KAILUA_FPVM_HANA_ELF: 43.7 MiB
CONTROL_ROOT: 0xA54DC85AC99F851C92D7C96D7318AF41DBE7C0194EDFCC37EB4D422A998C1F56
CONTROL_ID: 0x04446E66D300EB7FB45C9726BB53C793DDA407A62E9601618BB43C5C14657AC0
RISC_ZERO_VERIFIER: 0x8EAB2D97DFCE405A1692A21B3FF3A172D593D319
GENESIS_TIMESTAMP: 1686789347
BLOCK_TIME: 2
ROLLUP_CONFIG_HASH: 0x189C80BF708F54392730B852EEC1C5428DA75853FED5304D6F58E1442E1A4772
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
