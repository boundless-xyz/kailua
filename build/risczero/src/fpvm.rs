
pub const KAILUA_FPVM_KONA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-kona.bin");
pub const KAILUA_FPVM_KONA_PATH: &str = "bin/kailua-fpvm-kona.bin";
pub const KAILUA_FPVM_KONA_ID: [u32; 8] = [0x72C20E34, 0x6BDBE0F1, 0xCD77E418, 0x3866F8FA, 0xB8266D1B, 0x92FB0299, 0x1117788D, 0xC198DC28];

#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-hokulea.bin");
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_PATH: &str = "bin/kailua-fpvm-hokulea.bin";
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ID: [u32; 8] = [0xAE43BF06, 0xDAA7E999, 0xC07F54B9, 0xF1D30E41, 0x979E5B5B, 0x9F6B9A83, 0xF66C7560, 0x5EDECD1E];

#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-hana.bin");
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_PATH: &str = "bin/kailua-fpvm-hana.bin";
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ID: [u32; 8] = [0xB8A83D29, 0x414BD84E, 0xCE5CD0B5, 0x5E4D7077, 0x6E0F7E8C, 0xB8EBCC67, 0xBEC0BB90, 0xC1C8966A];
