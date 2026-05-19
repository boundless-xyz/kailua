
pub const KAILUA_FPVM_KONA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-kona.bin");
pub const KAILUA_FPVM_KONA_PATH: &str = "bin/kailua-fpvm-kona.bin";
pub const KAILUA_FPVM_KONA_ID: [u32; 8] = [0x5E5D41E7, 0xB209EAD8, 0x966DF8CA, 0x187E2527, 0x7B412835, 0xBB712869, 0x3E76BDBA, 0x7AFEB517];

#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-hokulea.bin");
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_PATH: &str = "bin/kailua-fpvm-hokulea.bin";
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ID: [u32; 8] = [0x1EABEE03, 0xC5BE81A4, 0xFF92147D, 0x382B17F7, 0x5EF8CDC5, 0xD094F84A, 0xFDEE9E1C, 0x4B708762];

#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-hana.bin");
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_PATH: &str = "bin/kailua-fpvm-hana.bin";
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ID: [u32; 8] = [0x68E49044, 0x52C29D4E, 0x7CF1E2E9, 0xD7E315A9, 0x380B35EF, 0x6928CC8D, 0xC1B92DBB, 0x97A76C06];
