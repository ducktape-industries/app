//! App-owned slices of module wire contracts.
//!
//! These are deliberately private: the app speaks only the fields and codecs
//! it consumes. The module repos remain owners of the complete interfaces and
//! compatibility fixtures.

pub(crate) mod chat;
pub(crate) mod files;
pub(crate) mod gateway;
pub(crate) mod identity;
