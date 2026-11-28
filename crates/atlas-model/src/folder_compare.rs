use std::{collections::HashMap, path::PathBuf};

#[derive(Clone, Copy)]
pub enum Mark {
    LeftOnly,
    RightOnly,
    MetadataMatch,
    Different,
    IdenticalContent,
    Unverified,
}

#[derive(Default)]
pub struct Comparison {
    pub left: HashMap<PathBuf, Mark>,
    pub right: HashMap<PathBuf, Mark>,
}
