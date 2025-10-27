
pub const KAILUA_FPVM_KONA_ELF: &[u8] = include_bytes!("kailua-fpvm-kona.bin");
pub const KAILUA_FPVM_KONA_PATH: &str = "./kailua-fpvm-kona.bin";
pub const KAILUA_FPVM_KONA_ID: [u32; 8] = [0xE009385F, 0x6A09E9B8, 0x4B7C64A4, 0x52366B8B, 0xB3FF34AE, 0x46E09206, 0x7F375D84, 0x6ED0C68A];

#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ELF: &[u8] = include_bytes!("kailua-fpvm-hokulea.bin");
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_PATH: &str = "./kailua-fpvm-hokulea.bin";
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ID: [u32; 8] = [0x24994F6B, 0x6B6CFC76, 0x28BF2AB2, 0xA5B6C512, 0x7A3FD6AC, 0x4FC7352, 0xC02597B9, 0xF4DBDED];

#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ELF: &[u8] = include_bytes!("kailua-fpvm-hana.bin");
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_PATH: &str = "./kailua-fpvm-hana.bin";
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ID: [u32; 8] = [0x37ADAE78, 0xBA13E452, 0x52816EA5, 0xE5147E74, 0x9D54A6DF, 0x19117573, 0x762C5B26, 0x20C10DCD];
