#[cfg(feature = "rebuild-fpvm")]
include!(concat!(env!("OUT_DIR"), "/methods.rs"));

#[cfg(not(feature = "rebuild-fpvm"))]
include!("fpvm.rs");

#[cfg(all(feature = "eigen", feature = "rebuild-da"))]
pub use canoe_steel_methods::CERT_VERIFICATION_ELF as KAILUA_DA_HOKULEA_ELF;
#[cfg(all(feature = "eigen", feature = "rebuild-da"))]
pub use canoe_steel_methods::CERT_VERIFICATION_ID as KAILUA_DA_HOKULEA_ID;
#[cfg(all(feature = "eigen", feature = "rebuild-da"))]
pub use canoe_steel_methods::CERT_VERIFICATION_PATH as KAILUA_DA_HOKULEA_PATH;

#[cfg(all(feature = "eigen", not(feature = "rebuild-da")))]
include!("./da.rs");
