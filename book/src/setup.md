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
KAILUA_FPVM_KONA_ID: 0xA19E48BE71B0E8933D21B15EA2EDF2DE8B36B8E21CC6E6A867C51C0C6480FF06
KAILUA_FPVM_KONA_ELF: 10 MiB
KAILUA_FPVM_HOKULEA_ID: 0xB11975A3AFE5EC595FC460A3CD8FF51CC727F03AF851047AA663B8B895875111
KAILUA_FPVM_HOKULEA_ELF: 11.3 MiB
KAILUA_FPVM_HANA_ID: 0x4FB5ACEA7BC22AC994062823352E6530B9666E2914EB5DE3E145FA40EB892C36
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
KAILUA_FPVM_KONA_ID: 0x51CDAFAAC82EEA42D65179128412DFCF69644AC54D3B21B799C5A272393DCB08
KAILUA_FPVM_KONA_ELF: 10.7 MiB
KAILUA_FPVM_HOKULEA_ID: 0x40D2826A8A67A604F7ED0E058661BBC6522FF11FFC318B4E9E4432B58711F1B0
KAILUA_FPVM_HOKULEA_ELF: 12 MiB
KAILUA_FPVM_HANA_ID: 0xB15EAADAB1B39EB0EF2852464A03180CEBC128FC42200F647CA012A588FC9615
KAILUA_FPVM_HANA_ELF: 11.6 MiB
```


## Telemetry

All Kailua binaries and commands support exporting telemetry data to an
[OTLP Collector](https://opentelemetry.io/docs/collector/).
The collector endpoint can be specified using the `--otlp-collector` parameter, or through specifying the
`OTLP_COLLECTOR` environment variable.
