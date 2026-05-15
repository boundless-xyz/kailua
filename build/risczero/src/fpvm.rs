
pub const KAILUA_FPVM_KONA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-kona.bin");
pub const KAILUA_FPVM_KONA_PATH: &str = "bin/kailua-fpvm-kona.bin";
pub const KAILUA_FPVM_KONA_ID: [u32; 8] = [0xBE489EA1, 0x93E8B071, 0x5EB1213D, 0xDEF2EDA2, 0xE2B8368B, 0xA8E6C61C, 0xC1CC567, 0x6FF8064];

#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-hokulea.bin");
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_PATH: &str = "bin/kailua-fpvm-hokulea.bin";
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ID: [u32; 8] = [0xA37519B1, 0x59ECE5AF, 0xA360C45F, 0x1CF58FCD, 0x3AF027C7, 0x7A0451F8, 0xB8B863A6, 0x11518795];

#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-hana.bin");
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_PATH: &str = "bin/kailua-fpvm-hana.bin";
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ID: [u32; 8] = [0xEAACB54F, 0xC92AC27B, 0x23280694, 0x30652E35, 0x296E66B9, 0xE35DEB14, 0x40FA45E1, 0x362C89EB];
