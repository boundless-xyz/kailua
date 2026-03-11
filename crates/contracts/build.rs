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

use foundry_compilers::artifacts::Remapping;
use std::{fs, io::ErrorKind, str::FromStr};

fn load_remappings(path: &str) -> Vec<Remapping> {
    match fs::read_to_string(path) {
        Ok(contents) => contents
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(|line| {
                Remapping::from_str(line)
                    .unwrap_or_else(|err| panic!("failed to parse remapping `{line}`: {err}"))
            })
            .collect(),
        Err(err) if err.kind() == ErrorKind::NotFound => Vec::new(),
        Err(err) => panic!("failed to read {path}: {err}"),
    }
}

fn load_configured_solc(path: &str) -> Option<foundry_compilers::solc::Solc> {
    let contents = fs::read_to_string(path).ok()?;
    let version = contents.lines().find_map(|line| {
        let line = line.split('#').next()?.trim();
        let (key, value) = line.split_once('=')?;
        (key.trim() == "solc_version").then_some(value.trim().trim_matches(['"', '\'']))
    })?;
    let solc_path = foundry_compilers::solc::Solc::svm_home()?
        .join(version)
        .join(format!("solc-{version}"));
    solc_path
        .is_file()
        .then(|| foundry_compilers::solc::Solc::new(solc_path).ok())
        .flatten()
}

fn main() {
    #[cfg(not(feature = "skip-solc"))]
    {
        let mut settings = foundry_compilers::multi::MultiCompilerSettings::default();
        settings.solc.optimizer.enabled = Some(true);
        settings.solc.optimizer.runs = Some(10_000);
        settings.solc.evm_version = Some(foundry_compilers::artifacts::EvmVersion::Cancun);
        let remappings = load_remappings("foundry/remappings.txt");
        let paths = foundry_compilers::ProjectPathsConfig::builder()
            .remappings(remappings)
            .build_with_root("foundry");
        let mut compiler = foundry_compilers::multi::MultiCompiler::default();
        if let Some(solc) = load_configured_solc("foundry/foundry.toml") {
            compiler.solc = Some(foundry_compilers::solc::SolcCompiler::Specific(solc));
        }

        let project = foundry_compilers::Project::builder()
            .settings(settings)
            .paths(paths)
            .build(compiler)
            .expect("failed to build project");

        let output = project.compile().expect("failed to compile project");

        if output.has_compiler_errors() {
            panic!("{}", format!("{:?}", output.output().errors));
        }

        // Tell Cargo that if a source file changes, to rerun this build script.
        project.rerun_if_sources_changed();
        println!("cargo:rerun-if-changed=src");
        println!("cargo:rerun-if-changed=foundry/remappings.txt");
        println!("cargo:rerun-if-changed=foundry/foundry.toml");
        println!("cargo:rerun-if-changed=foundry/lib");
    }
}
