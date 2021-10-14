use git_repository as git;
use git_repository::{bstr::ByteSlice, url::Scheme, Url};

use crate::{
    changelog,
    changelog::{
        section,
        section::{segment, segment::details::Category, Segment},
        Section,
    },
    ChangeLog,
};

impl std::fmt::Display for changelog::Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            changelog::Version::Unreleased => f.write_str("Unreleased"),
            changelog::Version::Semantic(v) => write!(f, "v{}", v),
        }
    }
}

/// Define how linkable items should be written
#[derive(Clone)]
pub enum Linkables {
    /// Use markdown links to link directly to the linkable items
    AsLinks {
        /// The location of the repository to link to
        repository_url: RepositoryUrl,
    },
    /// Leave them in a textual representation for the hosting platform to auto-link them
    AsText,
}

#[derive(Clone)]
pub struct RepositoryUrl {
    pub inner: git::Url,
}

impl From<git::Url> for RepositoryUrl {
    fn from(v: Url) -> Self {
        RepositoryUrl { inner: v }
    }
}

impl RepositoryUrl {
    pub fn is_github(&self) -> bool {
        self.inner.host.as_ref().map(|h| h == "github.com").unwrap_or(false)
    }

    pub fn github_https(&self) -> Option<String> {
        match &self.inner.host {
            Some(host) if host == "github.com" => match self.inner.scheme {
                Scheme::Http | Scheme::Https | Scheme::Git => {
                    format!("https://github.com/{}", self.inner.path.to_str_lossy()).into()
                }
                Scheme::Ssh => self
                    .inner
                    .user
                    .as_ref()
                    .map(|user| format!("https://github.com/{}/{}", user, self.inner.path.to_str_lossy())),
                Scheme::Radicle | Scheme::File => None,
            },
            None | Some(_) => None,
        }
    }
}

bitflags::bitflags! {
    pub struct Components: u8 {
        const SECTION_TITLE = 1<<0;
        const HTML_TAGS = 1<<1;
        const DETAIL_TAGS = 1<<2;
    }
}

impl Section {
    pub const UNKNOWN_TAG_START: &'static str = "<csr-unknown>";
    pub const UNKNOWN_TAG_END: &'static str = "<csr-unknown/>";
    pub const READONLY_TAG: &'static str = "<csr-read-only-do-not-edit/>\n"; // needs a newline to not interfere with formatting

    /// Note that `headline` should be enabled by default as it will break parsing to some extend. It's a special case for tag
    /// objects.
    pub fn write_to(
        &self,
        mut out: impl std::fmt::Write,
        link_mode: &Linkables,
        components: Components,
    ) -> std::fmt::Result {
        match self {
            Section::Verbatim { text, .. } => out.write_str(text),
            Section::Release {
                name,
                date,
                heading_level,
                segments,
                removed_messages,
                unknown,
            } => {
                if components.contains(Components::SECTION_TITLE) {
                    write!(out, "{} {}", heading(*heading_level), name)?;
                    match date {
                        None => out.write_str("\n\n"),
                        Some(date) => writeln!(
                            out,
                            " ({:04}-{:02}-{:02})\n",
                            date.year(),
                            date.month() as u32,
                            date.day()
                        ),
                    }?;
                }
                if !removed_messages.is_empty() && components.contains(Components::HTML_TAGS) {
                    for id in removed_messages {
                        writeln!(out, "{}{}/>", segment::Conventional::REMOVED_HTML_PREFIX, id)?;
                    }
                    writeln!(out)?;
                }

                let section_level = *heading_level + 1;
                for segment in segments {
                    segment.write_to(section_level, link_mode, components, &mut out)?;
                }
                if !unknown.is_empty() && components.contains(Components::HTML_TAGS) {
                    writeln!(out, "{}", Section::UNKNOWN_TAG_START)?;
                    out.write_str(unknown)?;
                    writeln!(out, "{}", Section::UNKNOWN_TAG_END)?;
                }
                Ok(())
            }
        }
    }
}

fn heading(level: usize) -> String {
    "#".repeat(level)
}

impl ChangeLog {
    pub fn write_to(&self, mut out: impl std::fmt::Write, link_mode: &Linkables) -> std::fmt::Result {
        for section in &self.sections {
            section.write_to(&mut out, link_mode, Components::all())?;
        }
        Ok(())
    }
}

impl section::Segment {
    pub fn write_to(
        &self,
        section_level: usize,
        link_mode: &Linkables,
        components: Components,
        mut out: impl std::fmt::Write,
    ) -> std::fmt::Result {
        let write_html = components.contains(Components::HTML_TAGS);
        match self {
            Segment::User { markdown } => out.write_str(markdown)?,
            Segment::Conventional(segment::Conventional {
                kind,
                is_breaking,
                removed,
                messages,
            }) => match segment::conventional::as_headline(kind).or_else(|| is_breaking.then(|| *kind)) {
                Some(headline) => {
                    writeln!(
                        out,
                        "{} {}{}\n",
                        heading(section_level),
                        headline,
                        if *is_breaking {
                            format!(" {}", segment::Conventional::BREAKING_TITLE_ENCLOSED)
                        } else {
                            "".into()
                        },
                    )?;

                    if !removed.is_empty() && write_html {
                        for id in removed {
                            writeln!(out, "{}{}/>", segment::Conventional::REMOVED_HTML_PREFIX, id)?;
                        }
                        writeln!(out)?;
                    }

                    use segment::conventional::Message;
                    for message in messages {
                        match message {
                            Message::Generated { title, id, body } => {
                                if write_html {
                                    writeln!(
                                        out,
                                        " - {}{}/> {}",
                                        segment::Conventional::REMOVED_HTML_PREFIX,
                                        id,
                                        title
                                    )?;
                                } else {
                                    writeln!(out, " - {}", title)?;
                                }
                                if let Some(body) = body {
                                    for line in body.as_bytes().as_bstr().lines_with_terminator() {
                                        write!(out, "   {}", line.to_str().expect("cannot fail as original is UTF-8"))?;
                                    }
                                    if !body.ends_with('\n') {
                                        writeln!(out)?;
                                    }
                                }
                            }
                            Message::User { markdown } => {
                                out.write_str(markdown)?;
                                if !markdown.ends_with('\n') {
                                    writeln!(out)?;
                                }
                            }
                        }
                    }
                    writeln!(out)?;
                }
                None => log::trace!(
                    "Skipping unknown git-conventional kind {:?} and all {} message(s) in it.",
                    kind,
                    messages.len()
                ),
            },
            Segment::Details(section::Data::Generated(segment::Details { commits_by_category }))
                if !commits_by_category.is_empty() =>
            {
                let write_details_tags = components.contains(Components::DETAIL_TAGS);
                writeln!(out, "{} {}\n", heading(section_level), segment::Details::TITLE)?;
                if write_details_tags {
                    writeln!(out, "{}", Section::READONLY_TAG)?;
                    writeln!(out, "{}\n", segment::Details::HTML_PREFIX)?;
                }
                for (category, messages) in commits_by_category.iter() {
                    writeln!(out, " * **{}**", format_category(category, link_mode))?;
                    for message in messages {
                        writeln!(out, "    - {} ({})", message.title, format_oid(&message.id, link_mode))?;
                    }
                }
                if write_details_tags {
                    writeln!(out, "{}\n", segment::Details::HTML_PREFIX_END)?;
                }
            }
            Segment::Statistics(section::Data::Generated(segment::CommitStatistics {
                count,
                duration,
                conventional_count,
                unique_issues,
            })) => {
                writeln!(out, "{} {}\n", heading(section_level), segment::CommitStatistics::TITLE)?;
                if write_html {
                    writeln!(out, "{}", Section::READONLY_TAG)?;
                }
                writeln!(
                    out,
                    " - {} {} contributed to the release{}",
                    count,
                    if *count == 1 { "commit" } else { "commits" },
                    match duration {
                        Some(duration) if duration.whole_days() > 0 => format!(
                            " over the course of {} calendar {}.",
                            duration.whole_days(),
                            if duration.whole_days() == 1 { "day" } else { "days" }
                        ),
                        _ => ".".into(),
                    }
                )?;
                writeln!(
                    out,
                    " - {} {} where understood as [conventional](https://www.conventionalcommits.org).",
                    conventional_count,
                    if *conventional_count == 1 { "commit" } else { "commits" }
                )?;
                if unique_issues.is_empty() {
                    writeln!(out, " - 0 issues like '(#ID)' where seen in commit messages")?;
                } else {
                    writeln!(
                        out,
                        " - {} unique {} {} worked on: {}",
                        unique_issues.len(),
                        if unique_issues.len() == 1 { "issue" } else { "issues" },
                        if unique_issues.len() == 1 { "was" } else { "were" },
                        unique_issues
                            .iter()
                            .map(|c| format_category(c, link_mode))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )?;
                }
                writeln!(out)?;
            }
            Segment::Clippy(section::Data::Generated(segment::ThanksClippy { count })) if *count > 0 => {
                writeln!(out, "{} {}\n", heading(section_level), segment::ThanksClippy::TITLE)?;
                if write_html {
                    writeln!(out, "{}", Section::READONLY_TAG)?;
                }
                writeln!(
                    out,
                    "[Clippy](https://github.com/rust-lang/rust-clippy) helped {} {} to make code idiomatic. \n",
                    count,
                    if *count > 1 { "times" } else { "time" }
                )?;
            }
            Segment::Clippy(_) => {}
            Segment::Statistics(_) => {}
            Segment::Details(_) => {}
        };
        Ok(())
    }
}

fn format_category(cat: &Category, link_mode: &Linkables) -> String {
    match (cat, link_mode) {
        (Category::Issue(id), Linkables::AsLinks { repository_url }) => match repository_url.github_https() {
            Some(base_url) => {
                format!("[#{}]({}/issues/{})", id, base_url, id)
            }
            None => format_category(cat, &Linkables::AsText),
        },
        (_, _) => cat.to_string(),
    }
}

fn format_oid(id: &git::oid, link_mode: &Linkables) -> String {
    match link_mode {
        Linkables::AsText => id.to_hex(7).to_string(),
        Linkables::AsLinks { repository_url } => match repository_url.github_https() {
            Some(base_url) => {
                format!("[`{}`]({}/commit/{})", id.to_hex(7), base_url, id)
            }
            None => format_oid(id, &Linkables::AsText),
        },
    }
}