
pub const KAILUA_FPVM_KONA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-kona.bin");
pub const KAILUA_FPVM_KONA_PATH: &str = "bin/kailua-fpvm-kona.bin";
pub const KAILUA_FPVM_KONA_ID: [u32; 8] = [0x9E57B6C, 0xFD6C0ED2, 0x8F4CFF05, 0xC7C0EE91, 0xD23799BA, 0xE8C25109, 0x36FB3F35, 0xB755A3A6];

#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-hokulea.bin");
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_PATH: &str = "bin/kailua-fpvm-hokulea.bin";
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ID: [u32; 8] = [0x2F3855FB, 0x57B627D9, 0x73532C3C, 0x4901D023, 0x57F007E8, 0xEF3ECC3A, 0xF2804F11, 0x60B1017A];

#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-hana.bin");
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_PATH: &str = "bin/kailua-fpvm-hana.bin";
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ID: [u32; 8] = [0x5ABB041, 0x78C7CA7F, 0x9F9D4E55, 0x1961D87B, 0x2B3322E5, 0xF11C13EC, 0xC8834691, 0x553CFEEF];
