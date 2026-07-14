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
KAILUA_FPVM_KONA_ID: 0xB2E2B1513E80EA1E8F998E51BF8E7754EEC21DBD0463E0B6B115165BA6BAC2BF
KAILUA_FPVM_KONA_ELF: 10.3 MiB
KAILUA_FPVM_HOKULEA_ID: 0x6F3681B55E2C7F038BAEFCC073ECE32879402B05FD58B6055C8FEA07CBD1ABCA
KAILUA_FPVM_HOKULEA_ELF: 11.2 MiB
KAILUA_FPVM_HANA_ID: 0x3F2D74776114C09897D323D40E0BE8F7CD802EEE200CF95E3D73B3280A12EDD5
KAILUA_FPVM_HANA_ELF: 10.8 MiB
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
KAILUA_FPVM_KONA_ID: 0x9821A06CE937E04B5A6A1EB9BC588851ACCE83FB2CD52A4C1BF2A27034AB3C19
KAILUA_FPVM_KONA_ELF: 11 MiB
KAILUA_FPVM_HOKULEA_ID: 0x880EE7C275919CB41AAEBC28D9D5C038FEF3AAAF4618C5C9A3932CEA578C0746
KAILUA_FPVM_HOKULEA_ELF: 11.9 MiB
KAILUA_FPVM_HANA_ID: 0xFE956CE48010C5218333B3760A0BBF6726A26D9A73747A8BAC55C255964CF2CB
KAILUA_FPVM_HANA_ELF: 11.5 MiB
```


## Telemetry

All Kailua binaries and commands support exporting telemetry data to an
[OTLP Collector](https://opentelemetry.io/docs/collector/).
The collector endpoint can be specified using the `--otlp-collector` parameter, or through specifying the
`OTLP_COLLECTOR` environment variable.
