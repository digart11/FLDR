use super::App;
use atlas_model::edition::{self, Feature};

impl App {
    /// Shared policy entry point; FileOP's baseline is always Full.
    pub(crate) fn allow_edition_action(&mut self, feature: Feature, count: usize) -> bool {
        edition::current().check(feature, count).is_ok()
    }

    pub(crate) fn allow_conversion_action(&mut self, count: usize) -> bool {
        self.allow_edition_action(Feature::BatchConversion, count)
    }
}
