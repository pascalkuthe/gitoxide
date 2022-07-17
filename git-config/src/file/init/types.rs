use crate::file::init;
use crate::parse;
use crate::path::interpolate;

/// The error returned by [`File::from_paths_metadata()`][crate::File::from_paths_metadata()] and
/// [`File::from_env_paths()`][crate::File::from_env_paths()].
#[derive(Debug, thiserror::Error)]
#[allow(missing_docs)]
pub enum Error {
    #[error(transparent)]
    Parse(#[from] parse::Error),
    #[error(transparent)]
    Interpolate(#[from] interpolate::Error),
    #[error(transparent)]
    Includes(#[from] init::includes::Error),
}

/// Options when loading git config using [`File::from_paths_metadata()`].
#[derive(Clone, Copy, Default)]
pub struct Options<'a> {
    /// Configure how to follow includes while handling paths.
    pub includes: init::includes::Options<'a>,
    /// If true, only value-bearing parse events will be kept to reduce memory usage and increase performance.
    ///
    /// Note that doing so will prevent [`write_to()`][File::write_to()] to serialize itself meaningfully and correctly,
    /// as newlines will be missing. Use this only if it's clear that serialization will not be attempted.
    pub lossy: bool,
}
