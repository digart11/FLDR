use atlas_model::clamp::ClampLenient;

/// User widths of the Details columns. The renderer may shrink these transiently
/// in a narrow pane, but never writes that responsive projection back to prefs.
/// Serialized into ProfilePrefs via `DetailsColumns: ProfileScoped`.
#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
pub(crate) struct ColWidths {
    /// Name column width (icon + text), in real pixels.
    #[serde(default = "default_name_width")]
    pub(crate) name: f32,
    pub(crate) size: f32,
    pub(crate) modified: f32,
    pub(crate) type_w: f32,
    #[serde(default = "default_recursive_width")]
    pub(crate) folders: f32,
    #[serde(default = "default_recursive_width")]
    pub(crate) files: f32,
    #[serde(default)]
    pub(crate) hidden: [bool; 5],
    /// Pane width at which these proportions were edited. Absent in legacy prefs.
    #[serde(default)]
    pub(crate) reference_width: Option<f32>,
}

impl ColWidths {
    pub(crate) fn projected(self, available: f32) -> Self {
        let factor = self
            .reference_width
            .filter(|width| width.is_finite() && *width > 0.0)
            .map_or(1.0, |width| available.max(0.0) / width);
        Self {
            name: self.name * factor,
            size: self.size * factor,
            modified: self.modified * factor,
            type_w: self.type_w * factor,
            folders: self.folders * factor,
            files: self.files * factor,
            reference_width: None,
            hidden: self.hidden,
        }
    }

    pub(crate) fn resize_split(
        &mut self, column: &str, pixels: f32, maximum: f32, zoom: f32, size_minimum: f32,
    ) {
        let (width, minimum) = match column {
            "size" => (&mut self.size, size_minimum),
            "modified" => (&mut self.modified, 92.0 * zoom),
            "type" => (&mut self.type_w, 56.0 * zoom),
            "folders" => (&mut self.folders, 46.0 * zoom),
            "files" => (&mut self.files, 46.0 * zoom),
            _ => return,
        };
        *width = pixels.clamp_lenient(minimum.min(maximum), maximum);
    }

    pub(crate) fn resize(&mut self, column: &str, pixels: f32, available: f32) {
        if !available.is_finite() || available <= 0.0 {
            return;
        }
        let index = match column {
            "name" => 0,
            "size" => 1,
            "modified" => 2,
            "type" => 3,
            "folders" => 4,
            "files" => 5,
            _ => return,
        };
        let projected = self.projected(available);
        let mut values = [
            projected.name,
            projected.size,
            projected.modified,
            projected.type_w,
            projected.folders,
            projected.files,
        ];
        let original = values;
        for (i, hidden) in self.hidden.iter().enumerate() {
            if *hidden {
                values[i + 1] = 0.0;
            }
        }
        let total: f32 = values.iter().sum();
        if total > 0.0 {
            for value in &mut values {
                *value *= available / total;
            }
        }
        if values[index] <= 0.0 {
            return;
        }
        let Some(neighbor) = ((index + 1)..6)
            .find(|i| values[*i] > 0.0)
            .or_else(|| (0..index).rev().find(|i| values[*i] > 0.0))
        else {
            return;
        };
        let combined = values[index] + values[neighbor];
        let minima: [f32; 6] = [120.0, 46.0, 92.0, 56.0, 46.0, 46.0];
        let minimum = minima[index].min(combined * 0.5);
        let maximum = (combined - minima[neighbor]).max(minimum);
        values[index] = pixels.clamp_lenient(minimum, maximum);
        values[neighbor] = combined - values[index];
        for (i, hidden) in self.hidden.iter().enumerate() {
            if *hidden {
                values[i + 1] = original[i + 1];
            }
        }
        self.name = values[0];
        self.size = values[1];
        self.modified = values[2];
        self.type_w = values[3];
        self.folders = values[4];
        self.files = values[5];
        self.reference_width = Some(available);
    }
}

fn default_recursive_width() -> f32 { 70. }

fn default_name_width() -> f32 {
    300.
}

impl Default for ColWidths {
    fn default() -> Self {
        Self {
            name: default_name_width(),
            size: 90.,
            modified: 150.,
            type_w: 110.,
            hidden: [false; 5],
            folders: default_recursive_width(),
            files: default_recursive_width(),
            reference_width: Some(790.0),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum DetailsColumn {
    Size,
    Modified,
    Type,
    Folders,
    Files,
}

impl DetailsColumn {
    pub(crate) const DEFAULT_ORDER: [Self; 5] = [
        Self::Size,
        Self::Modified,
        Self::Type,
        Self::Folders,
        Self::Files,
    ];

    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Size => "size",
            Self::Modified => "modified",
            Self::Type => "type",
            Self::Folders => "folders",
            Self::Files => "files",
        }
    }
}

/// Per-profile Details column preferences: widths + visual order. Persisted as a
/// single `ProfileScoped` blob so widths and order travel together.
#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
pub(crate) struct DetailsColumnPrefs {
    pub(crate) widths: ColWidths,
    #[serde(deserialize_with = "read_order")]
    pub(crate) order: [DetailsColumn; 5],
}

impl Default for DetailsColumnPrefs {
    fn default() -> Self {
        Self {
            widths: ColWidths::default(),
            order: DetailsColumn::DEFAULT_ORDER,
        }
    }
}

fn read_order<'de, D: serde::Deserializer<'de>>(d: D) -> Result<[DetailsColumn; 5], D::Error> {
    use serde::Deserialize;
    let order = Vec::<DetailsColumn>::deserialize(d)?;
    expand_order(&order).ok_or_else(|| serde::de::Error::custom("invalid Details column order"))
}
fn expand_order(order: &[DetailsColumn]) -> Option<[DetailsColumn; 5]> {
    if order.len() != 3 && order.len() != 5 {
        return None;
    }
    let mut all = order.to_vec();
    if all.iter().enumerate().any(|(i, c)| all[..i].contains(c)) {
        return None;
    }
    for c in DetailsColumn::DEFAULT_ORDER {
        if !all.contains(&c) {
            all.push(c);
        }
    }
    all.try_into().ok()
}
impl ColWidths {
    pub(crate) fn value(self, column: DetailsColumn) -> f32 {
        if self.hidden[column as usize] {
            return 0.0;
        }
        match column {
            DetailsColumn::Size => self.size,
            DetailsColumn::Modified => self.modified,
            DetailsColumn::Type => self.type_w,
            DetailsColumn::Folders => self.folders,
            DetailsColumn::Files => self.files,
        }
    }
    pub(crate) fn set_visible(&mut self, column: DetailsColumn, visible: bool) {
        self.hidden[column as usize] = !visible;
        let default = Self::default().value(column);
        let width = match column {
            DetailsColumn::Size => &mut self.size,
            DetailsColumn::Modified => &mut self.modified,
            DetailsColumn::Type => &mut self.type_w,
            DetailsColumn::Folders => &mut self.folders,
            DetailsColumn::Files => &mut self.files,
        };
        if visible && *width <= 0.0 {
            *width = default;
        }
    }
}
impl DetailsColumnPrefs {
    pub(crate) fn from_folder(columns: &atlas_model::profiles::FolderColumns) -> Option<Self> {
        if columns.ratios.len() != 4 && columns.ratios.len() != 6 {
            return None;
        }
        if columns.ratios.iter().any(|v| !v.is_finite() || *v < 0.0) {
            return None;
        }
        let sum: f32 = columns.ratios.iter().sum();
        if !sum.is_finite() || sum <= f32::EPSILON {
            return None;
        }
        let order: Option<Vec<_>> = columns
            .order
            .iter()
            .map(|s| {
                DetailsColumn::DEFAULT_ORDER
                    .into_iter()
                    .find(|c| c.key() == s)
            })
            .collect();
        let order = expand_order(&order?)?;
        let reference = columns
            .reference_width
            .filter(|v| v.is_finite() && *v > 0.0);
        let basis = reference.unwrap_or(790.0);
        let scale = if reference.is_some() {
            basis
        } else {
            basis / sum
        };
        let mut values = [0.0; 6];
        if columns.ratios.len() == 4 {
            values[4] = default_recursive_width();
            values[5] = default_recursive_width();
        }
        for (out, ratio) in values.iter_mut().zip(&columns.ratios) {
            *out = ratio * scale;
        }
        if values.iter().any(|v| !v.is_finite()) {
            return None;
        }
        Some(Self {
            widths: ColWidths {
                name: values[0],
                size: values[1],
                modified: values[2],
                type_w: values[3],
                folders: values[4],
                files: values[5],
                reference_width: Some(basis),
                hidden: DetailsColumn::DEFAULT_ORDER
                    .map(|c| columns.hidden.iter().any(|key| key == c.key())),
            },
            order,
        })
    }
    pub(crate) fn to_folder(self) -> atlas_model::profiles::FolderColumns {
        let values = [
            self.widths.name,
            self.widths.size,
            self.widths.modified,
            self.widths.type_w,
            self.widths.folders,
            self.widths.files,
        ];
        let sum = values.iter().sum::<f32>().max(1.0);
        let reference = self
            .widths
            .reference_width
            .filter(|v| v.is_finite() && *v > 0.0)
            .unwrap_or(sum);
        atlas_model::profiles::FolderColumns {
            ratios: values.map(|v| v / reference).to_vec(),
            order: self.order.map(|c| c.key().to_string()).to_vec(),
            reference_width: Some(reference),
            hidden: DetailsColumn::DEFAULT_ORDER
                .into_iter()
                .filter(|c| self.widths.hidden[*c as usize])
                .map(|c| c.key().to_string())
                .collect(),
        }
    }
}

/// Shared header/row projection; hidden columns consume neither width nor minima.
pub(crate) fn responsive_widths(
    available: f32,
    zoom: f32,
    preferred: ColWidths,
    order: &[DetailsColumn],
    split_size_minimum: Option<f32>,
) -> [f32; 6] {
    use crate::ui::dg::table::{fit_columns, ColumnSpec};
    let split = split_size_minimum.is_some();
    let preferred = if split { preferred } else { preferred.projected(available) };
    let name_minimum = if split { crate::ui::views::details::ROW_NAME_MIN_W } else { 120.0 } * zoom;
    let secondary_minimum = |column, minimum: f32| {
        if split { minimum.min(preferred.value(column)) } else { minimum }
    };
    let mut widths = fit_columns(
        available,
        [
            ColumnSpec {
                preferred: if split { name_minimum } else { preferred.name },
                minimum: name_minimum,
                priority: 3,
            },
            ColumnSpec {
                preferred: if order.contains(&DetailsColumn::Size) {
                    preferred.value(DetailsColumn::Size)
                } else {
                    0.0
                },
                minimum: if preferred.value(DetailsColumn::Size) > 0.0
                    && order.contains(&DetailsColumn::Size)
                {
                    split_size_minimum.unwrap_or(46.0 * zoom)
                } else {
                    0.0
                },
                priority: 2,
            },
            ColumnSpec {
                preferred: if order.contains(&DetailsColumn::Modified) {
                    preferred.value(DetailsColumn::Modified)
                } else {
                    0.0
                },
                minimum: if preferred.value(DetailsColumn::Modified) > 0.0
                    && order.contains(&DetailsColumn::Modified)
                {
                    secondary_minimum(DetailsColumn::Modified, 92.0 * zoom)
                } else {
                    0.0
                },
                priority: 0,
            },
            ColumnSpec {
                preferred: if order.contains(&DetailsColumn::Type) {
                    preferred.value(DetailsColumn::Type)
                } else {
                    0.0
                },
                minimum: if preferred.value(DetailsColumn::Type) > 0.0
                    && order.contains(&DetailsColumn::Type)
                {
                    secondary_minimum(DetailsColumn::Type, 56.0 * zoom)
                } else {
                    0.0
                },
                priority: 1,
            },
            ColumnSpec {
                preferred: if order.contains(&DetailsColumn::Folders) {
                    preferred.value(DetailsColumn::Folders)
                } else {
                    0.0
                },
                minimum: if preferred.value(DetailsColumn::Folders) > 0.0
                    && order.contains(&DetailsColumn::Folders)
                {
                    secondary_minimum(DetailsColumn::Folders, 46.0 * zoom)
                } else {
                    0.0
                },
                priority: 0,
            },
            ColumnSpec {
                preferred: if order.contains(&DetailsColumn::Files) {
                    preferred.value(DetailsColumn::Files)
                } else {
                    0.0
                },
                minimum: if preferred.value(DetailsColumn::Files) > 0.0
                    && order.contains(&DetailsColumn::Files)
                {
                    secondary_minimum(DetailsColumn::Files, 46.0 * zoom)
                } else {
                    0.0
                },
                priority: 0,
            },
        ],
        if split { 0 } else { 6 },
    );
    if let Some(minimum) = split_size_minimum {
        // Below the readable minimum, use the partial slot for Name.
        if widths[1] < minimum - 0.01 {
            widths[0] += widths[1];
            widths[1] = 0.0;
        }
    }
    widths
}
