
/// FPVM guest program proving OP Stack state transitions via kona.
pub const KAILUA_FPVM_KONA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-kona.bin");
/// Path to the [KAILUA_FPVM_KONA_ELF] binary, relative to this file.
pub const KAILUA_FPVM_KONA_PATH: &str = "bin/kailua-fpvm-kona.bin";
/// RISC Zero image ID committing to [KAILUA_FPVM_KONA_ELF].
pub const KAILUA_FPVM_KONA_ID: [u32; 8] = [0xBA46C00B, 0x9228532C, 0x9B818E8, 0x17EE812E, 0x71C7C3BF, 0x22102371, 0xB968E213, 0x68865CEA];

/// FPVM guest program variant with EigenDA support via hokulea.
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-hokulea.bin");
/// Path to the [KAILUA_FPVM_HOKULEA_ELF] binary, relative to this file.
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_PATH: &str = "bin/kailua-fpvm-hokulea.bin";
/// RISC Zero image ID committing to [KAILUA_FPVM_HOKULEA_ELF].
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ID: [u32; 8] = [0x1C94D8BB, 0xD824F947, 0x919DE395, 0x21797A8B, 0xF441C1C9, 0x23E188DE, 0x7ADA592E, 0x8754FC93];

/// FPVM guest program variant with Celestia DA support via hana.
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-hana.bin");
/// Path to the [KAILUA_FPVM_HANA_ELF] binary, relative to this file.
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_PATH: &str = "bin/kailua-fpvm-hana.bin";
/// RISC Zero image ID committing to [KAILUA_FPVM_HANA_ELF].
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ID: [u32; 8] = [0xAD0BCE5D, 0x5B6C432B, 0xC2533FC2, 0xC8C9D5C9, 0x74BEBFD5, 0xFD4E979, 0xCA940DD5, 0x90258C3F];
