
pub const KAILUA_FPVM_KONA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-kona.bin");
pub const KAILUA_FPVM_KONA_PATH: &str = "bin/kailua-fpvm-kona.bin";
pub const KAILUA_FPVM_KONA_ID: [u32; 8] = [0x22FFA4C7, 0xBE2ED9C7, 0x4784B6C, 0x8255696F, 0x11457944, 0xE8E36585, 0xA1CF8BFF, 0x2ACBF97B];

#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-hokulea.bin");
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_PATH: &str = "bin/kailua-fpvm-hokulea.bin";
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ID: [u32; 8] = [0xC0BBE097, 0xF1687616, 0x3D89DE6E, 0xB1AF997C, 0x5FB34C11, 0x4E03AD70, 0x6241B043, 0x8DCACE3E];

#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-hana.bin");
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_PATH: &str = "bin/kailua-fpvm-hana.bin";
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ID: [u32; 8] = [0xC1A45EBC, 0x160F66B6, 0x2097E233, 0x3EE196D8, 0x22700599, 0xD5ADFAEF, 0xA6F40AB5, 0x6BEF2182];
