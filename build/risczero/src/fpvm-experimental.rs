
/// FPVM guest program proving OP Stack state transitions via kona (experimental build).
pub const KAILUA_FPVM_KONA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-kona-experimental.bin");
/// Path to the [KAILUA_FPVM_KONA_ELF] binary, relative to this file.
pub const KAILUA_FPVM_KONA_PATH: &str = "bin/kailua-fpvm-kona-experimental.bin";
/// RISC Zero image ID committing to [KAILUA_FPVM_KONA_ELF].
pub const KAILUA_FPVM_KONA_ID: [u32; 8] = [0xB6113F42, 0x38D22FF5, 0x4F78B7F4, 0x7275A15D, 0x67500B16, 0x31748736, 0x778332FB, 0xA660C1CB];

/// FPVM guest program variant with EigenDA support via hokulea (experimental build).
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-hokulea-experimental.bin");
/// Path to the [KAILUA_FPVM_HOKULEA_ELF] binary, relative to this file.
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_PATH: &str = "bin/kailua-fpvm-hokulea-experimental.bin";
/// RISC Zero image ID committing to [KAILUA_FPVM_HOKULEA_ELF].
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ID: [u32; 8] = [0x1A3136F5, 0x4C797D0, 0xC8CE69FE, 0xE82E46D8, 0x153CC4DD, 0x7F2FD101, 0xBBA099AF, 0x7043AEF0];

/// FPVM guest program variant with Celestia DA support via hana (experimental build).
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-hana-experimental.bin");
/// Path to the [KAILUA_FPVM_HANA_ELF] binary, relative to this file.
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_PATH: &str = "bin/kailua-fpvm-hana-experimental.bin";
/// RISC Zero image ID committing to [KAILUA_FPVM_HANA_ELF].
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ID: [u32; 8] = [0xB616AF9F, 0x1B1CE986, 0x65FCAD3C, 0xA40215B9, 0xDF05EC83, 0x4257C3CA, 0xAC207400, 0xB34EFE92];
