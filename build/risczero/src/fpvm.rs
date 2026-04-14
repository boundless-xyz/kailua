
pub const KAILUA_FPVM_KONA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-kona.bin");
pub const KAILUA_FPVM_KONA_PATH: &str = "bin/kailua-fpvm-kona.bin";
pub const KAILUA_FPVM_KONA_ID: [u32; 8] = [0x3AE854C4, 0xAA0854B5, 0x98A71035, 0x2D307420, 0x39004C2A, 0x71100C55, 0xC3A104ED, 0x552E0CD1];

#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-hokulea.bin");
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_PATH: &str = "bin/kailua-fpvm-hokulea.bin";
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ID: [u32; 8] = [0xA57EF827, 0xEACBD926, 0xEADF7E6E, 0x76D9EF4C, 0xDF14E303, 0xC0DC48FA, 0xE3B1BE88, 0xD9D3407A];

#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-hana.bin");
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_PATH: &str = "bin/kailua-fpvm-hana.bin";
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ID: [u32; 8] = [0x266C77A4, 0x1F9719B3, 0x638E950B, 0x7A8BD025, 0x348D3CD9, 0xD55AF3F, 0xCE17D464, 0xC8B78E71];
