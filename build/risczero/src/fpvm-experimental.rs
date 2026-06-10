
pub const KAILUA_FPVM_KONA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-kona-experimental.bin");
pub const KAILUA_FPVM_KONA_PATH: &str = "bin/kailua-fpvm-kona-experimental.bin";
pub const KAILUA_FPVM_KONA_ID: [u32; 8] = [0x2E116711, 0x9821D27B, 0xBDF5B489, 0xA556E6F0, 0xCE69B0B1, 0x802DF93D, 0xB77B4789, 0x631E19E];

#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-hokulea-experimental.bin");
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_PATH: &str = "bin/kailua-fpvm-hokulea-experimental.bin";
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ID: [u32; 8] = [0xFCF48B96, 0x5322B527, 0x92EFA97B, 0x8DD49313, 0xEFF4FFFE, 0x859CE9EB, 0xF57FA2D0, 0x131290C4];

#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-hana-experimental.bin");
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_PATH: &str = "bin/kailua-fpvm-hana-experimental.bin";
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ID: [u32; 8] = [0x52F173C, 0xBEBCA622, 0x3F56BC0A, 0xE0E3BAD4, 0xAB024A12, 0xA2A872EC, 0xCD87B2F6, 0x601FC2B1];
