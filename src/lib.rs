pub mod admin;
pub mod config;
pub mod databases;
pub mod error;
pub mod gateway;
pub mod i18n;
pub mod model;
pub mod protocol;
pub mod sync;
pub mod tui;
mod upstream;
pub mod vault;
pub mod webdav;

#[cfg(test)]
mod gateway_tests;
pub mod library;
#[cfg(test)]
mod protocol_tests;
#[cfg(test)]
mod sync_tests;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod webdav_tests;
