// Copyright 2025 Boundless Foundation, Inc.
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

/// Celestia connection CLI arguments.
pub mod args;
/// Hint handling with Blobstream height validation and Steel height proofs.
pub mod handler;
/// Provider construction for the Celestia chain host.
pub mod providers;
/// Witness generation for the Celestia-enabled guest.
pub mod witgen;
