//! Administrative operations, grouped by the resource they manage.
//!
//! The router mounts these handlers behind administrator authentication. Each handler
//! translates a request for its resource into a store or runtime operation and returns a
//! dashboard response. Session replay, timeline grouping and privacy reporting each own
//! their projection rules.
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
