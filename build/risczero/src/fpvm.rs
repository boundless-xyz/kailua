
pub const KAILUA_FPVM_KONA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-kona.bin");
pub const KAILUA_FPVM_KONA_PATH: &str = "bin/kailua-fpvm-kona.bin";
pub const KAILUA_FPVM_KONA_ID: [u32; 8] = [0xC5F22561, 0x82112522, 0xA5C60DC8, 0xE7983B8C, 0xE7C68CE5, 0xDEDC8236, 0x97B9DFDA, 0xC4A3C436];

#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-hokulea.bin");
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_PATH: &str = "bin/kailua-fpvm-hokulea.bin";
#[cfg(feature = "eigen")]
pub const KAILUA_FPVM_HOKULEA_ID: [u32; 8] = [0x7B89406C, 0xAB6A6D65, 0x1716F947, 0x9D4131C9, 0x86F8D895, 0x27666E4E, 0x4EF3D138, 0xC41D3861];

#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ELF: &[u8] = include_bytes!("bin/kailua-fpvm-hana.bin");
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_PATH: &str = "bin/kailua-fpvm-hana.bin";
#[cfg(feature = "celestia")]
pub const KAILUA_FPVM_HANA_ID: [u32; 8] = [0x4C05DDB4, 0x61FAA4C3, 0x5C8CF7AE, 0xE94D56F0, 0x6298FE0B, 0x18F56791, 0xDB0449AA, 0x130A953];
