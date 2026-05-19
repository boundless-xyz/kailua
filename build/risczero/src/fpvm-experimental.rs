
pub const KAILUA_FPVM_KONA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-kona-experimental.bin");
pub const KAILUA_FPVM_KONA_PATH: &str = "bin/kailua-fpvm-kona-experimental.bin";
pub const KAILUA_FPVM_KONA_ID: [u32; 8] = [0xFE9BC4CE, 0x5FECDC5B, 0x2A88182F, 0xCA29E9AB, 0x37210260, 0x56A85D1F, 0x88842988, 0x11663C6A];

#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-hokulea-experimental.bin");
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_PATH: &str = "bin/kailua-fpvm-hokulea-experimental.bin";
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ID: [u32; 8] = [0x336EA89A, 0xAD0FE6F6, 0x9DD14570, 0xE0172388, 0xA8FC8282, 0x2CCBE045, 0x236DF5C3, 0x3083F18D];

#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-hana-experimental.bin");
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_PATH: &str = "bin/kailua-fpvm-hana-experimental.bin";
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ID: [u32; 8] = [0x7A0F7DCB, 0x1FF54C74, 0x73E27E69, 0xEA2D417C, 0x9912B2AA, 0x325C08C8, 0x88F1EF26, 0xD8BFB0F8];
