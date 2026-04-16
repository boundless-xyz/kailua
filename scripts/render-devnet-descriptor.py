#!/usr/bin/env python3

import argparse
import json
import subprocess
import tempfile
from pathlib import Path
from typing import Optional


MNEMONIC = "test test test test test test test test test test test junk"
L1_EL_SERVICE = "el-1-geth-teku"
L1_CL_SERVICE = "cl-1-teku-geth"
L2_PARTICIPANT = "node0"
L2_EL_TYPE = "op-geth"
L2_CL_TYPE = "op-node"
L2_NETWORK_NAME = "op-kurtosis"
L1_WALLET_INDEXES = range(3, 10)
ROLE_WALLET_ALIASES = {
    "deployer": "l1ProxyAdmin",
    "owner": "l1ProxyAdmin",
    "guardian": "l1ProxyAdmin",
    "proposer": "l1Faucet",
    "validator": "sequencer",
    "fault-proposer": "challenger",
    "trail-fault-proposer": "l1ProxyAdmin",
    "vanguard": "l1Faucet",
}


def run(*args: str, capture: bool = True) -> str:
    kwargs = {
        "check": True,
        "text": True,
    }
    if capture:
        kwargs["stdout"] = subprocess.PIPE
        kwargs["stderr"] = subprocess.PIPE
    subprocess_result = subprocess.run(args, **kwargs)
    return subprocess_result.stdout if capture else ""


def inspect_service(enclave: str, service: str) -> dict:
    return json.loads(run("kurtosis", "service", "inspect", enclave, service, "-o", "json"))


def inspect_service_optional(enclave: str, service: str) -> Optional[dict]:
    try:
        return inspect_service(enclave, service)
    except subprocess.CalledProcessError:
        return None


def public_endpoint(service: dict, port_name: str, *, endpoint_name: str) -> dict:
    port = service["public_ports"][port_name]
    endpoint = {
        "host": "127.0.0.1",
        "port": port["number"],
    }

    scheme = port.get("maybe_application_protocol")
    if scheme:
        endpoint["scheme"] = scheme
    elif endpoint_name in {"http", "rpc"}:
        endpoint["scheme"] = "http"

    return endpoint


def derive_l1_wallet(index: int) -> dict:
    hd_path = f"m/44'/60'/0'/0/{index}"
    private_key = run("cast", "wallet", "private-key", MNEMONIC, hd_path).strip()
    address = run("cast", "wallet", "address", private_key).strip()
    return {
        "address": address,
        "private_key": private_key,
    }


def normalize_private_key(private_key: str) -> str:
    return private_key if private_key.startswith("0x") else f"0x{private_key}"


def op_role_wallet(op_wallets: dict, alias: str) -> dict:
    return {
        "address": op_wallets[f"{alias}Address"],
        "private_key": normalize_private_key(op_wallets[f"{alias}PrivateKey"]),
    }


def download_op_wallets(enclave: str) -> tuple[str, dict]:
    with tempfile.TemporaryDirectory() as tmp_dir:
        run(
            "kurtosis",
            "files",
            "download",
            enclave,
            "op-deployer-configs",
            tmp_dir,
            capture=False,
        )
        wallets = json.loads((Path(tmp_dir) / "wallets.json").read_text())
    if len(wallets) != 1:
        raise SystemExit(f"Expected a single L2 chain in wallets.json, found {list(wallets)}")
    chain_id = next(iter(wallets))
    return chain_id, wallets[chain_id]


def build_descriptor(enclave: str) -> dict:
    chain_id, op_wallets = download_op_wallets(enclave)

    l1_el = inspect_service(enclave, L1_EL_SERVICE)
    l1_cl = inspect_service(enclave, L1_CL_SERVICE)
    l2_el = inspect_service(enclave, f"op-el-{chain_id}-{L2_PARTICIPANT}-{L2_EL_TYPE}")
    l2_cl = inspect_service(enclave, f"op-cl-{chain_id}-{L2_PARTICIPANT}-{L2_CL_TYPE}")
    da_service = inspect_service_optional(
        enclave,
        f"op-da-da-server-{chain_id}-{L2_NETWORK_NAME}",
    )
    wallets = {
        f"user-key-{index}": derive_l1_wallet(index)
        for index in L1_WALLET_INDEXES
    }
    wallets.update(
        {
            alias: op_role_wallet(op_wallets, op_wallet_alias)
            for alias, op_wallet_alias in ROLE_WALLET_ALIASES.items()
        }
    )

    descriptor = {
        "l1": {
            "nodes": [
                {
                    "services": {
                        "el": {
                            "endpoints": {
                                "rpc": public_endpoint(l1_el, "rpc", endpoint_name="rpc"),
                            }
                        },
                        "cl": {
                            "endpoints": {
                                "http": public_endpoint(l1_cl, "http", endpoint_name="http"),
                            }
                        },
                    }
                }
            ],
            "wallets": wallets,
        },
        "l2": [
            {
                "nodes": [
                    {
                        "services": {
                            "el": {
                                "endpoints": {
                                    "rpc": public_endpoint(l2_el, "rpc", endpoint_name="rpc"),
                                }
                            },
                            "cl": {
                                "endpoints": {
                                    "http": public_endpoint(l2_cl, "rpc", endpoint_name="http"),
                                }
                            },
                        }
                    }
                ]
            }
        ],
    }

    if da_service is not None:
        descriptor["auxiliary_services"] = {
            "eigenda_proxy": {
                "endpoints": {
                    "http": public_endpoint(da_service, "http", endpoint_name="http"),
                }
            }
        }

    return descriptor


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--enclave", required=True)
    parser.add_argument("--output", required=True)
    args = parser.parse_args()

    descriptor = build_descriptor(args.enclave)
    output_path = Path(args.output)
    output_path.parent.mkdir(parents=True, exist_ok=True)
    output_path.write_text(json.dumps(descriptor, indent=2, sort_keys=True) + "\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
