//! Administrative operations, grouped by the resource they manage.
mod catalogue;
pub(super) use catalogue::*;
mod configuration;
pub(super) use configuration::*;
mod settings;
pub(super) use settings::*;
mod activation;
pub(super) use activation::*;
mod progress;
pub(super) use progress::*;
mod sessions;
pub(super) use sessions::*;
mod replay;
pub(super) use replay::*;
mod data;
pub(super) use data::*;
mod dashboard;
pub(super) use dashboard::*;
