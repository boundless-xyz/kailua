// Copyright 2024 RISC Zero, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

fn main() {
    #[cfg(not(feature = "skip-solc"))]
    {
        let mut settings = foundry_compilers::multi::MultiCompilerSettings::default();
        settings.solc.optimizer.enabled = Some(true);
        settings.solc.optimizer.runs = Some(10_000);
        settings.solc.evm_version = Some(foundry_compilers::artifacts::EvmVersion::Cancun);
        let project = foundry_compilers::Project::builder()
            .settings(settings)
            .paths(foundry_compilers::ProjectPathsConfig::builder().build_with_root("foundry"))
            .build(Default::default())
            .expect("failed to build project");

        let output = project.compile().expect("failed to compile project");

        if output.has_compiler_errors() {
            panic!("{}", format!("{:?}", output.output().errors));
        }

        // Tell Cargo that if a source file changes, to rerun this build script.
        project.rerun_if_sources_changed();
        println!("cargo:rerun-if-changed=src");
    }
}
