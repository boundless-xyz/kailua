
pub const KAILUA_FPVM_KONA_ELF: &[u8] = include_bytes!("kailua-fpvm-kona.bin");
pub const KAILUA_FPVM_KONA_PATH: &str = "./kailua-fpvm-kona.bin";
pub const KAILUA_FPVM_KONA_ID: [u32; 8] = [0xFC59FEFB, 0x7BD8AAB0, 0x7FC08BE4, 0xC1C8AD6E, 0xE3E0E026, 0xB5619C98, 0xEDA443F9, 0x1850882F];

#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ELF: &[u8] = include_bytes!("kailua-fpvm-hokulea.bin");
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_PATH: &str = "./kailua-fpvm-hokulea.bin";
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ID: [u32; 8] = [0xF21E032D, 0xE08DD392, 0x2C23E2C4, 0xB48F0F43, 0xCABC3AA1, 0x86EDC6C8, 0xF0B1FC3B, 0x63F728FC];

#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ELF: &[u8] = include_bytes!("kailua-fpvm-hana.bin");
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_PATH: &str = "./kailua-fpvm-hana.bin";
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ID: [u32; 8] = [0xF2FFD880, 0x2EC14A5F, 0xCC956E9A, 0xBFA1E97E, 0x1D92F719, 0x5A4F184D, 0x697EC7F1, 0x165B6204];
