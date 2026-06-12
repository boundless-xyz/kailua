
pub const KAILUA_FPVM_KONA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-kona-experimental.bin");
pub const KAILUA_FPVM_KONA_PATH: &str = "bin/kailua-fpvm-kona-experimental.bin";
pub const KAILUA_FPVM_KONA_ID: [u32; 8] = [0x2E116711, 0x9821D27B, 0xBDF5B489, 0xA556E6F0, 0xCE69B0B1, 0x802DF93D, 0xB77B4789, 0x631E19E];

#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-hokulea-experimental.bin");
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_PATH: &str = "bin/kailua-fpvm-hokulea-experimental.bin";
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ID: [u32; 8] = [0xBAF40981, 0x63A00C83, 0x385C60D4, 0x22C3A670, 0x120E057D, 0x208BA345, 0xD9C0D515, 0xB84908CD];

#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-hana-experimental.bin");
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_PATH: &str = "bin/kailua-fpvm-hana-experimental.bin";
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ID: [u32; 8] = [0x510FCDE1, 0xC52594E0, 0xF2CA53C1, 0xDFE8CD65, 0xDC3F603A, 0x478009FA, 0xCBB4BE9, 0xC9E99DE3];
