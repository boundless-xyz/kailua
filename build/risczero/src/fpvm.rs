
pub const KAILUA_FPVM_KONA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-kona.bin");
pub const KAILUA_FPVM_KONA_PATH: &str = "bin/kailua-fpvm-kona.bin";
pub const KAILUA_FPVM_KONA_ID: [u32; 8] = [0x91B64DE3, 0xCB52494A, 0x2431FAE9, 0xC1AA4A5, 0x318A240A, 0x24CD74C3, 0x7620BCAD, 0x164AB955];

#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-hokulea.bin");
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_PATH: &str = "bin/kailua-fpvm-hokulea.bin";
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ID: [u32; 8] = [0x71BCE7C2, 0xF8CA10AE, 0x73A0A106, 0xAF0D17E4, 0x10C34A3F, 0x54F8F199, 0xD72D06F7, 0xE32FD10E];

#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-hana.bin");
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_PATH: &str = "bin/kailua-fpvm-hana.bin";
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ID: [u32; 8] = [0x788864D2, 0xA3503ADF, 0xCF03B907, 0x9ED9F4FE, 0x2E585864, 0xE99C12F2, 0x3F145764, 0x8441629F];
