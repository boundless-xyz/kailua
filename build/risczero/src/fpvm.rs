
pub const KAILUA_FPVM_KONA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-kona.bin");
pub const KAILUA_FPVM_KONA_PATH: &str = "bin/kailua-fpvm-kona.bin";
pub const KAILUA_FPVM_KONA_ID: [u32; 8] = [0x138124D, 0xABFDE5B9, 0xE42798A1, 0xECA151C3, 0x26568F4E, 0x37B37C1C, 0x41D191F5, 0xD3004EF0];

#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-hokulea.bin");
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_PATH: &str = "bin/kailua-fpvm-hokulea.bin";
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ID: [u32; 8] = [0xA8B2D2B7, 0x4E1062B3, 0x54DCD0F4, 0x9399925D, 0x947694B1, 0x33835D7D, 0xA57AEE4, 0x56F7DD8E];

#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-hana.bin");
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_PATH: &str = "bin/kailua-fpvm-hana.bin";
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ID: [u32; 8] = [0x3BE9DC39, 0xDFB50747, 0xD710ECEF, 0x736207EA, 0x5F7EBBB, 0xAA6527, 0x66780F5A, 0xBC1DCCBF];
