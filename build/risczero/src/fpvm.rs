
pub const KAILUA_FPVM_KONA_ELF: &[u8] = include_bytes!("kailua-fpvm-kona.bin");
pub const KAILUA_FPVM_KONA_PATH: &str = "./kailua-fpvm-kona.bin";
pub const KAILUA_FPVM_KONA_ID: [u32; 8] = [0x9F03989A, 0x1A57C3F7, 0xD48DBB6D, 0xAEA325F3, 0x7B514FFC, 0xC24E19E2, 0x8A2B42CC, 0x5114FD35];

#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ELF: &[u8] = include_bytes!("kailua-fpvm-hokulea.bin");
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_PATH: &str = "./kailua-fpvm-hokulea.bin";
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ID: [u32; 8] = [0x4390D4F3, 0x550F9BE8, 0x7C1E3923, 0xB23B7175, 0x9429C4EF, 0x6CC5F695, 0xAF62E635, 0x8A6AC648];

#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ELF: &[u8] = include_bytes!("kailua-fpvm-hana.bin");
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_PATH: &str = "./kailua-fpvm-hana.bin";
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ID: [u32; 8] = [0x6EEC4977, 0xA19467DF, 0xC893E075, 0x702F100B, 0x76BEE0D4, 0x2EE88847, 0x675FC164, 0xA2FE499B];
