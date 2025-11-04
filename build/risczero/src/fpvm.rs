
pub const KAILUA_FPVM_KONA_ELF: &[u8] = include_bytes!("kailua-fpvm-kona.bin");
pub const KAILUA_FPVM_KONA_PATH: &str = "./kailua-fpvm-kona.bin";
pub const KAILUA_FPVM_KONA_ID: [u32; 8] = [0xAA808B2D, 0xC14B07B, 0x1B3D0D89, 0xF862A446, 0x6639B694, 0x20D8FDC1, 0x8B2CE643, 0x28CD49F3];

#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ELF: &[u8] = include_bytes!("kailua-fpvm-hokulea.bin");
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_PATH: &str = "./kailua-fpvm-hokulea.bin";
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ID: [u32; 8] = [0x33803669, 0xE3214291, 0xB5DCB3AA, 0xF46254D3, 0xA73D16E8, 0x16B62704, 0x5E3B654C, 0x1523A6F1];

#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ELF: &[u8] = include_bytes!("kailua-fpvm-hana.bin");
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_PATH: &str = "./kailua-fpvm-hana.bin";
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ID: [u32; 8] = [0xF2FFD880, 0x2EC14A5F, 0xCC956E9A, 0xBFA1E97E, 0x1D92F719, 0x5A4F184D, 0x697EC7F1, 0x165B6204];
