
pub const KAILUA_FPVM_KONA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-kona.bin");
pub const KAILUA_FPVM_KONA_PATH: &str = "bin/kailua-fpvm-kona.bin";
pub const KAILUA_FPVM_KONA_ID: [u32; 8] = [0x70F0D98D, 0xF854BB86, 0x95E5E86F, 0x82D24CED, 0x414324AA, 0x36521E2B, 0x8AAABD3, 0x8BCFCF5];

#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-hokulea.bin");
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_PATH: &str = "bin/kailua-fpvm-hokulea.bin";
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ID: [u32; 8] = [0xE62DFFD0, 0x53CCFDE9, 0x2A765A45, 0x5C9793CC, 0x89CEED39, 0x8F251EB4, 0x11F0AF86, 0xDE0A3459];

#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-hana.bin");
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_PATH: &str = "bin/kailua-fpvm-hana.bin";
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ID: [u32; 8] = [0xA7E72B29, 0xE5C58DBA, 0xB3417077, 0xA2F8E253, 0x157FDB9E, 0x1FDBAFEC, 0xA367A5A7, 0x18A47B7D];
