#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod build;
mod dict;
mod domain;
mod format;
mod frontcode;
mod guide;
mod inspect;
mod merge;
mod mmap;
mod parser;
mod platform;
mod progress;
mod query;
mod reader;
mod repair;
mod temp;
mod varint;

pub use build::{TypedRecord, append, append_typed, build, build_typed};
pub use format::VERSION as FORMAT_VERSION;
pub use guide::guide;
pub use inspect::{bench, info, info_json, info_json_into, norm, norm_into, validate};
pub use merge::{merge, sortcount};
pub use query::{
    dump, dump_frames, match_url, match_url_bytes, match_url_frames, match_url_frames_bytes,
    match_user, match_user_bytes, query, query_bytes,
};
pub use repair::{RepairOutcome, repair};
