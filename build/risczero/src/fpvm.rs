
pub const KAILUA_FPVM_KONA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-kona.bin");
pub const KAILUA_FPVM_KONA_PATH: &str = "bin/kailua-fpvm-kona.bin";
pub const KAILUA_FPVM_KONA_ID: [u32; 8] = [0x9196CC4C, 0xA979FC6D, 0xA9112A40, 0xC44BE37A, 0xF92B0150, 0x2D259030, 0x29C0CDBF, 0xF8EBA1CD];

#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-hokulea.bin");
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_PATH: &str = "bin/kailua-fpvm-hokulea.bin";
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ID: [u32; 8] = [0x64EBC02E, 0xABBB2062, 0x1B3BEDC, 0x5086D22B, 0x91336F83, 0xE8183DE, 0xE2AC4973, 0xEE6EC4F9];

#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-hana.bin");
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_PATH: &str = "bin/kailua-fpvm-hana.bin";
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ID: [u32; 8] = [0x659CF9DE, 0xD224A199, 0x1B673E98, 0xE184AAAE, 0x2A79136E, 0x698C600C, 0xBCBEDFB6, 0x9F8C85B1];
