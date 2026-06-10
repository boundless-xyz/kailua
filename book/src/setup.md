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
KAILUA_FPVM_KONA_ID: 0x6C7BE509D20E6CFD05FF4C8F91EEC0C7BA9937D20951C2E8353FFB36A6A355B7
KAILUA_FPVM_KONA_ELF: 10.3 MiB
KAILUA_FPVM_HOKULEA_ID: 0x5DF8B686210B1F867B9978DB105E15F7607CAE67723EE7303E838F327331AE60
KAILUA_FPVM_HOKULEA_ELF: 11.2 MiB
KAILUA_FPVM_HANA_ID: 0xB109A6457128788DBECA888168C5BF70FCFA9F96C3AE0350004B9723104091A4
KAILUA_FPVM_HANA_ELF: 10.9 MiB
CONTROL_ROOT: 0xA54DC85AC99F851C92D7C96D7318AF41DBE7C0194EDFCC37EB4D422A998C1F56
CONTROL_ID: 0x04446E66D300EB7FB45C9726BB53C793DDA407A62E9601618BB43C5C14657AC0
RISC_ZERO_VERIFIER: 0x8EAB2D97DFCE405A1692A21B3FF3A172D593D319
GENESIS_TIMESTAMP: 1686789347
BLOCK_TIME: 2
ROLLUP_CONFIG_HASH: 0xCF34FB2ED267B276B52DA967EEB1F02B0BA36FC2AE6C0DC6474C73B9F7D9FEA7
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
KAILUA_FPVM_KONA_ID: 0x1167112E7BD2219889B4F5BDF0E656A5B1B069CE3DF92D8089477BB79EE13106
KAILUA_FPVM_KONA_ELF: 11 MiB
KAILUA_FPVM_HOKULEA_ID: 0x968BF4FC27B522537BA9EF921393D48DFEFFF4EFEBE99C85D0A27FF5C4901213
KAILUA_FPVM_HOKULEA_ELF: 12 MiB
KAILUA_FPVM_HANA_ID: 0x3C172F0522A6BCBE0ABC563FD4BAE3E0124A02ABEC72A8A2F6B287CDB1C21F60
KAILUA_FPVM_HANA_ELF: 11.6 MiB
```


## Telemetry

All Kailua binaries and commands support exporting telemetry data to an
[OTLP Collector](https://opentelemetry.io/docs/collector/).
The collector endpoint can be specified using the `--otlp-collector` parameter, or through specifying the
`OTLP_COLLECTOR` environment variable.
