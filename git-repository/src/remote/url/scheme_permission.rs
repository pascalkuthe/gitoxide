#![allow(dead_code, unused_variables)]

use crate::bstr::{BStr, BString, ByteSlice};
use crate::permission;
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::convert::TryFrom;

///
pub mod init {
    use crate::bstr::BString;

    #[derive(Debug, thiserror::Error)]
    pub enum Error {
        #[error("{value:?} must be allow|deny|user in configuration key protocol{0}.allow", scheme.as_ref().map(|s| format!(".{}", s)).unwrap_or_default())]
        InvalidConfiguration { scheme: Option<String>, value: BString },
    }
}

#[derive(Debug, Clone, Copy)]
enum Allow {
    Always,
    Never,
    User,
}

impl Allow {
    pub fn to_bool(self, user_allowed: Option<bool>) -> bool {
        match self {
            Allow::Always => true,
            Allow::Never => false,
            Allow::User => user_allowed.unwrap_or(true),
        }
    }
}

impl<'a> TryFrom<Cow<'a, BStr>> for Allow {
    type Error = BString;

    fn try_from(v: Cow<'a, BStr>) -> Result<Self, Self::Error> {
        Ok(match v.as_ref().as_ref() {
            b"never" => Allow::Never,
            b"always" => Allow::Always,
            b"user" => Allow::User,
            unknown => return Err(unknown.into()),
        })
    }
}

#[derive(Debug, Clone)]
pub(crate) struct SchemePermission {
    /// `None`, env-var is unset or wasn't queried, otherwise true if `GIT_PROTOCOL_FROM_USER` is `1`.
    user_allowed: Option<bool>,
    /// The general allow value from `protocol.allow`.
    allow: Option<Allow>,
    /// Per scheme allow information
    allow_per_scheme: BTreeMap<git_url::Scheme, Allow>,
}

/// Init
impl SchemePermission {
    pub fn from_config(
        config: &git_config::File<'static>,
        git_prefix: &permission::env_var::Resource,
        mut filter: fn(&git_config::file::Metadata) -> bool,
    ) -> Result<Self, init::Error> {
        let allow: Option<Allow> = config
            .string_filter("protocol", None, "allow", &mut filter)
            .map(Allow::try_from)
            .transpose()
            .map_err(|invalid| init::Error::InvalidConfiguration {
                value: invalid,
                scheme: None,
            })?;
        let allow_per_scheme = match config.sections_by_name_and_filter("protocol", &mut filter) {
            Some(it) => {
                let mut map = BTreeMap::default();
                for (section, scheme) in it.filter_map(|section| {
                    section.header().subsection_name().and_then(|scheme| {
                        scheme
                            .to_str()
                            .ok()
                            .and_then(|scheme| git_url::Scheme::try_from(scheme).ok().map(|scheme| (section, scheme)))
                    })
                }) {
                    if let Some(value) = section
                        .value("allow")
                        .map(Allow::try_from)
                        .transpose()
                        .map_err(|invalid| init::Error::InvalidConfiguration {
                            scheme: Some(scheme.as_str().into()),
                            value: invalid,
                        })?
                    {
                        map.insert(scheme, value);
                    }
                }
                map
            }
            None => Default::default(),
        };
        Ok(SchemePermission {
            allow,
            allow_per_scheme,
            user_allowed: None,
        })
    }
}

/// Access
impl SchemePermission {
    pub fn allow(&self, scheme: &git_url::Scheme) -> bool {
        self.allow_per_scheme.get(&scheme).or(self.allow.as_ref()).map_or_else(
            || {
                use git_url::Scheme::*;
                match scheme {
                    File | Git | Ssh | Http | Https => true,
                    Ext(_) => false,
                    // TODO: figure out what 'ext' really entails, and what 'other' protocols are which aren't representable for us yet
                }
            },
            |allow| allow.to_bool(self.user_allowed),
        )
    }
}