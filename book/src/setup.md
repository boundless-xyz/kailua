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
RISC0_VERSION: 3.0.6
KAILUA_FPVM_KONA_ID: 0xD47CC9319893A654CB774ED4A66BEAE5056F340C0C3FF6A3271712781A908BBE
KAILUA_FPVM_KONA_ELF: 10.4 MiB
KAILUA_FPVM_HOKULEA_ID: 0xA00F2620A1E64CEEF4E1A15A1E152D04D5E107F7AFDBA11774063D927B871C47
KAILUA_FPVM_HOKULEA_ELF: 11.4 MiB
KAILUA_FPVM_HANA_ID: 0x37C6358F72B5235513C6DC1FC90C18FD3B1D699DD32F84E07787C6D39F12467A
KAILUA_FPVM_HANA_ELF: 11 MiB
CONTROL_ROOT: 0xA54DC85AC99F851C92D7C96D7318AF41DBE7C0194EDFCC37EB4D422A998C1F56
CONTROL_ID: 0x04446E66D300EB7FB45C9726BB53C793DDA407A62E9601618BB43C5C14657AC0
RISC_ZERO_VERIFIER: 0x8EAB2D97DFCE405A1692A21B3FF3A172D593D319
GENESIS_TIMESTAMP: 1686789347
BLOCK_TIME: 2
ROLLUP_CONFIG_HASH: 0x21C9246CB36388245EF7CD08DC27531073F5C522E1BDD83180FBEEFCCB55D22E
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

### Experimental Config
If you are using the experimental build, you should see these values instead:
```
KAILUA_FPVM_KONA_ID: 0x423F11B6F52FD238F4B7784F5DA17572160B506736877431FB328377CBC160A6
KAILUA_FPVM_KONA_ELF: 11.2 MiB
KAILUA_FPVM_HOKULEA_ID: 0xF536311AD097C704FE69CEC8D8462EE8DDC43C1501D12F7FAF99A0BBF0AE4370
KAILUA_FPVM_HOKULEA_ELF: 12.1 MiB
KAILUA_FPVM_HANA_ID: 0x9FAF16B686E91C1B3CADFC65B91502A483EC05DFCAC35742007420AC92FE4EB3
KAILUA_FPVM_HANA_ELF: 11.8 MiB
```


## Telemetry

All Kailua binaries and commands support exporting telemetry data to an
[OTLP Collector](https://opentelemetry.io/docs/collector/).
The collector endpoint can be specified using the `--otlp-collector` parameter, or through specifying the
`OTLP_COLLECTOR` environment variable.
