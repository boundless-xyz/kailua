
pub const KAILUA_FPVM_KONA_ELF: &[u8] = include_bytes!("kailua-fpvm-kona.bin");
pub const KAILUA_FPVM_KONA_PATH: &str = "./kailua-fpvm-kona.bin";
pub const KAILUA_FPVM_KONA_ID: [u32; 8] = [0x9C5A0507, 0x8EBAE4C0, 0xD269108A, 0x3FB72FE6, 0xFD4B1CB7, 0x8FAE4630, 0xBA9327A9, 0x20648AD1];

#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ELF: &[u8] = include_bytes!("kailua-fpvm-hokulea.bin");
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_PATH: &str = "./kailua-fpvm-hokulea.bin";
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ID: [u32; 8] = [0x707391AF, 0xD7CB8D93, 0x72EF08CD, 0x296FE1B3, 0x5F9EFD83, 0xA01260FC, 0x3A87EA93, 0xA2801520];

#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ELF: &[u8] = include_bytes!("kailua-fpvm-hana.bin");
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_PATH: &str = "./kailua-fpvm-hana.bin";
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ID: [u32; 8] = [0x5C065281, 0x18FCEBDA, 0xC2061B35, 0x903E3C92, 0x305D13B5, 0x5322A10E, 0x9310CC83, 0x363CC3BC];
