
pub const KAILUA_FPVM_KONA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-kona.bin");
pub const KAILUA_FPVM_KONA_PATH: &str = "bin/kailua-fpvm-kona.bin";
pub const KAILUA_FPVM_KONA_ID: [u32; 8] = [0xCFD17B04, 0x92B38CC7, 0x9A1A9540, 0x360000FF, 0x71C216BC, 0x40743C85, 0xB399CAFC, 0x8F2BA8EF];

#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-hokulea.bin");
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_PATH: &str = "bin/kailua-fpvm-hokulea.bin";
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ID: [u32; 8] = [0x2E3E3CCD, 0xD967AA7F, 0x78434755, 0x6777A9C, 0x44757E6, 0x725C00AC, 0x5CEF42A1, 0xC9D19D9E];

#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-hana.bin");
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_PATH: &str = "bin/kailua-fpvm-hana.bin";
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ID: [u32; 8] = [0x451BE09E, 0x10272A15, 0xA2151B12, 0x821AEDDC, 0x4CD9350E, 0xCE3A6A3B, 0x37ADD099, 0xC14B9F60];
