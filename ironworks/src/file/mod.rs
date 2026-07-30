//! Collection of pre-defined file readers for known file formats.
//!
//! Each file type may contain a number of related supporting items, and as such are namespaced seperately.

#[cfg(any(feature = "eqp", feature = "gmp"))]
mod block_table;
mod file;

#[cfg(feature = "eqdp")]
pub mod eqdp;
#[cfg(feature = "eqp")]
pub mod eqp;
#[cfg(feature = "est")]
pub mod est;
#[cfg(feature = "evp")]
pub mod evp;
#[cfg(feature = "exd")]
pub mod exd;
#[cfg(feature = "exh")]
pub mod exh;
#[cfg(feature = "exl")]
pub mod exl;
#[cfg(feature = "fdt")]
pub mod fdt;
#[cfg(feature = "gfd")]
pub mod gfd;
#[cfg(feature = "gmp")]
pub mod gmp;
#[cfg(feature = "imc")]
pub mod imc;
#[cfg(feature = "mdl")]
pub mod mdl;
#[cfg(feature = "mtrl")]
pub mod mtrl;
#[cfg(feature = "patch")]
pub mod patch;
#[cfg(feature = "pbd")]
pub mod pbd;
#[cfg(feature = "scd")]
pub mod scd;
#[cfg(feature = "shpk")]
pub mod shpk;
#[cfg(feature = "sklb")]
pub mod sklb;
#[cfg(feature = "tex")]
pub mod tex;
#[cfg(feature = "uld")]
pub mod uld;

pub use file::File;
