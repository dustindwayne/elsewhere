pub mod agents;
pub mod knowledge;
pub mod model;
pub mod network;
pub mod signatures;
pub mod store;
pub mod templates;
pub mod vault;

pub type Result<T> = std::result::Result<T, String>;

pub mod web;
