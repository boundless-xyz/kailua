
pub const KAILUA_FPVM_KONA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-kona-experimental.bin");
pub const KAILUA_FPVM_KONA_PATH: &str = "bin/kailua-fpvm-kona-experimental.bin";
pub const KAILUA_FPVM_KONA_ID: [u32; 8] = [0x43C2FC4D, 0x1FF13783, 0xE7B8A15A, 0x52F31B0E, 0x9486A31F, 0xE52A899A, 0xBF03CFD8, 0xAE855129];

#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-hokulea-experimental.bin");
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_PATH: &str = "bin/kailua-fpvm-hokulea-experimental.bin";
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ID: [u32; 8] = [0x67D76F65, 0x524FCF7E, 0x109B4F60, 0x13551CB2, 0x76B68323, 0x9AEAC1D2, 0x1A66D2F0, 0xECE95C06];

#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-hana-experimental.bin");
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_PATH: &str = "bin/kailua-fpvm-hana-experimental.bin";
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ID: [u32; 8] = [0xDB5601C6, 0xC4E06009, 0xAB04BAA9, 0xAA7BC62, 0x4AECBA75, 0x1F6D0B30, 0xFB7CFE45, 0xAEA56B25];
