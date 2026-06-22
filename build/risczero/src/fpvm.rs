
pub const KAILUA_FPVM_KONA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-kona.bin");
pub const KAILUA_FPVM_KONA_PATH: &str = "bin/kailua-fpvm-kona.bin";
pub const KAILUA_FPVM_KONA_ID: [u32; 8] = [0x51B1E2B2, 0x1EEA803E, 0x518E998F, 0x54778EBF, 0xBD1DC2EE, 0xB6E06304, 0x5B1615B1, 0xBFC2BAA6];

#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-hokulea.bin");
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_PATH: &str = "bin/kailua-fpvm-hokulea.bin";
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ID: [u32; 8] = [0xDF97C0D3, 0xB33B58EC, 0xB5FCEE05, 0x13C3DDDC, 0x72E372B0, 0x136EE6CE, 0xFB372C49, 0xBA9E650];

#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-hana.bin");
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_PATH: &str = "bin/kailua-fpvm-hana.bin";
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ID: [u32; 8] = [0x77742D3F, 0x98C01461, 0xD423D397, 0xF7E80B0E, 0xEE2E80CD, 0x5EF90C20, 0x28B3733D, 0xD5ED120A];
