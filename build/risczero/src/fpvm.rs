
/// FPVM guest program proving OP Stack state transitions via kona.
pub const KAILUA_FPVM_KONA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-kona.bin");
/// Path to the [KAILUA_FPVM_KONA_ELF] binary, relative to this file.
pub const KAILUA_FPVM_KONA_PATH: &str = "bin/kailua-fpvm-kona.bin";
/// RISC Zero image ID committing to [KAILUA_FPVM_KONA_ELF].
pub const KAILUA_FPVM_KONA_ID: [u32; 8] = [0x31C97CD4, 0x54A69398, 0xD44E77CB, 0xE5EA6BA6, 0xC346F05, 0xA3F63F0C, 0x78121727, 0xBE8B901A];

/// FPVM guest program variant with EigenDA support via hokulea.
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-hokulea.bin");
/// Path to the [KAILUA_FPVM_HOKULEA_ELF] binary, relative to this file.
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_PATH: &str = "bin/kailua-fpvm-hokulea.bin";
/// RISC Zero image ID committing to [KAILUA_FPVM_HOKULEA_ELF].
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ID: [u32; 8] = [0x20260FA0, 0xEE4CE6A1, 0x5AA1E1F4, 0x42D151E, 0xF707E1D5, 0x17A1DBAF, 0x923D0674, 0x471C877B];

/// FPVM guest program variant with Celestia DA support via hana.
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-hana.bin");
/// Path to the [KAILUA_FPVM_HANA_ELF] binary, relative to this file.
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_PATH: &str = "bin/kailua-fpvm-hana.bin";
/// RISC Zero image ID committing to [KAILUA_FPVM_HANA_ELF].
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ID: [u32; 8] = [0x8F35C637, 0x5523B572, 0x1FDCC613, 0xFD180CC9, 0x9D691D3B, 0xE0842FD3, 0xD3C68777, 0x7A46129F];
