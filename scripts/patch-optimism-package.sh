#!/usr/bin/env bash

set -euo pipefail

package_dir="${1:?usage: patch-optimism-package.sh <package-dir>}"

python3 - "$package_dir" <<'PY'
from pathlib import Path
import sys


def patch_file(path: Path, before: str, after: str) -> None:
    text = path.read_text()
    if after in text:
        return
    if before not in text:
        raise SystemExit(f"Failed to patch {path}: expected pattern not found")
    path.write_text(text.replace(before, after, 1))


def insert_after(path: Path, marker: str, addition: str) -> None:
    text = path.read_text()
    if addition in text:
        return
    if marker not in text:
        raise SystemExit(f"Failed to patch {path}: expected marker not found")
    path.write_text(text.replace(marker, marker + addition, 1))


root = Path(sys.argv[1])

observability = root / "src/observability/observability.star"
patch_file(
    observability,
    """def register_op_service_metrics_job(helper, service, network_name=None):\n    register_service_metrics_job(\n""",
    """def register_op_service_metrics_job(helper, service, network_name=None):\n    if not helper.enabled:\n        return\n\n    register_service_metrics_job(\n""",
)

contract_deployer = root / "src/contracts/contract_deployer.star"
patch_file(
    contract_deployer,
    """            "sequencerFeeVaultRecipient": read_chain_cmd(\n                "sequencerFeeVaultRecipient", chain_id\n            ),\n            "roles": {\n""",
    """            "sequencerFeeVaultRecipient": read_chain_cmd(\n                "sequencerFeeVaultRecipient", chain_id\n            ),\n            "operatorFeeVaultRecipient": read_chain_cmd(\n                "baseFeeVaultRecipient", chain_id\n            ),\n            "roles": {\n""",
)

op_node_launcher = root / "src/cl/op-node/launcher.star"
insert_after(
    op_node_launcher,
    """        + "{0}/rollup-{1}.json".format(\n            _ethereum_package_constants.GENESIS_DATA_MOUNTPOINT_ON_CLIENTS,\n            network_params.network_id,\n        ),\n""",
    """        "--rollup.l1-chain-config="\n        + "{0}/l1-chain-config.json".format(\n            _ethereum_package_constants.GENESIS_DATA_MOUNTPOINT_ON_CLIENTS,\n        ),\n""",
)
patch_file(
    op_node_launcher,
    """    supervisor_params = _filter.first(supervisors_params)\n\n    # configure files\n\n    files = {\n        _ethereum_package_constants.GENESIS_DATA_MOUNTPOINT_ON_CLIENTS: Directory(\n            artifact_names=[\n                deployment_output,\n                supervisor_params.superchain.dependency_set.name,\n            ]\n        )\n        if supervisor_params\n        else deployment_output,\n        _ethereum_package_constants.JWT_MOUNTPOINT_ON_CLIENTS: jwt_file,\n    }\n""",
    """    supervisor_params = _filter.first(supervisors_params)\n\n    # configure files\n\n    l1_chain_config = plan.run_sh(\n        name=params.service_name + \"-l1-chain-config\",\n        description=\"Build L1 chain config for op-node\",\n        image=_util.DEPLOYMENT_UTILS_IMAGE,\n        files={\"/l1-genesis\": plan.get_files_artifact(name=\"el_cl_genesis_data\")},\n        store=[StoreSpec(src=\"/l1-chain-config.json\", name=params.service_name + \"-l1-chain-config\")],\n        run=\"jq '.config | del(.terminalTotalDifficultyPassed)' /l1-genesis/genesis.json > /l1-chain-config.json\",\n    )\n\n    network_config_artifacts = [deployment_output, l1_chain_config.files_artifacts[0]]\n    if supervisor_params:\n        network_config_artifacts.append(supervisor_params.superchain.dependency_set.name)\n\n    files = {\n        _ethereum_package_constants.GENESIS_DATA_MOUNTPOINT_ON_CLIENTS: Directory(\n            artifact_names=network_config_artifacts,\n        ),\n        _ethereum_package_constants.JWT_MOUNTPOINT_ON_CLIENTS: jwt_file,\n    }\n""",
)
PY
