use std::collections::hash_map::{IntoIter, Iter};
use std::ops::{Deref, DerefMut};

use rustc_hash::FxHashMap;

use crate::templatetags::{EndTag, IntermediateTag, TagArg, TagSpec, TagSpecs};

#[derive(Clone, Debug, Default)]
pub struct TagShapes(FxHashMap<String, TagShape>);

impl Deref for TagShapes {
    type Target = FxHashMap<String, TagShape>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for TagShapes {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<'a> IntoIterator for &'a TagShapes {
    type Item = (&'a String, &'a TagShape);
    type IntoIter = Iter<'a, String, TagShape>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl IntoIterator for TagShapes {
    type Item = (String, TagShape);
    type IntoIter = IntoIter<String, TagShape>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl From<&TagSpecs> for TagShapes {
    fn from(specs: &TagSpecs) -> Self {
        TagShapes(
            specs
                .into_iter()
                .map(|(name, spec)| (name.clone(), TagShape::from(spec)))
                .collect(),
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagShape {
    form: TagForm,
    namespace: Option<String>,
}

impl TagShape {
    pub fn form(&self) -> &TagForm {
        &self.form
    }
}

impl From<&TagSpec> for TagShape {
    fn from(spec: &TagSpec) -> Self {
        TagShape {
            namespace: None,
            form: match &spec.end_tag {
                None => TagForm::Leaf,
                Some(end) => TagForm::Block {
                    end: end.into(),
                    intermediates: spec.intermediate_tags.iter().map(Into::into).collect(),
                },
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TagForm {
    Leaf,
    Block {
        end: TagEndShape,
        intermediates: Vec<IntermediateShape>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagEndShape {
    name: String,
    policy: EndPolicy,
}

impl TagEndShape {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn policy(&self) -> EndPolicy {
        self.policy
    }
}

impl From<&EndTag> for TagEndShape {
    fn from(end: &EndTag) -> Self {
        TagEndShape {
            name: end.name.as_ref().to_owned(),
            policy: if end.optional {
                EndPolicy::Optional
            } else {
                EndPolicy::Required
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntermediateShape {
    name: String,
    args: Vec<ArgShape>,
}

impl IntermediateShape {
    pub fn name(&self) -> &str {
        &self.name
    }
}

impl From<&IntermediateTag> for IntermediateShape {
    fn from(tag: &IntermediateTag) -> Self {
        IntermediateShape {
            name: tag.name.as_ref().to_owned(),
            args: tag.args.iter().map(Into::into).collect(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArgShape {
    name: String,
    required: bool,
}

impl From<&TagArg> for ArgShape {
    fn from(arg: &TagArg) -> Self {
        ArgShape {
            name: arg.name().as_ref().to_owned(),
            required: arg.is_required(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EndPolicy {
    Required,
    MustMatchOpenName,
    Optional,
}

#[derive(Clone, Debug)]
pub struct EndEntry {
    opener: String,
    policy: EndPolicy,
}

impl EndEntry {
    pub fn opener(&self) -> &str {
        &self.opener
    }

    pub fn policy(&self) -> EndPolicy {
        self.policy
    }

    pub fn matches_opener(&self, name: &str) -> bool {
        name == self.opener
    }
}

type EndIndex = FxHashMap<String, EndEntry>;

pub fn build_end_index(shapes: &TagShapes) -> EndIndex {
    let mut end_index = EndIndex::default();
    for (open_name, shape) in shapes {
        if let TagForm::Block { end, .. } = &shape.form {
            // Illegal states are impossible here; each Block must have an end tag
            end_index.insert(
                end.name.clone(),
                EndEntry {
                    opener: open_name.clone(),
                    policy: end.policy,
                },
            );
        }
    }
    end_index
}
