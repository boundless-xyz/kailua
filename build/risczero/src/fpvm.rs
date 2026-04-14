
pub const KAILUA_FPVM_KONA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-kona.bin");
pub const KAILUA_FPVM_KONA_PATH: &str = "bin/kailua-fpvm-kona.bin";
pub const KAILUA_FPVM_KONA_ID: [u32; 8] = [0x17238812, 0xF8BA9621, 0xE4DE0B81, 0x4C0EF4E1, 0x30C215A9, 0xE506FF0B, 0xD920E502, 0x75A45EAE];

#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-hokulea.bin");
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_PATH: &str = "bin/kailua-fpvm-hokulea.bin";
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ID: [u32; 8] = [0x9D829154, 0xDE8C0CA5, 0xFD277A88, 0x8C81AE76, 0xEA89AE0E, 0xB1B6F353, 0x6B136B11, 0x4E634733];

#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-hana.bin");
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_PATH: &str = "bin/kailua-fpvm-hana.bin";
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ID: [u32; 8] = [0x339A5785, 0xBCE13812, 0x6895108D, 0xCA8BDE93, 0xBE970434, 0x8B35789F, 0x9EBDC4E, 0xF21CF77C];
