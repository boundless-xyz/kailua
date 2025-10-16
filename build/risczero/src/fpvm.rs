
pub const KAILUA_FPVM_KONA_ELF: &[u8] = include_bytes!("kailua-fpvm-kona.bin");
pub const KAILUA_FPVM_KONA_PATH: &str = "./kailua-fpvm-kona.bin";
pub const KAILUA_FPVM_KONA_ID: [u32; 8] = [0xB4D195E0, 0x305D7039, 0x915D4578, 0x153CC8CB, 0x46B306A8, 0x769DD158, 0x69C191F6, 0x2883BB6E];

#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ELF: &[u8] = include_bytes!("kailua-fpvm-hokulea.bin");
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_PATH: &str = "./kailua-fpvm-hokulea.bin";
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ID: [u32; 8] = [0xA42A16A5, 0x2F6AFCB3, 0xDB3B4491, 0x19BBC039, 0xE61671C7, 0xD448135C, 0x3149A58A, 0xC80015C6];

#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ELF: &[u8] = include_bytes!("kailua-fpvm-hana.bin");
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_PATH: &str = "./kailua-fpvm-hana.bin";
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ID: [u32; 8] = [0x74CE5EE, 0x6E290ACB, 0x73E79E05, 0x7AB037CC, 0x4F7297A3, 0x8FB23FC3, 0xF85163A1, 0x9B0F53AA];
