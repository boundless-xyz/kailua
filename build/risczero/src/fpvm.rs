
/// FPVM guest program proving OP Stack state transitions via kona.
pub const KAILUA_FPVM_KONA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-kona.bin");
/// Path to the [KAILUA_FPVM_KONA_ELF] binary, relative to this file.
pub const KAILUA_FPVM_KONA_PATH: &str = "bin/kailua-fpvm-kona.bin";
/// RISC Zero image ID committing to [KAILUA_FPVM_KONA_ELF].
pub const KAILUA_FPVM_KONA_ID: [u32; 8] = [0x51B1E2B2, 0x1EEA803E, 0x518E998F, 0x54778EBF, 0xBD1DC2EE, 0xB6E06304, 0x5B1615B1, 0xBFC2BAA6];

/// FPVM guest program variant with EigenDA support via hokulea.
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-hokulea.bin");
/// Path to the [KAILUA_FPVM_HOKULEA_ELF] binary, relative to this file.
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_PATH: &str = "bin/kailua-fpvm-hokulea.bin";
/// RISC Zero image ID committing to [KAILUA_FPVM_HOKULEA_ELF].
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ID: [u32; 8] = [0xB581366F, 0x37F2C5E, 0xC0FCAE8B, 0x28E3EC73, 0x52B4079, 0x5B658FD, 0x7EA8F5C, 0xCAABD1CB];

/// FPVM guest program variant with Celestia DA support via hana.
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-hana.bin");
/// Path to the [KAILUA_FPVM_HANA_ELF] binary, relative to this file.
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_PATH: &str = "bin/kailua-fpvm-hana.bin";
/// RISC Zero image ID committing to [KAILUA_FPVM_HANA_ELF].
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ID: [u32; 8] = [0x77742D3F, 0x98C01461, 0xD423D397, 0xF7E80B0E, 0xEE2E80CD, 0x5EF90C20, 0x28B3733D, 0xD5ED120A];
