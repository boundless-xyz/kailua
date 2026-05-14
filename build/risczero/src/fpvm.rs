
pub const KAILUA_FPVM_KONA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-kona.bin");
pub const KAILUA_FPVM_KONA_PATH: &str = "bin/kailua-fpvm-kona.bin";
pub const KAILUA_FPVM_KONA_ID: [u32; 8] = [0x4D025893, 0xCCABBDA6, 0xAAE99808, 0xF2272401, 0x132477CC, 0xB1857BDC, 0xB4A1D7F, 0xD24B5C21];

#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-hokulea.bin");
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_PATH: &str = "bin/kailua-fpvm-hokulea.bin";
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ID: [u32; 8] = [0xB7B2F9CD, 0x5517984, 0x4458F95A, 0xE6AE8496, 0x41127CD4, 0xCBBC89CD, 0x9D7E9C52, 0x17CD9684];

#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-hana.bin");
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_PATH: &str = "bin/kailua-fpvm-hana.bin";
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ID: [u32; 8] = [0x7B2E4CC0, 0x951BF44A, 0x8F702ACC, 0x1F49FF75, 0x73624CA5, 0x7D1D3B7A, 0x6B1E7157, 0xC706DF48];
