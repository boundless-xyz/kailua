
pub const KAILUA_FPVM_KONA_ELF: &[u8] = include_bytes!("kailua-fpvm-kona.bin");
pub const KAILUA_FPVM_KONA_PATH: &str = "./kailua-fpvm-kona.bin";
pub const KAILUA_FPVM_KONA_ID: [u32; 8] = [0xC46C4D11, 0xC30D8CB9, 0x2837F031, 0x8106DF60, 0xE620099D, 0xEC113F77, 0x18B59F2C, 0xA1F3FD3B];

#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ELF: &[u8] = include_bytes!("kailua-fpvm-hokulea.bin");
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_PATH: &str = "./kailua-fpvm-hokulea.bin";
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ID: [u32; 8] = [0x52936F75, 0x55CC14F9, 0x9C2B03E1, 0x8115B608, 0xDF55952C, 0x1EF93812, 0x1162452D, 0x53ECB4F2];

#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ELF: &[u8] = include_bytes!("kailua-fpvm-hana.bin");
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_PATH: &str = "./kailua-fpvm-hana.bin";
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ID: [u32; 8] = [0x4046712E, 0xCDFA0F8A, 0x9D954FA6, 0xE91FE711, 0xD8BED42F, 0x2C608914, 0xE127A123, 0x20C9F9D];
