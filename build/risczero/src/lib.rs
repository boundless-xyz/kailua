#[cfg(feature = "rebuild-fpvm")]
include!(concat!(env!("OUT_DIR"), "/methods.rs"));

#[cfg(not(any(feature = "rebuild-fpvm", feature = "experimental")))]
include!("fpvm.rs");

#[cfg(all(not(feature = "rebuild-fpvm"), feature = "experimental"))]
include!("fpvm-experimental.rs");
