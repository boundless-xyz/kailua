// Copyright 2024, 2025 Boundless Foundation, Inc.
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
    #[cfg(feature = "rebuild-fpvm")]
    {
        // Workaround: risc0-build sets CC for the guest build but not AR/RANLIB.
        ensure_risc0_archiver_env();

        // The pinned guest-builder images are amd64-only; force the docker platform.
        #[cfg(not(any(feature = "debug-guest-build", debug_assertions)))]
        ensure_docker_platform_env();

        risc0_build::embed_methods_with_options({
            let guest_options = {
                // Start with default build options
                let opts = risc0_build::GuestOptions::default();

                // Build a reproducible ELF file using docker under the release profile
                #[cfg(not(any(feature = "debug-guest-build", debug_assertions)))]
                let opts = {
                    let mut opts = opts;
                    opts.use_docker = Some(
                        risc0_build::DockerOptionsBuilder::default()
                            .docker_container_tag("r0.1.97.0")
                            .env({
                                vec![(
                                    String::from("CARGO_HOME"),
                                    String::from("/src/build/risczero/.cargo"),
                                )]
                            })
                            .root_dir({
                                let cwd = std::env::current_dir().unwrap();
                                cwd.parent()
                                    .unwrap()
                                    .parent()
                                    .map(|d| d.to_path_buf())
                                    .unwrap()
                            })
                            .build()
                            .unwrap(),
                    );
                    opts
                };

                // Disable dev-mode receipts from being validated inside the guest
                #[cfg(any(
                    feature = "disable-dev-mode",
                    not(any(feature = "debug-guest-build", debug_assertions))
                ))]
                let opts = {
                    let mut opts = opts;
                    opts.features.push(String::from("disable-dev-mode"));
                    opts
                };

                // Forward the experimental flag
                #[cfg(feature = "experimental")]
                let opts = {
                    let mut opts = opts;
                    opts.features.push(String::from("experimental"));
                    opts
                };
                opts
            };

            std::collections::HashMap::from([
                ("kailua-fpvm-kona", guest_options.clone()),
                ("kailua-fpvm-hokulea", guest_options.clone()),
                ("kailua-fpvm-hana", guest_options.clone()),
            ])
        });
    }

    println!("cargo:rerun-if-env-changed=AR");
    println!("cargo:rerun-if-env-changed=RANLIB");
    println!("cargo:rerun-if-env-changed=DOCKER_DEFAULT_PLATFORM");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=kona/src");
    #[cfg(feature = "eigen")]
    println!("cargo:rerun-if-changed=hokulea/src");
    #[cfg(feature = "celestia")]
    println!("cargo:rerun-if-changed=hana/src");
}

/// Set DOCKER_DEFAULT_PLATFORM to linux/amd64 if not already set.
///
/// The pinned `risczero/risc0-guest-builder` images are published for amd64 only — as manifest
/// lists, which buildkit platform-matches strictly — so on arm64 hosts the reproducible guest
/// build must explicitly request amd64 and run under emulation. risc0-build spawns the docker
/// CLI as a child process, which inherits this variable.
#[cfg(all(
    feature = "rebuild-fpvm",
    not(any(feature = "debug-guest-build", debug_assertions))
))]
fn ensure_docker_platform_env() {
    if std::env::var_os("DOCKER_DEFAULT_PLATFORM").is_none() {
        // SAFETY: build scripts are single-threaded at this point.
        unsafe { std::env::set_var("DOCKER_DEFAULT_PLATFORM", "linux/amd64") };
    }
}

/// Set AR and RANLIB env vars to the RISC0 C++ toolchain's riscv32 binutils if not already set.
///
/// The `cc` crate doesn't recognize the `riscv32im-risc0-zkvm-elf` target, so it falls back to
/// the system archiver. The system `ar` and `ranlib` may silently produce corrupt archives
/// from non-Mach-O objects, causing undefined symbol errors at link time.
#[cfg(feature = "rebuild-fpvm")]
fn ensure_risc0_archiver_env() {
    let Some(home) = std::env::var_os("HOME") else {
        return;
    };
    let bin = std::path::PathBuf::from(home).join(".risc0/cpp/bin");
    let ar = bin.join("riscv32-unknown-elf-ar");
    if std::env::var_os("AR_riscv32im_risc0_zkvm_elf").is_none() && ar.exists() {
        // SAFETY: build scripts are single-threaded at this point.
        unsafe { std::env::set_var("AR_riscv32im_risc0_zkvm_elf", &ar) };
    }
    let ranlib = bin.join("riscv32-unknown-elf-ranlib");
    if std::env::var_os("RANLIB_riscv32im_risc0_zkvm_elf").is_none() && ranlib.exists() {
        // SAFETY: build scripts are single-threaded at this point.
        unsafe { std::env::set_var("RANLIB_riscv32im_risc0_zkvm_elf", &ranlib) };
    }
}
