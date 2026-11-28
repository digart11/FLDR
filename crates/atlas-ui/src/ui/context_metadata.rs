//! Compact, type-specific presentation of canonical Windows property values.
//! Localized display names remain available to the full inspector/tooltip.
pub(crate) fn display_property(name: &str, value: &str) -> Option<String> {
    // Windows formatting can insert direction marks around numeric values.
    let value: String = value.chars().filter(|c| !matches!(*c, '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')).collect();
    let value = value.trim();
    let identifier = value.trim_matches(|c| matches!(c, '{' | '}' | '(' | ')' | '"'));
    if value.is_empty() || matches!(value.to_ascii_lowercase().as_str(), "unknown" | "n/a" | "-" | "--")
        || (identifier.len() == 36 && identifier.chars().all(|c| c.is_ascii_hexdigit() || c == '-')) { return None; }
    if name == "System.Image.Dimensions" {
        let normalized=value.replace(" x ","×").replace(" × ","×");return Some(normalized);
    }
    match name {
        "System.Media.Duration" => {
            let parts: Vec<_> = value.split(':').map(str::parse::<u64>).collect::<Result<_, _>>().ok()?;
            match parts.as_slice() {
                [0, minutes, seconds] if *minutes < 60 && *seconds < 60 => Some(format!("{minutes:02}:{seconds:02}")),
                [hours, minutes, seconds] if *minutes < 60 && *seconds < 60 => Some(format!("{hours:02}:{minutes:02}:{seconds:02}")),
                [minutes, seconds] if *minutes < 60 && *seconds < 60 => Some(format!("{minutes:02}:{seconds:02}")),
                _ => None,
            }
        }
        "System.Video.FrameRate" => {
            let number = value.split_whitespace().next()?.replace(',', ".").parse::<f64>().ok()?;
            if !number.is_finite() || number <= 0. || number > 1000. { return None; }
            let number = format!("{number:.2}");
            Some(format!("{} fps", number.trim_end_matches('0').trim_end_matches('.')))
        }
        "System.Video.Compression" => match value.to_ascii_uppercase().as_str() {
            "H.264" | "H264" | "AVC" | "AVC1" => Some("H.264".into()),
            "H.265" | "H265" | "HEVC" => Some("HEVC".into()),
            "VP8" | "VP9" | "AV1" | "MPEG-2" | "MPEG-4" | "MJPEG" => Some(value.to_owned()),
            _ => None, // Unknown subtype IDs are not useful presentation.
        },
        "System.Audio.Format" => match value.to_ascii_uppercase().as_str() {
            "MP3" | "AAC" | "PCM" | "FLAC" | "WAVE" | "ALAC" | "OPUS" | "VORBIS" | "WMA" => Some(value.to_owned()),
            _ => None,
        },
        _ => Some(value.to_owned()),
    }
}
pub(crate) fn compact_fields(
    extension: &str,
    properties: &[(String, String, String)],
) -> Vec<String> {
    fields_at_level(extension, properties, atlas_model::settings::InformationLevel::Standard)
}
pub(crate) fn fields_at_level(extension: &str, properties: &[(String, String, String)], level: atlas_model::settings::InformationLevel) -> Vec<String> {
    use atlas_model::settings::InformationLevel;
    if level == InformationLevel::Off { return Vec::new(); }
    let get = |key: &str| {
        properties
            .iter()
            .find(|(name, _, value)| name == key && !value.trim().is_empty())
            .and_then(|(_, _, value)| display_property(key, value))
    };
    let extension = extension.to_ascii_lowercase();
    let raw = matches!(
        extension.as_str(),
        "arw"
            | "cr2"
            | "cr3"
            | "crw"
            | "dng"
            | "erf"
            | "nef"
            | "nrw"
            | "orf"
            | "pef"
            | "raf"
            | "raw"
            | "rw2"
            | "srw" | "srf" | "sr2" | "x3f"
    );
    let image = raw
        || matches!(
            extension.as_str(),
            "png"
                | "jpg" | "jpe"
                | "jpeg"
                | "gif"
                | "bmp"
                | "webp"
                | "tif"
                | "tiff"
                | "heic"
                | "heif"
                | "avif"
                | "jxl"
                | "ico" | "psd" | "psb"
        );
    let video = matches!(
        extension.as_str(),
        "mp4" | "m4v" | "mov" | "mkv" | "avi" | "wmv" | "webm" | "mpg" | "mpeg" | "mts" | "m2ts" | "ts"
    );
    let audio = matches!(
        extension.as_str(),
        "mp3" | "wav" | "flac" | "ogg" | "opus" | "m4a" | "aac" | "wma" | "aiff" | "alac"
    );
    let mut fields = Vec::new();
    let extended = level == InformationLevel::Extended;
    if image {
        fields.extend(get("System.Image.Dimensions").or_else(|| Some(format!("{}×{}", get("System.Image.HorizontalSize")?, get("System.Image.VerticalSize")?))));
        fields.extend(get("System.Photo.CameraModel"));
        fields.extend(get("System.Photo.LensModel"));
        if extended {
            if let Some(iso) = get("System.Photo.ISOSpeed") { fields.push(format!("ISO {}", iso.trim_start_matches("ISO-").trim_start_matches("ISO "))); }
            for key in ["System.Photo.FNumber", "System.Photo.ExposureTime", "System.Photo.FocalLength"] { fields.extend(get(key)); }
        }
    } else if video || audio {
        if video {
            if let (Some(width), Some(height)) = (get("System.Video.FrameWidth"), get("System.Video.FrameHeight")) { fields.push(format!("{width}×{height}")); }
        }
        fields.extend(get("System.Media.Duration"));
        if extended {
            if video { fields.extend(get("System.Video.FrameRate")); fields.extend(get("System.Video.Compression")); }
            fields.extend(get("System.Audio.SampleRate"));
            if let Some(channels) = get("System.Audio.ChannelCount") { fields.push(match channels.as_str() { "1" => "Mono".into(), "2" => "Stereo".into(), _ => channels }); }
            fields.extend(get("System.Audio.EncodingBitrate"));
        }
    } else if extended {
        fields.extend(get("System.ItemTypeText"));
        fields.extend(get("System.DateModified"));
        for key in ["System.Title", "System.Author", "System.Document.PageCount", "System.Document.WordCount"] { fields.extend(get(key)); }
    }
    fields
}

#[cfg(test)]
mod tests {
    use super::*;
    fn properties(items: &[(&str, &str)]) -> Vec<(String, String, String)> {
        items
            .iter()
            .map(|(key, value)| (key.to_string(), "Localized label".into(), value.to_string()))
            .collect()
    }
    #[test]
    fn image_raw_audio_and_video_use_semantic_values_without_dates() {
        let values = properties(&[
            ("System.Photo.DateTaken", "unwanted date"),
            ("System.Music.Artist", "artist"),
            ("System.Image.Dimensions", "6000×4000"),
            ("System.Photo.CameraModel", "Camera"),
            ("System.Photo.LensModel", "Lens"),
            ("System.Photo.ISOSpeed", "400"),
            ("System.Media.Duration", "00:03:00"),
            ("System.Audio.EncodingBitrate", "320 kbps"),
            ("System.Video.FrameWidth", "1920"),
            ("System.Video.FrameHeight", "1080"),
        ]);
        assert_eq!(compact_fields("JPG", &values), ["6000×4000", "Camera", "Lens"]);
        assert_eq!(
            compact_fields("nef", &values),
            ["6000×4000", "Camera", "Lens"]
        );
        assert_eq!(compact_fields("mp3", &values), ["03:00"]);
        assert_eq!(compact_fields("mp4", &values), ["1920×1080", "03:00"]);
        assert!(compact_fields("txt", &values).is_empty());
    }
    #[test]
    fn absent_or_empty_properties_are_not_invented() {
        assert!(compact_fields("cr3", &[]).is_empty());
        assert!(
            compact_fields("mp4", &properties(&[("System.Video.FrameWidth", "1920")])).is_empty()
        );
        assert!(compact_fields("jpg", &properties(&[("System.Image.Dimensions", " ")])).is_empty());
    }
    #[test]
    fn formats_existing_windows_media_values_without_guid_or_unknown_subtype() {
        let fields = fields_at_level("mp4", &properties(&[
            ("System.Video.FrameWidth", "1920"), ("System.Video.FrameHeight", "1080"),
            ("System.Media.Duration", "00:03:00"), ("System.Video.FrameRate", "30.00 frames/second"),
            ("System.Video.Compression", "{34363248-0000-0010-8000-00AA00389B71}"),
        ]), atlas_model::settings::InformationLevel::Extended);
        assert_eq!(fields, ["1920×1080", "03:00", "30 fps"]);
        assert_eq!(display_property("System.Video.FrameRate", "\u{200e}29,97 кадр/с").as_deref(), Some("29.97 fps"));
        assert_eq!(display_property("System.Media.Duration", "01:49:19").as_deref(), Some("01:49:19"));
        assert!(display_property("System.Video.Compression", "MFVideoFormat_Unknown").is_none());
        assert_eq!(display_property("System.Video.Compression", "AVC").as_deref(), Some("H.264"));
        assert!(display_property("System.Video.FrameRate", "NaN fps").is_none());
        assert!(display_property("System.Media.Duration", "00:99:99").is_none());
    }
}

/// Shared grouping policy for Settings and the compact information bar.
pub(crate) fn level_for(settings: &atlas_model::settings::Settings, extension: &str) -> atlas_model::settings::InformationLevel {
    let ext = extension.to_ascii_lowercase();
    if matches!(ext.as_str(), "jpg"|"jpeg"|"jpe"|"png"|"gif"|"bmp"|"webp"|"tif"|"tiff"|"heic"|"heif"|"avif"|"jxl"|"ico"|"psd"|"psb"|"arw"|"cr2"|"cr3"|"crw"|"dng"|"erf"|"nef"|"nrw"|"orf"|"pef"|"raf"|"raw"|"rw2"|"srw"|"srf"|"sr2"|"x3f") { settings.photo_information }
    else if matches!(ext.as_str(), "mp4"|"m4v"|"mov"|"mkv"|"avi"|"wmv"|"webm"|"mpg"|"mpeg"|"mts"|"m2ts"|"ts"|"mp3"|"wav"|"flac"|"ogg"|"opus"|"m4a"|"aac"|"wma"|"aiff"|"alac") { settings.media_information }
    else { settings.other_information }
}

/// Never render a result for an earlier file or an earlier revision of that file.
pub(crate) fn selection_key_matches(key: Option<&(std::path::PathBuf, Option<u64>, Option<std::time::SystemTime>)>, path: &std::path::Path, size: Option<u64>, modified: Option<std::time::SystemTime>) -> bool {
    key.is_some_and(|(p,s,m)| p == path && *s == size && *m == modified)
}
