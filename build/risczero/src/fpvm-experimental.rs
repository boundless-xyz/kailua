
/// FPVM guest program proving OP Stack state transitions via kona (experimental build).
pub const KAILUA_FPVM_KONA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-kona-experimental.bin");
/// Path to the [KAILUA_FPVM_KONA_ELF] binary, relative to this file.
pub const KAILUA_FPVM_KONA_PATH: &str = "bin/kailua-fpvm-kona-experimental.bin";
/// RISC Zero image ID committing to [KAILUA_FPVM_KONA_ELF].
pub const KAILUA_FPVM_KONA_ID: [u32; 8] = [0x558697D9, 0xF146122E, 0x64C77C69, 0x608B566E, 0x12B07ED4, 0x4F59F05F, 0x5C78FD5E, 0xC9F527B6];

/// FPVM guest program variant with EigenDA support via hokulea (experimental build).
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-hokulea-experimental.bin");
/// Path to the [KAILUA_FPVM_HOKULEA_ELF] binary, relative to this file.
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_PATH: &str = "bin/kailua-fpvm-hokulea-experimental.bin";
/// RISC Zero image ID committing to [KAILUA_FPVM_HOKULEA_ELF].
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ID: [u32; 8] = [0x28AB9DFA, 0xEB12DC9, 0xE770FF4D, 0x3C858F5E, 0x27F9A5A5, 0x75BFBC71, 0x90736143, 0x317016C8];

/// FPVM guest program variant with Celestia DA support via hana (experimental build).
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-hana-experimental.bin");
/// Path to the [KAILUA_FPVM_HANA_ELF] binary, relative to this file.
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_PATH: &str = "bin/kailua-fpvm-hana-experimental.bin";
/// RISC Zero image ID committing to [KAILUA_FPVM_HANA_ELF].
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ID: [u32; 8] = [0x3B85911B, 0xDEC0EF9C, 0x11C5E8B6, 0x2F68046, 0x5ADC8F5C, 0x77D7025B, 0xA548BDB5, 0xBD9C13CA];
