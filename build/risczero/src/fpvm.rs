
pub const KAILUA_FPVM_KONA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-kona.bin");
pub const KAILUA_FPVM_KONA_PATH: &str = "bin/kailua-fpvm-kona.bin";
pub const KAILUA_FPVM_KONA_ID: [u32; 8] = [0x241FCE22, 0x1E8CF549, 0xBF30E6E1, 0x234ED663, 0xD0D226C3, 0xADFE47F4, 0xDC7E807E, 0xB64447DF];

#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-hokulea.bin");
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_PATH: &str = "bin/kailua-fpvm-hokulea.bin";
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ID: [u32; 8] = [0x545E4346, 0xC237BC52, 0x20AEAEB6, 0xED509E50, 0xCB396262, 0x613E274, 0x8C2F491, 0xA79E39A1];

#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-hana.bin");
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_PATH: &str = "bin/kailua-fpvm-hana.bin";
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ID: [u32; 8] = [0xA532CD9A, 0xF3AD7D3A, 0x5F1DE3D7, 0x521DF5D0, 0xA8EAFF3D, 0x68C9406, 0x1BE714F9, 0x5CFB9135];
