pub mod admin;
pub mod ai_proxy;
pub mod api_keys;
#[cfg(test)]
mod api_keys_tests;
pub mod approval;
mod blobs;
pub mod clipboard;
pub mod config;
#[cfg(test)]
mod contract_tests;
pub mod credstore;
pub mod databases;
pub mod error;
pub mod gateway;
pub mod i18n;
pub mod install;
pub mod keys;
pub mod model;
mod object;
pub mod protocol;
pub mod segment;
pub mod service_api;
pub mod sync;
pub mod tiga;
pub mod tui;
mod upstream;
pub mod vault;
pub mod webdav;

#[cfg(test)]
mod gateway_tests;
pub mod library;
pub mod mdbx;
#[cfg(test)]
mod protocol_tests;
#[cfg(test)]
mod sync_tests;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod webdav_tests;
