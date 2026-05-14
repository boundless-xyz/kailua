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
RISC0_VERSION: 3.0.5
KAILUA_FPVM_KONA_ID: 0x9358024DA6BDABCC0898E9AA012427F2CC772413DC7B85B17F1D4A0B215C4BD2
KAILUA_FPVM_KONA_ELF: 10 MiB
KAILUA_FPVM_HOKULEA_ID: 0x2EDBA008CFE2401A512DB553CC02E6FD0824B3393AD3FC2C43EF83A1FEECD572
KAILUA_FPVM_HOKULEA_ELF: 11.3 MiB
KAILUA_FPVM_HANA_ID: 0xC04C2E7B4AF41B95CC2A708F75FF491FA54C62737A3B1D7D57711E6B48DF06C7
KAILUA_FPVM_HANA_ELF: 10.9 MiB
CONTROL_ROOT: 0xA54DC85AC99F851C92D7C96D7318AF41DBE7C0194EDFCC37EB4D422A998C1F56
CONTROL_ID: 0x04446E66D300EB7FB45C9726BB53C793DDA407A62E9601618BB43C5C14657AC0
RISC_ZERO_VERIFIER: 0x8EAB2D97DFCE405A1692A21B3FF3A172D593D319
GENESIS_TIMESTAMP: 1686789347
BLOCK_TIME: 2
ROLLUP_CONFIG_HASH: 0xDF9FA8CA4D926BC81755591EC6D07F5C72F7EC4F0546A7311916674D95B0513B
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
KAILUA_FPVM_KONA_ID: 0x4DFCC2438337F11F5AA1B8E70E1BF3521FA386949A892AE5D8CF03BF295185AE
KAILUA_FPVM_KONA_ELF: 10.4 MiB
KAILUA_FPVM_HOKULEA_ID: 0x525AB84CE9E49BDB91572BAE167B386CC25861D25D4BCCD8BEA363B35F6DA2A2
KAILUA_FPVM_HOKULEA_ELF: 11.6 MiB
KAILUA_FPVM_HANA_ID: 0xC60156DB0960E0C4A9BA04AB62BCA70A75BAEC4A300B6D1F45FE7CFB256BA5AE
KAILUA_FPVM_HANA_ELF: 11.2 MiB
```


## Telemetry

All Kailua binaries and commands support exporting telemetry data to an
[OTLP Collector](https://opentelemetry.io/docs/collector/).
The collector endpoint can be specified using the `--otlp-collector` parameter, or through specifying the
`OTLP_COLLECTOR` environment variable.
