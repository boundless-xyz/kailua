#[cfg(feature = "rebuild-fpvm")]
include!(concat!(env!("OUT_DIR"), "/methods.rs"));

#[cfg(not(any(
    feature = "rebuild-fpvm",
    feature = "enable-experimental-transaction-stitching"
)))]
include!("fpvm.rs");

#[cfg(all(
    not(feature = "rebuild-fpvm"),
    feature = "enable-experimental-transaction-stitching"
))]
include!("fpvm-experimental.rs");
