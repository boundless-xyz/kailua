
pub const KAILUA_FPVM_KONA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-kona.bin");
pub const KAILUA_FPVM_KONA_PATH: &str = "bin/kailua-fpvm-kona.bin";
pub const KAILUA_FPVM_KONA_ID: [u32; 8] = [0xADF28821, 0x54A04599, 0x35565376, 0x52CAEFD1, 0xEBAC75D, 0xBC97D089, 0x287FE46B, 0x5630A5C4];

#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-hokulea.bin");
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_PATH: &str = "bin/kailua-fpvm-hokulea.bin";
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ID: [u32; 8] = [0x9729298, 0xCD8CBEDD, 0x911BEF3F, 0x8EA03999, 0xE00ACDF5, 0xEBAE9822, 0xD316457, 0xED5BB3A7];

#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-hana.bin");
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_PATH: &str = "bin/kailua-fpvm-hana.bin";
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ID: [u32; 8] = [0xBAF74B5C, 0x4311C1AD, 0x56C8DD3B, 0x2BA42C08, 0x6C1F8D4C, 0xF331F44D, 0x39B3C2, 0xA0EDA468];
