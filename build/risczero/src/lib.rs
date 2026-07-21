// Copyright 2024 - 2026 Boundless Foundation, Inc.
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

//! Embedded FPVM guest programs and their RISC Zero image IDs.
//!
//! The crate root exposes `KAILUA_FPVM_*_ELF`/`_ID`/`_PATH` constants for each supported
//! proving target: kona (always), hokulea for EigenDA (`eigen` feature), and hana for
//! Celestia DA (`celestia` feature). Exactly one source provides them: the checked-in
//! binaries under `src/bin` via `fpvm.rs` by default (or `fpvm-experimental.rs` under the
//! `experimental` feature), or a fresh guest compilation through `risc0-build` under the
//! `rebuild-fpvm` feature. The checked-in image IDs are pinned by deployed contracts;
//! regenerate them with the `build-fpvm*` and `export-fpvm` justfile recipes.

// The rebuild-fpvm path includes risc0-build generated code, which carries no doc comments.
#![cfg_attr(not(any(test, feature = "rebuild-fpvm")), warn(missing_docs))]

#[cfg(feature = "rebuild-fpvm")]
include!(concat!(env!("OUT_DIR"), "/methods.rs"));

#[cfg(not(any(feature = "rebuild-fpvm", feature = "experimental")))]
include!("fpvm.rs");

#[cfg(all(not(feature = "rebuild-fpvm"), feature = "experimental"))]
include!("fpvm-experimental.rs");
