
pub const KAILUA_FPVM_KONA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-kona-experimental.bin");
pub const KAILUA_FPVM_KONA_PATH: &str = "bin/kailua-fpvm-kona-experimental.bin";
pub const KAILUA_FPVM_KONA_ID: [u32; 8] = [0xAAAFCD51, 0x42EA2EC8, 0x127951D6, 0xCFDF1284, 0xC54A6469, 0xB7213B4D, 0x72A2C599, 0x8CB3D39];

#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-hokulea-experimental.bin");
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_PATH: &str = "bin/kailua-fpvm-hokulea-experimental.bin";
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ID: [u32; 8] = [0x6A82D240, 0x4A6678A, 0x50EEDF7, 0xC6BB6186, 0x1FF12F52, 0x4E8B31FC, 0xB532449E, 0xB0F11187];

#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-hana-experimental.bin");
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_PATH: &str = "bin/kailua-fpvm-hana-experimental.bin";
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ID: [u32; 8] = [0xDAAA5EB1, 0xB09EB3B1, 0x465228EF, 0xC18034A, 0xFC28C1EB, 0x640F2042, 0xA512A07C, 0x1596FC88];
