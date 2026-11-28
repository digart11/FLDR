//! Secondary Glass ink retains theme hue and full coverage on variable backings.
//! Wallpaper pixels are unavailable here; this improves contrast, not a universal
//! contrast guarantee at every transparency setting.
pub(crate) fn secondary(muted: [f32; 3], foreground: [f32; 3]) -> [f32; 3] {
    // Dark ink over a thin light canvas needs a smaller separation than light
    // ink over a dark canvas. Keep a tonal distinction in both cases.
    let dark_ink = foreground.iter().sum::<f32>() < muted.iter().sum::<f32>();
    let primary_weight = if dark_ink { 0.95 } else { 0.85 };
    std::array::from_fn(|i| muted[i] * (1.0-primary_weight) + foreground[i] * primary_weight)
}
