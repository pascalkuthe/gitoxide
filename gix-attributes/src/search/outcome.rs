use crate::search::{
    Assignments, AttributeId, Attributes, Match, MatchKind, MatchLocation, Metadata, MetadataCollection, Outcome,
    TrackedAssignment, Value,
};
use crate::{Assignment, NameRef, State};
use bstr::{BString, ByteSlice};
use gix_glob::Pattern;
use kstring::{KString, KStringRef};
use std::borrow::Cow;
use std::path::Path;

/// Initialization
impl<'pattern> Outcome<'pattern> {
    /// Initialize this instance to collect outcomes for all names in `collection`, which represents all possible attributes
    /// or macros we may visit.
    ///
    /// This must be called after each time `collection` changes.
    pub fn initialize(&mut self, collection: &MetadataCollection) {
        if self.matches_by_id.len() != collection.name_to_meta.len() {
            let global_num_attrs = collection.name_to_meta.len();

            self.matches_by_id.resize(global_num_attrs, Default::default());

            // NOTE: This works only under the assumption that macros remain defined.
            for (order, macro_attributes) in collection.iter().filter_map(|(_, meta)| {
                (!meta.macro_attributes.is_empty()).then_some((meta.id.0, &meta.macro_attributes))
            }) {
                self.matches_by_id[order].macro_attributes = macro_attributes.clone()
            }
        }
        self.reset();
    }

    /// Like [`initialize()`][Self::initialize()], but limits the set of attributes to look for and fill in
    /// to `attribute_names`.
    /// Users of this instance should prefer to limit their search as this would allow it to finish earlier.
    ///
    /// Note that `attribute_names` aren't validated to be valid names here, as invalid names definitely will always be unspecified.
    pub fn initialize_with_selection<'a>(
        &mut self,
        collection: &MetadataCollection,
        attribute_names: impl IntoIterator<Item = impl Into<KStringRef<'a>>>,
    ) {
        self.initialize(collection);

        self.selected.clear();
        self.selected.extend(attribute_names.into_iter().map(|name| {
            let name = name.into();
            (
                name.to_owned(),
                collection.name_to_meta.get(name.as_str()).map(|meta| meta.id),
            )
        }));
        self.reset_remaining();
    }

    /// Prepare for a new search over the known set of attributes by resetting our state.
    pub fn reset(&mut self) {
        self.matches_by_id.iter_mut().for_each(|item| item.r#match = None);
        self.attrs_stack.clear();
        self.reset_remaining();
    }

    fn reset_remaining(&mut self) {
        self.remaining = Some(if self.selected.is_empty() {
            self.matches_by_id.len()
        } else {
            self.selected.iter().filter(|(_name, id)| id.is_some()).count()
        });
    }
}

/// Access
impl<'pattern> Outcome<'pattern> {
    /// Return an iterator over all filled attributes we were initialized with.
    ///
    /// ### Note
    ///
    /// If [`initialize_with_selection`][Self::initialize_with_selection()] was used,
    /// use [`iter_selected()`][Self::iter_selected()] instead.
    ///
    /// ### Deviation
    ///
    /// It's possible that the order in which the attribute are returned (if not limited to a set of attributes) isn't exactly
    /// the same as what `git` provides.
    /// Ours is in order of declaration, whereas `git` seems to list macros first somehow. Since the values are the same, this
    /// shouldn't be an issue.
    pub fn iter<'a>(&'a self) -> impl Iterator<Item = &'a Match<'pattern>> + 'a {
        self.matches_by_id.iter().filter_map(|item| item.r#match.as_ref())
    }

    /// Iterate over all matches of the attribute selection in their original order.
    pub fn iter_selected<'a>(&'a self) -> impl Iterator<Item = Cow<'a, Match<'pattern>>> + 'a {
        static DUMMY: Pattern = Pattern {
            text: BString::new(Vec::new()),
            mode: gix_glob::pattern::Mode::empty(),
            first_wildcard_pos: None,
        };
        self.selected.iter().map(|(name, id)| {
            id.and_then(|id| self.matches_by_id[id.0].r#match.as_ref())
                .map(Cow::Borrowed)
                .unwrap_or_else(|| {
                    Cow::Owned(Match {
                        pattern: &DUMMY,
                        assignment: Assignment {
                            name: NameRef::try_from(name.as_bytes().as_bstr())
                                .unwrap_or_else(|_| NameRef("invalid".into()))
                                .to_owned(),
                            state: State::Unspecified,
                        },
                        kind: MatchKind::Attribute { macro_id: None },
                        location: MatchLocation {
                            source: None,
                            sequence_number: 0,
                        },
                    })
                })
        })
    }

    /// Obtain a match by the order of its attribute, if the order exists in our initialized attribute list and there was a match.
    pub fn match_by_id(&self, id: AttributeId) -> Option<&Match<'pattern>> {
        self.matches_by_id.get(id.0).and_then(|m| m.r#match.as_ref())
    }
}

/// Mutation
impl<'pattern> Outcome<'pattern> {
    /// Fill all `attrs` and resolve them recursively if they are macros. Return `true` if there is no attribute left to be resolved and
    /// we are totally done.
    /// `pattern` is what matched a patch and is passed for contextual information,
    /// providing `sequence_number` and `source` as well.
    pub(crate) fn fill_attributes<'a>(
        &mut self,
        attrs: impl Iterator<Item = &'a TrackedAssignment>,
        pattern: &'pattern gix_glob::Pattern,
        source: Option<&'pattern Path>,
        sequence_number: usize,
    ) -> bool {
        self.attrs_stack.extend(attrs.filter_map(|attr| {
            self.matches_by_id[attr.id.0]
                .r#match
                .is_none()
                .then(|| (attr.id, attr.inner.clone(), None))
        }));
        while let Some((id, assignment, parent_order)) = self.attrs_stack.pop() {
            let slot = &mut self.matches_by_id[id.0];
            if slot.r#match.is_some() {
                continue;
            }
            // Let's be explicit - this is only non-empty for macros.
            let is_macro = !slot.macro_attributes.is_empty();

            slot.r#match = Some(Match {
                pattern,
                assignment: assignment.to_owned(),
                kind: if is_macro {
                    MatchKind::Macro {
                        parent_macro_id: parent_order,
                    }
                } else {
                    MatchKind::Attribute { macro_id: parent_order }
                },
                location: MatchLocation {
                    source,
                    sequence_number,
                },
            });
            if self.reduce_and_check_if_done(id) {
                return true;
            }

            if is_macro {
                // TODO(borrowchk): one fine day we should be able to re-borrow `slot` without having to redo the array access.
                let slot = &self.matches_by_id[id.0];
                self.attrs_stack.extend(slot.macro_attributes.iter().filter_map(|attr| {
                    self.matches_by_id[attr.id.0]
                        .r#match
                        .is_none()
                        .then(|| (attr.id, attr.inner.clone(), Some(id)))
                }));
            }
        }
        false
    }
}

impl<'attr> Outcome<'attr> {
    /// Given a list of `attrs` by order, return true if at least one of them is not set
    pub(crate) fn has_unspecified_attributes(&self, mut attrs: impl Iterator<Item = AttributeId>) -> bool {
        attrs.any(|order| self.matches_by_id[order.0].r#match.is_none())
    }
    /// Return the amount of attributes haven't yet been found.
    ///
    /// If this number reaches 0, then the search can be stopped as there is nothing more to fill in.
    pub(crate) fn remaining(&self) -> usize {
        self.remaining
            .expect("BUG: instance must be initialized for each search set")
    }

    /// Return true if there is nothing more to be done as all attributes were filled.
    pub(crate) fn is_done(&self) -> bool {
        self.remaining() == 0
    }

    fn reduce_and_check_if_done(&mut self, attr: AttributeId) -> bool {
        if self.selected.is_empty()
            || self
                .selected
                .iter()
                .any(|(_name, id)| id.map_or(false, |id| id == attr))
        {
            *self.remaining.as_mut().expect("initialized") -= 1;
        }
        self.is_done()
    }
}

/// Mutation
impl MetadataCollection {
    /// Assign order ids to each attribute either in macros (along with macros themselves) or attributes of patterns, and store
    /// them in this collection.
    ///
    /// Must be called before querying matches.
    pub fn update_from_list(&mut self, list: &mut gix_glob::search::pattern::List<Attributes>) {
        for pattern in &mut list.patterns {
            match &mut pattern.value {
                Value::MacroAssignments { id: order, assignments } => {
                    *order = self.id_for_macro(
                        pattern
                            .pattern
                            .text
                            .to_str()
                            .expect("valid macro names are always UTF8 and this was verified"),
                        assignments,
                    );
                }
                Value::Assignments(assignments) => {
                    self.assign_order_to_attributes(assignments);
                }
            }
        }
    }
}

/// Access
impl MetadataCollection {
    /// Return an iterator over the contents of the map in an easy-to-consume form.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Metadata)> {
        self.name_to_meta.iter().map(|(k, v)| (k.as_str(), v))
    }
}

impl MetadataCollection {
    pub(crate) fn id_for_macro(&mut self, name: &str, attrs: &mut Assignments) -> AttributeId {
        let order = match self.name_to_meta.get_mut(name) {
            Some(meta) => meta.id,
            None => {
                let order = AttributeId(self.name_to_meta.len());
                self.name_to_meta.insert(
                    KString::from_ref(name),
                    Metadata {
                        id: order,
                        macro_attributes: Default::default(),
                    },
                );
                order
            }
        };

        self.assign_order_to_attributes(attrs);
        self.name_to_meta.get_mut(name).expect("just added").macro_attributes = attrs.clone();

        order
    }
    pub(crate) fn id_for_attribute(&mut self, name: &str) -> AttributeId {
        match self.name_to_meta.get(name) {
            Some(meta) => meta.id,
            None => {
                let order = AttributeId(self.name_to_meta.len());
                self.name_to_meta.insert(KString::from_ref(name), order.into());
                order
            }
        }
    }
    pub(crate) fn assign_order_to_attributes(&mut self, attributes: &mut [TrackedAssignment]) {
        for TrackedAssignment {
            id: order,
            inner: crate::Assignment { name, .. },
        } in attributes
        {
            *order = self.id_for_attribute(&name.0);
        }
    }
}

impl From<AttributeId> for Metadata {
    fn from(order: AttributeId) -> Self {
        Metadata {
            id: order,
            macro_attributes: Default::default(),
        }
    }
}

impl MatchKind {
    /// return the id of the macro that resolved us, or `None` if that didn't happen.
    pub fn source_id(&self) -> Option<AttributeId> {
        match self {
            MatchKind::Attribute { macro_id: id } | MatchKind::Macro { parent_macro_id: id } => *id,
        }
    }
}